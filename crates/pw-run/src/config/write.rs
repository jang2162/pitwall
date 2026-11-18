//! 실행 구성 쓰기 — 만들기 · 고치기 · 지우기가 모두 [`write`] 하나(계획 [`plan`] + 적용 [`apply`])를 지난다. 화면(M6-07 — RPC
//! `run.config.save` · `delete`)과 MCP 툴(M6-13 — 계획을 승인에 싣고 승인 뒤 [`apply`])이 이것만 쓴다.
//!
//! - **고칠 때 사람의 것을 지킨다**(`pw_core::config::write` 와 같은 방식 — `toml_edit`): 있는 키는 그 자리에서 값만 바꿔 키 위의
//!   주석 줄 · 값 뒤 주석이 남고, 이 크레이트가 모르는 최상위 키(새 버전 Pitwall 이 더한 키 · 사람이 적어 둔 키)는 그대로 둔다.
//!   아는 키(구성 스키마의 키 — [`known_keys`])에서 새 구성에 없는 것은 지운다(종류를 바꾸면 앞 종류의 필드가 남지 않는다). 없는
//!   값 · 빈 목록 · 빈 표는 적지 않는다. 그래서 개인 파일(`.local.toml`)에 쓰면 공유 파일에만 있는 키는 그대로 비친다(설정 계층에
//!   키를 지우는 문법이 없다 — `db.connection.upsert` 와 같다).
//! - 파일을 TOML 로 읽을 수 없거나 `schema` 가 앱보다 새것이면 **쓰지 않는다**(사람이 고치다 만 파일 · 새 Pitwall 의 파일을 덮지
//!   않는다). 새 파일은 `schema = 1` 로 시작한다.
//! - **계획 뒤 바뀐 파일은 쓰지 않는다**(TOCTOU — `pw_core::registry::plan` 머리 주석): [`apply`] 는 지금 파일이 계획 때 읽은
//!   내용([`RunConfigEdit::before`])과 같을 때만 쓰고, 다르면 conflict 다. 다시 만들지 않는다 — 승인을 지난 쓰기는 사람이 본 diff
//!   말고는 쓰지 않는다. 같은 프로세스의 쓰기는 [`IN_PROCESS`] 로 한 줄로 선다.
//! - **링크**: 저장소 안(`.pitwall/run/`)의 구성 파일은 공유 · 개인 모두 링크면 쓰지 않고, 쓸 폴더가 체크아웃 밖으로 풀리면 쓰지
//!   않는다 — 그 링크는 저장소가 정한 것이라 대상이 저장소 밖 사람의 파일일 수 있다. 개인 파일을 처음 만들면 `.pitwall/.gitignore`
//!   에 `*.local.toml` 을 둔다(`ConfigSource::prepare_local_write`, D9). 개인 전용 폴더(`$CONFIG/projects/<slug>/run/`)는 dotfiles
//!   링크를 따른다.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use pw_core::config::migrate::write_atomic;
use pw_core::config::write::{Links, WriteError};
use pw_rpc::{RpcError, code};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use toml_edit::{DocumentMut, InlineTable, Item, Table, TableLike};

use super::read::{RunConfigPlace, to_file_value};
use super::{DIR, FILE_KEYS, RunConfig, SCHEMA, json_schema, valid_id, validate};

/// 이 프로세스의 구성 쓰기를 한 줄로 세운다(비교 → 쓰기 사이에 같은 프로세스의 다른 쓰기가 끼지 않게).
static IN_PROCESS: Mutex<()> = Mutex::new(());

/// Which file to write: the shared, committed `run/<id>.toml` or the personal `run/<id>.local.toml`.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RunConfigLocation {
    /// `run/<id>.toml` of the active worktree (of the personal folder for a personal-only project).
    #[default]
    Shared,
    /// `run/<id>.local.toml`, the personal override (the main checkout's when a linked worktree has none).
    Local,
}

/// 디스크에 쓰지 않고 만든 쓰기 계획. 승인을 청하는 툴은 `before` · `after` 를 diff 로 보이고 승인 뒤 [`apply`] 로 그대로 쓴다.
#[derive(Debug, Clone)]
pub struct RunConfigEdit {
    pub path: PathBuf,
    /// 계획 때 읽은 내용. 없던 파일이면 `None`.
    pub before: Option<String>,
    /// 쓸 내용. `None` 이면 파일을 지운다.
    pub after: Option<String>,
    location: RunConfigLocation,
}

impl RunConfigEdit {
    /// 쓰면 파일이 바뀌는가(같은 내용 · 없는 파일 지우기면 아니다).
    pub fn changes(&self) -> bool {
        self.before != self.after
    }
}

/// 구성 `id` 의 `location` 파일을 `config` 로 만들거나 고치고, `None` 이면 그 파일을 지운다. [`plan`] 뒤 [`apply`] 다. 연결 범위
/// (그 프로젝트 창인가)는 부르는 쪽이 본다.
pub fn write(
    place: &RunConfigPlace,
    id: &str,
    location: RunConfigLocation,
    config: Option<&RunConfig>,
) -> Result<RunConfigEdit, RpcError> {
    let edit = plan(place, id, location, config)?;
    apply(place, &edit)?;
    Ok(edit)
}

/// 쓰기 계획. 디스크에 쓰지 않는다(개인 파일의 `.gitignore` 도 [`apply`] 가 둔다). `config` 는 [`validate`] 를 지나야 한다(invalid
/// params). 파일이 TOML 이 아니거나 앱보다 새 `schema` 면 conflict.
pub fn plan(
    place: &RunConfigPlace,
    id: &str,
    location: RunConfigLocation,
    config: Option<&RunConfig>,
) -> Result<RunConfigEdit, RpcError> {
    check_id(id)?;
    if let Some(config) = config {
        validate(config, &place.root).map_err(RpcError::invalid_params)?;
    }
    let path = file_of(place, id, location);
    let before = read(&path).map_err(|e| io_error("read", &path, &e))?;
    let after = match config {
        None => None,
        Some(config) => Some(render(before.as_deref(), config).map_err(|message| {
            RpcError::new(code::CONFLICT, format!("{}: {message}", path.display()))
        })?),
    };
    Ok(RunConfigEdit {
        path,
        before,
        after,
        location,
    })
}

/// 계획한 내용을 쓴다 — 지금 파일이 계획 때 읽은 내용과 같을 때만(아니면 conflict, 쓰지 않는다). 바뀔 것이 없으면 아무것도 하지
/// 않는다.
pub fn apply(place: &RunConfigPlace, edit: &RunConfigEdit) -> Result<(), RpcError> {
    let _one_at_a_time = IN_PROCESS.lock().unwrap_or_else(PoisonError::into_inner);
    let path = &edit.path;
    if !edit.changes() {
        return Ok(());
    }
    let links = guard(place, edit)?;
    if links == Links::Refuse && is_link(path) {
        return Err(refused_link(path));
    }
    let now = read(path).map_err(|e| io_error("read", path, &e))?;
    if now != edit.before {
        return Err(RpcError::new(
            code::CONFLICT,
            format!(
                "{} changed after it was read; nothing was written",
                path.display()
            ),
        ));
    }
    match (&edit.before, &edit.after) {
        (_, None) => fs::remove_file(path).map_err(|e| io_error("remove", path, &e))?,
        (Some(_), Some(after)) => {
            write_atomic(path, after).map_err(|e| io_error("write", path, &e))?
        }
        (None, Some(after)) => match create(path, after) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                return Err(RpcError::new(
                    code::CONFLICT,
                    format!(
                        "{} was created after it was read; nothing was written",
                        path.display()
                    ),
                ));
            }
            Err(e) => return Err(io_error("write", path, &e)),
        },
    }
    tracing::info!(target: "pw_run", project = %place.project_id, path = %path.display(), deleted = edit.after.is_none(), "run config written");
    Ok(())
}

/// 구성 id 의 파일 자리.
pub fn file_of(place: &RunConfigPlace, id: &str, location: RunConfigLocation) -> PathBuf {
    match location {
        RunConfigLocation::Shared => place.source.dir().join(DIR).join(format!("{id}.toml")),
        RunConfigLocation::Local => place.source.local_file(&format!("{DIR}/{id}")),
    }
}

pub(crate) fn check_id(id: &str) -> Result<(), RpcError> {
    if valid_id(id) {
        Ok(())
    } else {
        Err(RpcError::invalid_params(format!(
            "run config id `{id}` must be 1-64 letters, digits, `-` or `_`"
        )))
    }
}

/// 쓰기 전 확인(모듈 문서의 「링크」). 쓸 때의 링크 규칙을 돌려준다.
fn guard(place: &RunConfigPlace, edit: &RunConfigEdit) -> Result<Links, RpcError> {
    if !place.repository {
        return Ok(Links::Follow);
    }
    if edit.location == RunConfigLocation::Local && edit.after.is_some() {
        return place
            .source
            .prepare_local_write(&edit.path)
            .map_err(write_error);
    }
    // 공유 파일(또는 개인 파일 지우기): 쓸 폴더가 체크아웃 밖으로 풀리면 거절한다.
    let config_dir = [Some(place.source.dir()), place.source.local_fallback_dir()]
        .into_iter()
        .flatten()
        .find(|dir| edit.path.starts_with(dir))
        .ok_or_else(|| {
            RpcError::internal(format!(
                "{} is not inside the project's config folder",
                edit.path.display()
            ))
        })?;
    let checkout = config_dir.parent().unwrap_or(config_dir);
    let real_checkout =
        fs::canonicalize(checkout).map_err(|e| io_error("resolve", checkout, &e))?;
    let folder = edit.path.parent().unwrap_or(config_dir);
    let existing = folder
        .ancestors()
        .find(|a| fs::symlink_metadata(a).is_ok())
        .unwrap_or(checkout);
    let real = fs::canonicalize(existing).map_err(|e| io_error("resolve", existing, &e))?;
    if !real.starts_with(&real_checkout) {
        return Err(RpcError::new(
            code::CONFLICT,
            format!(
                "{} resolves to {}, outside the project {}; Pitwall does not write a run config there",
                existing.display(),
                real.display(),
                real_checkout.display()
            ),
        ));
    }
    Ok(Links::Refuse)
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

fn refused_link(path: &Path) -> RpcError {
    RpcError::new(
        code::CONFLICT,
        format!(
            "{}: is a symbolic link; Pitwall does not write a run config through a link inside the repository \
             (its target may be outside the project) — replace the link with a plain file or edit it by hand",
            path.display()
        ),
    )
}

fn write_error(e: WriteError) -> RpcError {
    match e {
        WriteError::Refused { .. } | WriteError::Changed { .. } | WriteError::Busy { .. } => {
            RpcError::new(code::CONFLICT, e.to_string())
        }
        e => RpcError::internal(e.to_string()),
    }
}

fn io_error(what: &str, path: &Path, e: &io::Error) -> RpcError {
    RpcError::internal(format!("cannot {what} {}: {e}", path.display()))
}

/// 파일 내용. 없으면 `None`. 대상이 없는 심링크는 없는 파일이 아니라 오류다(만들면 링크를 덮는다).
fn read(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if fs::symlink_metadata(path).is_ok() {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "symlink target does not exist",
                ))
            } else {
                Ok(None)
            }
        }
        Err(e) => Err(e),
    }
}

fn create(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

// --- TOML ------------------------------------------------------------------------------------------------------

/// 구성 스키마의 최상위 키(파일 키 — snake_case)와 `schema`. 쓸 때 새 구성에 없으면 지우는 키다 — 그 밖은 사람 · 새 버전의 키라
/// 그대로 둔다. 종류를 더하는 행이 갈래를 더하면 그 필드도 저절로 든다(스키마에서 읽는다).
pub fn known_keys() -> &'static BTreeSet<String> {
    static KEYS: OnceLock<BTreeSet<String>> = OnceLock::new();
    KEYS.get_or_init(|| {
        let schema = json_schema();
        let root = schema.as_value();
        let mut keys = BTreeSet::new();
        collect_keys(root, root, &mut keys, 0);
        keys.insert(pw_core::config::migrate::SCHEMA_KEY.to_owned());
        keys.into_iter()
            .map(|key| {
                FILE_KEYS
                    .iter()
                    .find(|(_, rpc)| *rpc == key)
                    .map_or(key, |(file, _)| (*file).to_owned())
            })
            .collect()
    })
}

/// `schema` 의 `properties` 키들. `oneOf` · `anyOf` · `allOf` 갈래와 그것이 `$ref` 로 가리키는 정의는 따라가고, 속성 안(`env` 의
/// 값 타입 등)으로는 들어가지 않는다 — 펼친(flatten) 종류의 필드가 최상위 키다.
fn collect_keys(root: &Value, schema: &Value, keys: &mut BTreeSet<String>, depth: usize) {
    if depth > 8 {
        return;
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str)
        && let Some(target) = reference
            .strip_prefix('#')
            .and_then(|pointer| root.pointer(pointer))
    {
        collect_keys(root, target, keys, depth + 1);
    }
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        keys.extend(props.keys().cloned());
    }
    for branch in ["oneOf", "anyOf", "allOf"] {
        for item in schema
            .get(branch)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            collect_keys(root, item, keys, depth + 1);
        }
    }
}

/// 새로 쓰는 최상위 키의 자리 순서(있는 파일은 있는 자리를 지킨다).
fn rank(key: &str) -> u8 {
    match key {
        "schema" => 0,
        "name" => 1,
        "type" => 2,
        "cwd" => 4,
        "env" => 5,
        "env_file" => 6,
        "profiles" => 7,
        "debug" => 8,
        _ => 3,
    }
}

/// 새 파일이면 인라인 표로 쓰는 표 키(`env = { … }`). 그 밖의 표는 `[표]` 다(`[debug]` · `[profiles.<이름>]`).
const INLINE_TABLES: &[&str] = &["env"];

/// `before`(없으면 새 파일)에 `config` 를 쓴 내용.
fn render(before: Option<&str>, config: &RunConfig) -> Result<String, String> {
    let mut doc: DocumentMut = match before {
        Some(text) => text
            .parse()
            .map_err(|e: toml_edit::TomlError| format!("not valid TOML, not changing it: {e}"))?,
        None => DocumentMut::new(),
    };
    match doc.get(pw_core::config::migrate::SCHEMA_KEY) {
        None => {}
        Some(item) => match item.as_integer() {
            Some(v) if v <= i64::from(SCHEMA) => {}
            Some(v) => {
                return Err(format!(
                    "`schema = {v}` is newer than this app writes ({SCHEMA}), not changing it"
                ));
            }
            None => return Err("`schema` is not an integer, not changing it".to_owned()),
        },
    }
    let had_schema = doc.contains_key(pw_core::config::migrate::SCHEMA_KEY);
    let mut value = to_file_value(config);
    prune(&mut value, false);
    value.insert(
        pw_core::config::migrate::SCHEMA_KEY.to_owned(),
        Value::from(SCHEMA),
    );
    sync(doc.as_table_mut(), &value, Level::Top, false);
    if !had_schema {
        // `schema` 는 첫 줄이다. 다른 키의 순서는 그대로(안정 정렬).
        doc.as_table_mut().sort_values_by(|a, _, b, _| {
            (b.get() == pw_core::config::migrate::SCHEMA_KEY)
                .cmp(&(a.get() == pw_core::config::migrate::SCHEMA_KEY))
        });
    }
    Ok(doc.to_string())
}

/// 없는 값(`null`)을 빼고, 빈 목록 · 빈 표를 뺀다. `keep_children` 이면 그 표의 바로 아래 항목은 비어도 둔다(빈 프로필도 고를 수
/// 있는 이름이다).
fn prune(map: &mut Map<String, Value>, keep_children: bool) {
    map.retain(|key, value| {
        match value {
            Value::Null => return false,
            Value::Object(inner) => prune(inner, key == "profiles" && !keep_children),
            _ => {}
        }
        keep_children
            || !matches!(value, Value::Array(a) if a.is_empty())
                && !matches!(value, Value::Object(o) if o.is_empty())
    });
}

/// 표의 자리 — 어느 키가 이 크레이트의 것인가(지워도 되는가)를 정한다.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    /// 최상위 — 구성 스키마의 키만 이 크레이트의 것([`known_keys`]).
    Top,
    /// `[profiles]` — 이름이 모두 프로필이라 새 구성에 없는 프로필은 지운다.
    Profiles,
    /// `[profiles.<이름>]` — `RunProfile` 의 키만 이 크레이트의 것([`profile_keys`]). 새 버전이 더한 키는 지킨다(리뷰 3).
    Profile,
    /// 키가 모두 값인 표(`env` · `[debug]` — `[debug]` 는 모든 키를 들고 다닌다) — 새 값에 없는 키는 지운다.
    Free,
}

impl Level {
    fn known(self) -> Option<&'static BTreeSet<String>> {
        match self {
            Level::Top => Some(known_keys()),
            Level::Profile => Some(profile_keys()),
            Level::Profiles | Level::Free => None,
        }
    }

    fn child(self, key: &str) -> Level {
        match (self, key) {
            (Level::Top, "profiles") => Level::Profiles,
            (Level::Profiles, _) => Level::Profile,
            _ => Level::Free,
        }
    }
}

/// `RunProfile` 의 키(`env` · `args`).
fn profile_keys() -> &'static BTreeSet<String> {
    static KEYS: OnceLock<BTreeSet<String>> = OnceLock::new();
    KEYS.get_or_init(|| {
        let schema = schemars::schema_for!(super::RunProfile);
        let root = schema.as_value();
        let mut keys = BTreeSet::new();
        collect_keys(root, root, &mut keys, 0);
        keys
    })
}

/// `table` 을 `new` 에 맞춘다. 새 값에 없는 키 가운데 그 자리([`Level`])에서 이 크레이트의 것만 지운다 — 모르는 키는 둔다.
fn sync(table: &mut dyn TableLike, new: &Map<String, Value>, level: Level, inline: bool) {
    let known = level.known();
    let stale: Vec<String> = table
        .iter()
        .map(|(k, _)| k.to_owned())
        .filter(|k| !new.contains_key(k) && known.is_none_or(|known| known.contains(k)))
        .collect();
    for key in stale {
        table.remove(&key);
    }
    let mut fresh: Vec<(&String, &Value)> =
        new.iter().filter(|(k, _)| !table.contains_key(k)).collect();
    fresh.sort_by_key(|(k, _)| (rank(k), (*k).clone()));
    for (key, value) in new {
        let Some(item) = table.get_mut(key) else {
            continue;
        };
        match value {
            Value::Object(inner) if item.is_table_like() => {
                let inline_here = item.is_inline_table();
                if let Some(child) = item.as_table_like_mut() {
                    sync(child, inner, level.child(key), inline || inline_here);
                }
            }
            _ => {
                let mut next = to_value(value);
                if let Some(old) = item.as_value() {
                    *next.decor_mut() = old.decor().clone();
                }
                *item = Item::Value(next);
            }
        }
    }
    for (key, value) in fresh {
        table.insert(key, new_item(key, value, inline));
    }
}

/// 새 항목.
fn new_item(key: &str, value: &Value, inline: bool) -> Item {
    match value {
        Value::Object(map) if !inline && !INLINE_TABLES.contains(&key) => {
            let mut table = Table::new();
            // 표만 담은 표(`profiles`)는 머리글을 두지 않는다 — `[profiles.dev]` 만 보인다.
            table.set_implicit(map.values().all(Value::is_object) && !map.is_empty());
            for (k, v) in map {
                table.insert(k, new_item(k, v, false));
            }
            Item::Table(table)
        }
        value => Item::Value(to_value(value)),
    }
}

/// JSON 값을 TOML 값으로(표는 인라인 표). `null` 은 쓰기 전에 빠진다([`prune`]).
fn to_value(value: &Value) -> toml_edit::Value {
    match value {
        Value::String(s) => s.as_str().into(),
        Value::Bool(b) => (*b).into(),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => i.into(),
            (None, Some(f)) => f.into(),
            (None, None) => n.to_string().into(),
        },
        Value::Array(items) => toml_edit::Value::Array(items.iter().map(to_value).collect()),
        Value::Object(map) => {
            let mut table = InlineTable::new();
            for (k, v) in map {
                table.insert(k, to_value(v));
            }
            toml_edit::Value::InlineTable(table)
        }
        Value::Null => "".into(),
    }
}
