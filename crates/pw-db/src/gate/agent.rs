//! 에이전트의 DB 입구(16-m5-plan.md M5-06 ①, 결정 M5-D2 · M5-D27 · M5-D37). 판정 순서와 세 기억(「사람이 연 연결」 · 「승인받은
//! 연결」 · 대상에 묶은 표지)은 상위 모듈 머리 주석의 「에이전트의 입구」.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pw_core::config::mcp_policy::{self, Production};
use pw_core::registry::Project;
use pw_core::registry::agent::AgentGrant;
use pw_core::registry::path::simplify;
use pw_core::registry::plan::Approved;
use pw_db_types::DriverKind;
use pw_rpc::{RpcError, SessionId, code as rpc_code};
use serde_json::json;

use super::DbGrant;
use crate::config::{DbConnectionConfig, Loaded, LoadedConnection, check_connection_id, lock};
use crate::session::DbSessions;

/// `data.reason`: 사람이 이번 앱 실행에서 그 프로젝트 창으로 (그 연결 id, 지금 대상)을 연 적이 없다.
pub const NOT_OPENED_BY_USER: &str = "notOpenedByUser";
/// `data.reason`: 프로덕션 표지가 있는 대상이다(`[mcp.policy] production` 의 `block` — 기본). `ask` 면 사람이 연 연결에 한해
/// 호출마다 승인을 청한다([`DbGrant::needs_approval`]).
pub const PRODUCTION_BLOCKED: &str = "productionBlocked";

/// 푼 대상 — 「사람이 연 연결」 의 열쇠(표지의 열쇠는 [`DbTarget::mark_key`]). 드라이버 · 호스트 · 포트(적지 않았으면 드라이버
/// 기본) · DB · 사용자 · 터널 목적지, SQLite 면 파일의 실제 경로(링크를 끝까지 푼 것 — 워크트리마다 상대 경로가 다른 파일을
/// 가리킨다). 호스트는 소문자로 맞춘다(DNS 이름은 대소문자를 가리지 않는다 — `DB.example.com` 으로 표지를 비껴가지 못하게). TLS
/// 설정은 넣지 않는다 — 같은 서버 · DB 를 `sslmode` 만 바꿔 가리켜 표지를 비껴가지 못하게(TLS 의 확인은 `crate::trust` 가 따로 본다).
/// 루프백 별칭은 하나로 맞춘다([`normalize_host`]). 남는 한계: 그 밖의 호스트 별칭(DNS 이름과 그 IP), 터널 목적지의 다른 표기,
/// DB 이름의 대소문자는 다른 대상으로 본다.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbTarget {
    pub driver: DriverKind,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub user: Option<String>,
    pub tunnel: Option<String>,
    pub file: Option<PathBuf>,
}

impl DbTarget {
    /// 연결 설정의 대상. `root` 는 그 설정을 읽은 활성 워크트리 안의 프로젝트 루트(SQLite 상대 경로의 기준).
    pub fn of(config: &DbConnectionConfig, root: &Path) -> Self {
        let file = config.path.as_deref().map(|path| {
            let path = Path::new(path);
            let joined = if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            };
            std::fs::canonicalize(&joined).map_or(joined, simplify)
        });
        Self {
            driver: config.driver,
            host: config.host.as_deref().map(normalize_host),
            port: config.effective_port(),
            database: config.database.clone(),
            user: config.user.clone(),
            tunnel: config.ssh_tunnel.as_ref().map(|t| t.host_ref.clone()),
            file,
        }
    }

    /// 푼 연결의 대상. SQLite 는 신뢰 판정이 드라이버에 넘길 실제 경로(`spec` 의 파일)로 만든다 — 판정한 경로와 견주는 경로가
    /// 갈리지 않게(그 사이에 링크를 바꿔 끼워도).
    pub(crate) fn of_resolved(resolved: &crate::trust::ResolvedConnection) -> Self {
        let mut target = Self::of(&resolved.config, &resolved.root);
        if let pw_db_types::Target::File { path } = &resolved.spec.target {
            target.file = Some(path.clone());
        }
        target
    }

    /// 표지의 열쇠(결정 M5-D37 — 메인 판정 2026-11-14). 한 연결로 다른 DB 를 읽을 수 있는 드라이버(MySQL · MariaDB — 드라이버 id
    /// `mysql`)는 DB 이름을 뺀다: 그 서버의 어느 DB 가 프로덕션이면 그 서버 · 사용자로 가는 에이전트 호출은 모두 프로덕션이다
    /// (`db_query` 가 SQL 글 안의 `other_db.t` 를 지킬 수 없다). Redis 도 DB 번호를 뺀다(메인 판정 2026-11-17 — M5-19): 한 연결이
    /// `SELECT` 로 다른 번호로 옮겨 가고 번호 사이에 권한 경계가 없다. 번호를 열쇠에 두면 같은 번호의 다른 표기(`database` 입력
    /// `"00"` · `"+0"`, 설정에서 뺀 번호와 `"0"`)로 표지를 비껴간다 — 드라이버는 그 모두를 같은 번호로 읽는다. PG 는 DB 이름을
    /// 둔다(한 연결이 다른 DB 를 읽지 못한다). SQLite 는 파일이 열쇠다(`ATTACH` 는 실행 분류의 몫 — M5-07).
    ///
    /// Redis 는 사용자도 맞춘다(메인 판정 2026-11-17 — M5-19 리뷰 R1): 사용자를 적지 않은 것 · 빈 글 · `default` 는 모두 Redis 의
    /// `default` 사용자다(드라이버는 빈 사용자를 적지 않은 것으로 다룬다). ACL 사용자 이름은 대소문자를 가리므로 `Default` 는 다른
    /// 사용자다.
    pub fn mark_key(&self) -> Self {
        match self.driver {
            DriverKind::Mysql => Self {
                database: None,
                ..self.clone()
            },
            DriverKind::Redis => Self {
                database: None,
                user: self
                    .user
                    .clone()
                    .filter(|u| !u.is_empty() && u != REDIS_DEFAULT_USER),
                ..self.clone()
            },
            _ => self.clone(),
        }
    }

    /// 호출이 설정의 DB 대신 고른 DB 의 대상. `None` 이면 그대로.
    pub fn with_database(&self, database: Option<&str>) -> Self {
        match database {
            Some(db) => Self {
                database: Some(db.to_owned()),
                ..self.clone()
            },
            None => self.clone(),
        }
    }
}

/// 대상의 호스트 표기를 맞춘다: 끝의 점을 떼고 소문자로, 그리고 **루프백 별칭을 하나로**(메인 판정 2026-11-17 — M5-17 재리뷰
/// G1) — `localhost` · `*.localhost` · `127.0.0.0/8` · `::1` · `0.0.0.0` · `::` · IPv4 를 담은 IPv6(`::ffff:127.0.0.1`) · 대괄호
/// 표기는 모두 `localhost` 다. 에이전트가 같은 로컬 서버를 다른 별칭으로 가리켜 표지 · 「사람이 연 연결」 · 승인을 비껴가지
/// 못하게. 표지 열쇠 · 「사람이 연 연결」 · 「승인받은 연결」 이 모두 이 대상을 쓴다. 그 밖의 별칭(DNS 이름과 그 IP, CNAME)은 다른
/// 대상이다(남는 한계 — 「이 연결 열기」 다이얼로그가 대상을 보이는 것이 방어).
fn normalize_host(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(&host);
    let loopback = bare == "localhost"
        || bare.ends_with(".localhost")
        || bare.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_unspecified(),
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6
                        .to_ipv4_mapped()
                        .is_some_and(|v4| v4.is_loopback() || v4.is_unspecified())
            }
        });
    if loopback { LOOPBACK.to_owned() } else { host }
}

/// 루프백 별칭을 맞춘 표기.
const LOOPBACK: &str = "localhost";

/// Redis 에서 사용자를 적지 않은 `AUTH` 가 쓰는 사용자.
const REDIS_DEFAULT_USER: &str = "default";

/// 에이전트에게 적용하는 표지(결정 M5-D37). 조여지기만 한다([`DbMarks::tighten`]).
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbMarks {
    pub production: bool,
    pub read_only: bool,
}

impl DbMarks {
    pub fn of(config: &DbConnectionConfig) -> Self {
        Self {
            production: config.production,
            read_only: config.read_only,
        }
    }

    /// 읽어 올린 연결의 표지: 병합 값과 각 레이어 파일에 적힌 표지의 합(메인 판정 2026-11-17 — 재리뷰 G2). 에이전트에게 쓰는 모든
    /// 자리(관문 · 표지 기억 · 쓰기 판정)가 이것을 본다.
    pub(crate) fn of_loaded(loaded: &LoadedConnection, config: &DbConnectionConfig) -> Self {
        Self::of(config).tighten(loaded.layer_marks)
    }

    /// 둘 가운데 엄한 것.
    pub fn tighten(self, other: Self) -> Self {
        Self {
            production: self.production || other.production,
            read_only: self.read_only || other.read_only,
        }
    }
}

/// 대상 → 본 가장 엄한 표지. `Connections` 가 하나 갖는다. **앱 실행을 넘어 영속한다**(결정 M5-D37 — 사용자 결정 2026-11-18,
/// M5-17b): 더한 표지는 Host 의 상태 DB(`$DATA/state.db` 의 `db_marks` — `pw_core::registry::db_marks`)에도 쓰고, 에이전트의 DB
/// 툴 호출마다([`agent_view`] → [`MarkMemory::sync`]) 그 표를 다시 읽어 더한다. 그래서 에이전트가 자기 파일 툴로 설정의
/// `production` 을 지우고 앱이 다시 켜져도 그 대상은 프로덕션이다. 기억은 조여지기만 하고, 지우는 길은 사람이 그 프로젝트 창에서
/// 부르는 [`MarkMemory::forget`](`db.connection.forgetMarks`) 하나다.
///
/// **fail-closed**: 상태 DB 를 쓸 수 없거나(`readOnly` · `memory`) 읽기 · 쓰기가 실패하면 [`MarkMemory::sync`] 가 거절한다 —
/// 에이전트의 DB 툴이 모두 거절된다(사람의 창은 그대로 쓴다). 쓰지 못한 표지는 이번 실행의 메모리에 남고 다음 `sync` 가 다시 쓴다.
///
/// 열쇠는 판 표지 `v1:` 와 [`DbTarget::mark_key`] 의 JSON 글([`mark_store_key`]). **열쇠 글이 바뀌면 앞 실행들의 기억이 소리 없이
/// 고아가 된다(fail-open)** — 그래서 `gate::tests::mark_store_keys_are_golden` 이 드라이버마다 글 그대로를 지킨다. `DbTarget` 의
/// 칸 · 이름 · 정규화(`normalize_host` 등)를 바꾸는 행은 그 시험이 깨진다 — [`MARK_KEY_VERSION`] 을 올리고 `state.db` 마이그레이션으로
/// 옛 열쇠를 옮긴다. 모르는 판의 행이 남아 있으면 [`MarkMemory::sync`] 가 거절한다(옮기기를 빠뜨리면 조용히 풀리지 않고 막힌다).
/// 다른 프로세스(같은 상태 DB 를 쓰는 다른 앱 실행)가 지운 기억은 이 프로세스의 메모리에 남는다 — 더 엄한 쪽이다.
#[derive(Default)]
pub(crate) struct MarkMemory {
    seen: Mutex<HashMap<String, DbMarks>>,
    /// 상태 DB 에 아직 쓰지 못한 열쇠(쓰기 실패). 비어 있지 않으면 [`MarkMemory::sync`] 가 다시 쓰고, 못 쓰면 거절한다.
    unsaved: Mutex<HashSet<String>>,
    store: std::sync::OnceLock<pw_core::registry::Registry>,
}

/// 상태 DB 열쇠 글의 판. 열쇠 글의 꼴을 바꾸면 올리고 옛 행을 옮긴다([`MarkMemory`] 의 주석).
pub(crate) const MARK_KEY_VERSION: &str = "v1";

/// 표지 열쇠의 상태 DB 글: `v1:` + [`DbTarget::mark_key`] 의 JSON. 직렬화가 실패하면(UTF-8 이 아닌 SQLite 경로) JSON 대신 `Debug`
/// 표기 — 둘 다 같은 대상에 늘 같은 글이다.
pub(crate) fn mark_store_key(target: &DbTarget) -> String {
    let key = target.mark_key();
    let text = serde_json::to_string(&key).unwrap_or_else(|_| format!("{key:?}"));
    format!("{MARK_KEY_VERSION}:{text}")
}

fn stored(marks: DbMarks) -> pw_core::registry::db_marks::StoredDbMarks {
    pw_core::registry::db_marks::StoredDbMarks {
        production: marks.production,
        read_only: marks.read_only,
    }
}

/// `data.reason`: 표지 기억을 상태 DB 에 쓰거나 읽지 못한다 — 에이전트의 DB 툴을 거절한다(fail-closed).
pub const MARK_MEMORY_UNAVAILABLE: &str = "markMemoryUnavailable";

fn mark_memory_unavailable(why: &str) -> RpcError {
    RpcError::new(
        rpc_code::STORAGE_UNAVAILABLE,
        "Pitwall cannot keep its memory of production and read-only DB targets (its state database is unavailable), \
         so agent DB tools are off until it can; open Pitwall to see why",
    )
    .with_data(json!({ "reason": MARK_MEMORY_UNAVAILABLE, "detail": why }))
}

impl MarkMemory {
    /// 상태 DB(Host 의 레지스트리)를 붙인다. 두 번째부터는 무시한다.
    pub fn attach(&self, store: pw_core::registry::Registry) {
        let _ = self.store.set(store);
    }

    /// 읽어 올린 연결 설정들의 표지를 더한다(`Connections::load` · `refresh`). 틀린 설정은 대상을 풀 수 없어 건너뛴다. 메모리를
    /// 조인 열쇠는 상태 DB 에도 쓴다 — 쓰지 못하면 [`Self::sync`] 가 다시 쓴다.
    pub fn see(&self, root: &Path, list: &[LoadedConnection]) {
        let seen: Vec<(String, DbMarks)> = list
            .iter()
            .filter_map(|c| c.config.as_ref().ok().map(|config| (c, config)))
            .map(|(c, config)| {
                (
                    mark_store_key(&DbTarget::of(config, root)),
                    DbMarks::of_loaded(c, config),
                )
            })
            .filter(|(_, marks)| *marks != DbMarks::default())
            .collect();
        if seen.is_empty() {
            return;
        }
        let tightened: Vec<String> = {
            let mut memory = lock(&self.seen);
            seen.into_iter()
                .filter_map(|(key, marks)| {
                    let entry = memory.entry(key.clone()).or_default();
                    let after = entry.tighten(marks);
                    (after != *entry).then(|| {
                        *entry = after;
                        key
                    })
                })
                .collect()
        };
        if !tightened.is_empty() {
            lock(&self.unsaved).extend(tightened);
            if let Err(e) = self.flush() {
                tracing::warn!(target: "pw_db", %e, "cannot save the db mark memory; agent db tools are refused until it can");
            }
        }
    }

    /// 쓰지 못한 열쇠를 상태 DB 에 쓴다.
    fn flush(&self) -> Result<(), String> {
        let store = self.store.get().ok_or("no state database is attached")?;
        let pending: Vec<(String, pw_core::registry::db_marks::StoredDbMarks)> = {
            let unsaved = lock(&self.unsaved);
            if unsaved.is_empty() {
                return Ok(());
            }
            let memory = lock(&self.seen);
            unsaved
                .iter()
                .map(|key| {
                    (
                        key.clone(),
                        stored(memory.get(key).copied().unwrap_or_default()),
                    )
                })
                .collect()
        };
        store.add_db_marks(&pending).map_err(|e| e.to_string())?;
        // 쓰는 사이 다른 호출이 같은 열쇠를 더 조였으면(그 쓰기는 실패했을 수 있다) 남겨 둔다 — 다음 `sync` 가 그 값을 쓴다.
        // 잠금 순서는 `unsaved` → `seen`(위와 같다).
        let mut unsaved = lock(&self.unsaved);
        let memory = lock(&self.seen);
        for (key, written) in &pending {
            if memory.get(key).copied().map(stored) == Some(*written) {
                unsaved.remove(key);
            }
        }
        Ok(())
    }

    /// 에이전트의 DB 호출 앞(블로킹): 상태 DB 의 기억을 읽어 메모리에 더하고(다른 실행이 남긴 것), 쓰지 못한 표지를 다시 쓴다.
    /// 상태 DB 를 쓸 수 없거나 읽기 · 쓰기가 실패하면 `markMemoryUnavailable`(fail-closed).
    pub fn sync(&self) -> Result<(), RpcError> {
        let store = self
            .store
            .get()
            .ok_or_else(|| mark_memory_unavailable("no state database is attached"))?;
        if let Some(reason) = store.status().unwritable_reason() {
            tracing::warn!(target: "pw_db", reason, "the state database is not writable; agent db tools are refused");
            return Err(mark_memory_unavailable(reason));
        }
        let rows = store.db_marks().map_err(|e| {
            tracing::warn!(target: "pw_db", %e, "cannot read the db mark memory; agent db tools are refused");
            mark_memory_unavailable(&e.to_string())
        })?;
        let prefix = format!("{MARK_KEY_VERSION}:");
        if let Some((key, _)) = rows.iter().find(|(key, _)| !key.starts_with(&prefix)) {
            let version = key.split(':').next().unwrap_or_default();
            tracing::warn!(target: "pw_db", version, "the db mark memory has keys of another version; agent db tools are refused");
            return Err(mark_memory_unavailable(&format!(
                "remembered marks with key version `{version}` this build does not read"
            )));
        }
        {
            let mut memory = lock(&self.seen);
            for (key, marks) in rows {
                let entry = memory.entry(key).or_default();
                *entry = entry.tighten(DbMarks {
                    production: marks.production,
                    read_only: marks.read_only,
                });
            }
        }
        self.flush().map_err(|e| {
            tracing::warn!(target: "pw_db", %e, "cannot save the db mark memory; agent db tools are refused");
            mark_memory_unavailable(&e)
        })
    }

    /// 그 대상에서 본 가장 엄한 표지(본 적 없으면 표지 없음). 메모리만 본다 — 상태 DB 의 것은 [`Self::sync`] 가 더해 둔다.
    pub fn get(&self, target: &DbTarget) -> DbMarks {
        lock(&self.seen)
            .get(&mark_store_key(target))
            .copied()
            .unwrap_or_default()
    }

    /// 그 대상에 기억된 표지 — 메모리와 상태 DB 의 것(사람의 창이 본다, `db.connection.marks`). 상태 DB 를 읽지 못하면 오류.
    /// 블로킹이다.
    pub fn remembered(&self, target: &DbTarget) -> Result<DbMarks, RpcError> {
        let key = mark_store_key(target);
        let store = self
            .store
            .get()
            .ok_or_else(|| RpcError::internal("db connections are not attached to a host"))?;
        let rows = store.db_marks().map_err(crate::config::registry_error)?;
        Ok(rows
            .into_iter()
            .filter(|(k, _)| *k == key)
            .fold(self.get(target), |marks, (_, m)| {
                marks.tighten(DbMarks {
                    production: m.production,
                    read_only: m.read_only,
                })
            }))
    }

    /// 그 대상의 기억을 지운다 — 사람이 그 프로젝트 창에서만(`db.connection.forgetMarks`). 상태 DB 와 메모리 둘 다. 지운 표지를
    /// 돌려준다. 상태 DB 를 쓰지 못하면 지우지 않는다(메모리도 그대로). 블로킹이다.
    pub fn forget(&self, target: &DbTarget) -> Result<DbMarks, RpcError> {
        let key = mark_store_key(target);
        let store = self
            .store
            .get()
            .ok_or_else(|| RpcError::internal("db connections are not attached to a host"))?;
        let before = store
            .forget_db_marks(&key)
            .map_err(crate::config::registry_error)?;
        let in_memory = lock(&self.seen).remove(&key).unwrap_or_default();
        lock(&self.unsaved).remove(&key);
        Ok(in_memory.tighten(DbMarks {
            production: before.production,
            read_only: before.read_only,
        }))
    }
}

/// 「사람이 연 연결」: (프로젝트 id, 연결 id, 푼 대상). `DbSessions` 가 하나 갖는다.
#[derive(Debug, Default)]
pub(crate) struct OpenedMemory(Mutex<HashSet<(String, String, DbTarget)>>);

impl OpenedMemory {
    pub fn record(&self, project_id: &str, connection_id: &str, target: DbTarget) {
        let added = lock(&self.0).insert((project_id.to_owned(), connection_id.to_owned(), target));
        if added {
            tracing::info!(target: "pw_db", project = project_id, connection = connection_id, "db connection opened by the user; agent tools may use it");
        }
    }

    /// 그 (프로젝트, 연결 id)를 어느 대상으로든 기록했는가(시험).
    pub fn has_connection(&self, project_id: &str, connection_id: &str) -> bool {
        lock(&self.0)
            .iter()
            .any(|(p, c, _)| p == project_id && c == connection_id)
    }

    pub fn contains(&self, project_id: &str, connection_id: &str, target: &DbTarget) -> bool {
        lock(&self.0).contains(&(
            project_id.to_owned(),
            connection_id.to_owned(),
            target.clone(),
        ))
    }
}

/// 「승인받은 연결」(16-m5-plan M5-17 ③ — 「이 연결 열기」 승인): (MCP 세션, 프로젝트 id, 연결 id, 푼 대상). `DbSessions` 가 하나
/// 갖는다. 「사람이 연 연결」([`OpenedMemory`])과 따로 둔다 — 승인은 그 세션에만 닿고(MCP 세션 하나가 Host 연결 하나다), 세션이
/// 끝나면([`ApprovedMemory::forget_session`]) 사라진다. 다른 세션 · 사람의 창 기록에 섞이지 않는다.
#[derive(Debug, Default)]
pub(crate) struct ApprovedMemory(Mutex<ApprovedState>);

#[derive(Debug, Default)]
struct ApprovedState {
    approved: HashSet<(SessionId, String, String, DbTarget)>,
    /// 끝나기를 지켜보는 세션(지킴이를 한 번만 띄운다).
    watched: HashSet<SessionId>,
}

impl ApprovedMemory {
    /// 기록한다. 그 세션을 처음 보면 `true` — 부른 쪽이 세션이 끝날 때 [`Self::forget_session`] 을 부르게 한다.
    pub fn record(
        &self,
        session: SessionId,
        project_id: &str,
        connection_id: &str,
        target: DbTarget,
    ) -> bool {
        let mut state = lock(&self.0);
        let added = state.approved.insert((
            session,
            project_id.to_owned(),
            connection_id.to_owned(),
            target,
        ));
        if added {
            tracing::info!(target: "pw_db", project = project_id, connection = connection_id, ?session, "db connection approved for an agent session");
        }
        state.watched.insert(session)
    }

    pub fn contains(
        &self,
        session: SessionId,
        project_id: &str,
        connection_id: &str,
        target: &DbTarget,
    ) -> bool {
        lock(&self.0).approved.contains(&(
            session,
            project_id.to_owned(),
            connection_id.to_owned(),
            target.clone(),
        ))
    }

    /// 그 세션의 승인을 모두 지운다(세션이 끝났다).
    pub fn forget_session(&self, session: SessionId) {
        let mut state = lock(&self.0);
        state.approved.retain(|(s, ..)| *s != session);
        state.watched.remove(&session);
    }

    /// 기록 수(시험).
    pub fn len(&self) -> usize {
        lock(&self.0).approved.len()
    }
}

/// 에이전트의 증거가 묶인 판정.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentBinding {
    pub(super) connection_id: String,
    /// 연결 이름(설정의 `name` — 승인 요청이 보인다).
    pub(super) name: String,
    pub(super) worktree: Option<String>,
    pub(super) target: DbTarget,
    pub(super) marks: DbMarks,
    pub(super) approval: AgentApproval,
}

impl AgentBinding {
    /// 이 판정으로 쓰는 호출마다 승인을 청한다(프로덕션 대상 · `production = "ask"`).
    pub fn asks_approval(&self) -> bool {
        self.approval == AgentApproval::Required
    }
}

/// 증거가 청하는 승인: 프로덕션 `ask`(16-m5-plan M5-18 ④ — 「프로덕션 태그 연결의 판정」 표)와 「이 연결 열기」(M5-17 ③).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AgentApproval {
    /// 사람이 연 연결(또는 이 세션이 승인받은 연결)이고 프로덕션 대상이 아니다 — 승인 없이 쓴다(쓰기 툴은 제 등급으로 따로
    /// 묻는다).
    NotNeeded,
    /// 프로덕션 대상이고 `production = "ask"` 다 — 이 호출의 승인([`DbGrant::approve`])을 받기 전에는 연결을 열지 않는다.
    Required,
    /// 사람이 연 적도 이 세션이 승인받은 적도 없는 연결이다(`db_connection_test` 의 입구만 — [`db_grant_to_open`]). 「이 연결 열기」
    /// 승인을 받기 전에는 열지 않는다.
    Open,
    /// 그 승인을 받았다(계획 해시).
    Given(String),
    /// 「이 연결 열기」 승인을 받았다(계획 해시) — 툴이 그 세션의 기록에 더한다([`DbGrant::remember_open_approval`]).
    GivenToOpen(String),
}

/// 그 프로젝트의 `[mcp.policy] production`(사람의 설정에서만 — 읽는 함수는 MCP 세션과 같은 [`mcp_policy::read_project_over`],
/// 메인 판정 2026-11-17). 전역 설정 스토어가 없는 Host(시험 등)는 프로젝트 설정만 본다. 경고는 로그에 남긴다. 프로젝트 설정의
/// 어느 레이어든 읽지 못하면 전역으로 떨어지지 않고 [`settings_unreadable`] 로 거절한다(fail-closed — 17-m6-plan M6-01 리뷰 1).
/// 블로킹이다.
pub(crate) fn production_policy(
    sessions: &DbSessions,
    project: &Project,
) -> Result<Production, RpcError> {
    let Ok(host) = sessions.connections.host() else {
        return Ok(Production::Block);
    };
    let global = sessions.connections.settings.store().map(|s| s.snapshot());
    let global_mcp = global
        .as_ref()
        .and_then(|g| g.get("settings", mcp_policy::MCP_KEY));
    let (policy, warnings) = mcp_policy::read_project_over(&host.paths, global_mcp, project)
        .map_err(|e| {
            tracing::warn!(target: "pw_db", project = %project.id, path = ?e.path, "cannot read the project's settings; refusing the agent's db call");
            settings_unreadable(&project.id)
        })?;
    for w in warnings {
        tracing::warn!(target: "pw_db", project = %project.id, "{w}");
    }
    Ok(policy.production)
}

/// `PERMISSION_DENIED`(`data.reason = "projectConfigUnreadable"`): 그 프로젝트의 설정 파일(정책 · DB 연결 — 어느 레이어든)을 읽지
/// 못해 표지 · 정책을 모른다. 에이전트의 DB 호출은 돌지 않는다(MCP 세션의 거절과 같은 까닭 — `pw_mcp::marker`). 문장에는 파일
/// 내용을 싣지 않는다.
pub(crate) fn settings_unreadable(project_id: &str) -> RpcError {
    RpcError::new(rpc_code::PERMISSION_DENIED, UNREADABLE_SETTINGS).with_data(json!({
        "reason": mcp_policy::PROJECT_CONFIG_UNREADABLE,
        "projectId": project_id,
    }))
}

/// `PERMISSION_DENIED`(`data.reason = "notOpenedByUser"`). 문장은 「이 연결 열기」 승인을 청하는 길(`db_connection_test`)도 알린다.
pub fn not_opened_by_user(project_id: &str, connection_id: &str) -> RpcError {
    not_opened(project_id, connection_id, true)
}

/// [`not_opened_by_user`]. `may_ask` 가 거짓이면(프로덕션 대상 — 승인으로도 열리지 않는다, 결정 M5-D21) 승인을 청하는 길을 알리지
/// 않는다.
fn not_opened(project_id: &str, connection_id: &str, may_ask: bool) -> RpcError {
    let how = if may_ask {
        "open it once in the project's DB explorer in Pitwall, or call `db_connection_test` to ask the person to approve \
         opening it for this agent session, then call again"
    } else {
        "it points at a target marked production, which only the person can open: open it once in the project's DB \
         explorer in Pitwall, then call again"
    };
    RpcError::new(
        rpc_code::PERMISSION_DENIED,
        format!(
            "db connection `{connection_id}` was not opened in Pitwall during this run (or its target changed since): {how}"
        ),
    )
    .with_data(json!({
        "reason": NOT_OPENED_BY_USER,
        "projectId": project_id,
        "connectionId": connection_id,
    }))
}

/// `PERMISSION_DENIED`(`data.reason = "productionBlocked"`).
pub fn production_blocked(project_id: &str, connection_id: &str) -> RpcError {
    RpcError::new(
        rpc_code::PERMISSION_DENIED,
        format!(
            "db connection `{connection_id}` points at a target marked production; agent tools do not use production connections"
        ),
    )
    .with_data(json!({
        "reason": PRODUCTION_BLOCKED,
        "projectId": project_id,
        "connectionId": connection_id,
    }))
}

/// `PERMISSION_DENIED`(`data.reason = "readOnlyConnection"`): 판정 뒤 그 대상에 read-only 표지가 더해졌다.
fn marked_read_only(project_id: &str, connection_id: &str) -> RpcError {
    RpcError::new(
        rpc_code::PERMISSION_DENIED,
        format!(
            "db connection `{connection_id}` was marked read-only after this call was checked; nothing ran — call again"
        ),
    )
    .with_data(json!({
        "reason": "readOnlyConnection",
        "projectId": project_id,
        "connectionId": connection_id,
    }))
}

/// 에이전트의 DB 툴 호출이 읽는 연결 설정: **그 저장소의 메인 체크아웃**의 것(표지를 더하려고 — 결정 M5-D37)과 호출의 워크트리의
/// 것. 메인 체크아웃은 그 프로젝트의 주 루트이고, 연결 워크트리를 따로 등록한 프로젝트면 신뢰가 따르는 메인 저장소 프로젝트
/// (`Registry::main_repository_of`)의 주 루트도 읽는다. 돌려주는 것은 호출의 워크트리의 것이다. 메인 체크아웃의 설정을 읽지 못하면
/// (깨진 파일 등) 그 표지는 더하지 못하고 넘어간다 — 앞서 읽힌 표지는 기억(상태 DB)에 남아 있다. 끝에 표지 기억을 상태 DB 와
/// 맞춘다([`MarkMemory::sync`]) — 맞추지 못하면 `markMemoryUnavailable` 로 거절한다(fail-closed). 블로킹이다.
pub(crate) fn agent_view(
    sessions: &DbSessions,
    project_id: &str,
    worktree: Option<&str>,
) -> Result<Loaded, RpcError> {
    let connections = &sessions.connections;
    let main_repository = connections
        .host()?
        .projects
        .main_repository_of(project_id)
        .ok()
        .flatten()
        .map(|p| p.id);
    let mains = worktree
        .is_some()
        .then(|| project_id.to_owned())
        .into_iter()
        .chain(main_repository);
    for main in mains {
        if let Err(e) = connections.load(&main, None) {
            tracing::info!(target: "pw_db", project = %main, %e, "cannot read the main checkout's db connections for agent marks");
        }
    }
    let loaded = connections.load(project_id, worktree)?;
    // 앞 실행들이 남긴 표지(상태 DB)를 더하고 이번 읽기의 표지를 쓴다 — 쓰지 못하면 거절(fail-closed, M5-17b).
    connections.marks.sync()?;
    Ok(loaded)
}

/// `data.reason`: 에이전트의 연결 쓰기가 표지(프로덕션 · read-only)를 내린다(메인 판정 2026-11-17 — M5-17 리뷰 B1).
pub const LOWERS_MARKS: &str = "lowersMarks";
/// `data.reason`: 에이전트의 연결 쓰기가 표지가 붙은 연결의 대상을 바꾼다(같은 판정).
pub const MOVES_MARKED_TARGET: &str = "movesMarkedTarget";

/// 에이전트의 연결 쓰기(`db_connection_upsert`)는 **표지를 조이기만 한다**(메인 판정 2026-11-17 — M5-17 리뷰 B1). 이 판정은 표지
/// 기억(결정 M5-D37)이 이번 앱 실행 동안만이던 때 다음 실행으로 비껴가지 못하게 생겼고, 기억이 앱 실행을 넘어 영속한 뒤(M5-17b)에도
/// 그대로 둔다 — 에이전트가 Pitwall 을 거쳐 파일과 기억이 다른 표지를 말하게 만들지 못하게. 묻지 않고 거절한다:
///
/// 1. 그 연결 id 의 지금 표지(이 워크트리 · 메인 체크아웃 · 메인 저장소 프로젝트의 병합 설정, 그리고 **각 레이어 파일**(공유 ·
///    개인 `.local.toml` · 메인 체크아웃 폴백)에 적힌 `production` · `read_only`, 그 대상들의 기억)를 새 설정이 끄거나 지운다
///    (`lowersMarks`). 레이어로 비껴가지 못한다 — 개인 파일에 `production = false` 를 써 공유 파일을 덮는 것도 같다.
/// 2. 표지가 붙은 연결의 푼 대상(호스트 · 포트 · DB · 사용자 · 터널 · 파일)을 바꾼다(`movesMarkedTarget`).
/// 3. 새 설정의 푼 대상에 기억된 표지(앞 실행들의 것 포함)가 있는데 새 설정이 그 표지를 달지 않는다(`lowersMarks`). 열쇠는 표지 기억의
///    것 그대로다(`DbTarget::mark_key` — MySQL · MariaDB · Redis 는 서버 · 사용자 단위).
///
/// 창의 `db.connection.upsert` 는 이 판정을 지나지 않는다 — 표지는 사람이 Pitwall 에서만 내린다. 블로킹이다.
pub(crate) fn check_agent_write(
    sessions: &DbSessions,
    project_id: &str,
    worktree: Option<&str>,
    connection_id: &str,
    new: &DbConnectionConfig,
) -> Result<(), RpcError> {
    let connections = &sessions.connections;
    // 이 워크트리의 설정(그리고 `agent_view` 가 메인 체크아웃 · 메인 저장소 프로젝트를 읽어 기억에 더한다).
    let loaded = agent_view(sessions, project_id, worktree)?;
    let new_target = DbTarget::of(new, &loaded.root);
    let main_repository = connections
        .host()?
        .projects
        .main_repository_of(project_id)
        .ok()
        .flatten()
        .map(|p| p.id);
    let mut views = vec![(project_id.to_owned(), worktree.map(str::to_owned))];
    if worktree.is_some() {
        views.push((project_id.to_owned(), None));
    }
    views.extend(main_repository.into_iter().map(|p| (p, None)));
    let mut existing = DbMarks::default();
    let mut targets: Vec<DbTarget> = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();
    for (project, wt) in &views {
        let view = if project == project_id && wt.as_deref() == worktree {
            None
        } else {
            connections.load(project, wt.as_deref()).ok()
        };
        let view = view.as_ref().unwrap_or(&loaded);
        if let Some((loaded_connection, config)) = view
            .get(connection_id)
            .and_then(|c| c.config.as_ref().ok().map(|config| (c, config)))
        {
            let target = DbTarget::of(config, &view.root);
            existing = existing
                .tighten(DbMarks::of_loaded(loaded_connection, config))
                .tighten(connections.marks.get(&target));
            targets.push(target);
        }
        files.extend(connections.connection_files(project, wt.as_deref(), connection_id)?);
    }
    files.push(crate::config::config_file(
        &loaded.local_dir,
        connection_id,
        crate::config::DbConfigLocation::Local,
    ));
    files.sort();
    files.dedup();
    for file in &files {
        existing = existing.tighten(layer_marks(file)?);
    }
    let lowers = |floor: DbMarks| {
        (floor.production && !new.production) || (floor.read_only && !new.read_only)
    };
    if lowers(existing) {
        tracing::info!(target: "pw_db", project = project_id, connection = connection_id, "agent db connection write refused: it lowers the connection's marks");
        return Err(write_refused(
            project_id,
            connection_id,
            LOWERS_MARKS,
            "would turn off its production or read-only mark; only the person can lower a mark, in Pitwall",
        ));
    }
    if existing != DbMarks::default() && !targets.contains(&new_target) {
        tracing::info!(target: "pw_db", project = project_id, connection = connection_id, "agent db connection write refused: it moves a marked connection");
        return Err(write_refused(
            project_id,
            connection_id,
            MOVES_MARKED_TARGET,
            "is marked production or read-only, and this write changes its target (host, port, database, user, SSH tunnel \
             or file); only the person can change it, in Pitwall",
        ));
    }
    if lowers(connections.marks.get(&new_target)) {
        tracing::info!(target: "pw_db", project = project_id, connection = connection_id, "agent db connection write refused: its target is marked");
        return Err(write_refused(
            project_id,
            connection_id,
            LOWERS_MARKS,
            "would point at a target marked production or read-only in Pitwall's memory without that mark; keep the mark \
             in the settings",
        ));
    }
    Ok(())
}

/// [`layer_marks`] 인데 읽지 못하면 두 표지를 다 단 것으로 본다(fail-closed — 설정 계층이 방금 읽은 파일이라 드물다).
pub(crate) fn layer_marks_or_strict(file: &Path) -> DbMarks {
    layer_marks(file).unwrap_or_else(|_| {
        tracing::warn!(target: "pw_db", path = %file.display(), "cannot read a db connection layer for its marks; treating it as marked");
        DbMarks {
            production: true,
            read_only: true,
        }
    })
}

/// 레이어 파일 하나에 적힌 표지(없는 파일은 표지 없음). 읽지 못하면 거절한다 — 표지를 모르는 채로 쓰지 않는다.
fn layer_marks(file: &Path) -> Result<DbMarks, RpcError> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(DbMarks::default()),
        Err(_) => return Err(RpcError::internal(UNREADABLE_SETTINGS)),
    };
    let doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|_| RpcError::internal(UNREADABLE_SETTINGS))?;
    let flag = |key: &str| doc.get(key).and_then(toml_edit::Item::as_bool) == Some(true);
    Ok(DbMarks {
        production: flag("production"),
        read_only: flag("read_only"),
    })
}

/// `PERMISSION_DENIED` — 에이전트의 연결 쓰기를 묻지 않고 거절한다.
fn write_refused(project_id: &str, connection_id: &str, reason: &str, why: &str) -> RpcError {
    RpcError::new(
        rpc_code::PERMISSION_DENIED,
        format!("db connection `{connection_id}` was not saved: this write {why}"),
    )
    .with_data(json!({
        "reason": reason,
        "projectId": project_id,
        "connectionId": connection_id,
    }))
}

/// 틀린 연결 설정의 까닭(serde 오류 글)은 툴 응답 · 감사 로그로 내보내지 않는다 — 사람이 실수로 적은 비밀 값이 그 글에 실린다.
pub(crate) fn invalid_settings(connection_id: &str) -> String {
    format!("db connection `{connection_id}` has invalid settings; open it in Pitwall to see why")
}

/// 프로젝트 설정을 읽지 못한 까닭도 같다(설정 계층의 오류 글은 파일 줄을 싣는다).
pub(crate) const UNREADABLE_SETTINGS: &str =
    "cannot read the project's settings files; open the project in Pitwall to see why";

/// 판정을 청하는 쪽(MCP 세션)과 무엇을 위해서인가.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Asker {
    /// MCP 세션(= Host 연결 하나). 「승인받은 연결」 의 열쇠다. 모르면(목록이 세션 없이 판정할 때) `None`.
    pub session: Option<SessionId>,
    /// 연 적 없는 연결이면 「이 연결 열기」 승인을 청하는 증거를 준다(`db_connection_test` 만 — 결정 M5-D21).
    pub may_ask_to_open: bool,
}

/// 연결 하나에 대한 에이전트의 판정(신뢰 게이트는 부른 쪽이 지났다). `database` 는 호출이 설정의 DB 대신 고른 DB — 그 대상의
/// 표지도 본다. `production` 은 그 프로젝트의 `[mcp.policy] production`([`production_policy`]): 프로덕션 대상이면 `block` 은
/// 거절(`productionBlocked`), `ask` 는 「사람이 연 연결」 판정을 그대로 지난 뒤 승인을 청하는 증거다(연 적 없으면 `ask` 여도
/// `notOpenedByUser` — 결정 M5-D21).
///
/// 「사람이 연 연결」 이 아니어도 그 세션이 「이 연결 열기」 승인을 받은 (연결 id, 푼 대상)이면 지난다(M5-17 — 「승인받은 연결」
/// 입구). 프로덕션 대상은 승인받은 연결로 보지 않는다 — 승인 뒤 표지가 더해졌어도(결정 M5-D37) 사람이 연 연결만 지난다. 둘 다
/// 아니면 `asker.may_ask_to_open`(`db_connection_test`)일 때만 「이 연결 열기」 승인을 청하는 증거를 주고, 아니면 묻지 않고
/// `notOpenedByUser` 다.
pub(crate) fn judge_for_agent(
    sessions: &DbSessions,
    loaded: &Loaded,
    connection: &LoadedConnection,
    database: Option<&str>,
    production: Production,
    asker: Asker,
) -> Result<AgentBinding, RpcError> {
    let project_id = loaded.project.id.as_str();
    let config = connection
        .config
        .as_ref()
        .map_err(|_| RpcError::invalid_params(invalid_settings(&connection.id)))?;
    let target = DbTarget::of(config, &loaded.root);
    let marks_memory = &sessions.connections.marks;
    let mut marks = DbMarks::of_loaded(connection, config).tighten(marks_memory.get(&target));
    if database.is_some_and(|db| config.database.as_deref() != Some(db)) {
        marks = marks.tighten(marks_memory.get(&target.with_database(database)));
    }
    if marks.production && production == Production::Block {
        tracing::info!(target: "pw_db", project = project_id, connection = %connection.id, "agent db access refused: production target");
        return Err(production_blocked(project_id, &connection.id));
    }
    let opened = sessions
        .opened
        .contains(project_id, &connection.id, &target);
    let approved = !marks.production
        && asker.session.is_some_and(|session| {
            sessions
                .approved
                .contains(session, project_id, &connection.id, &target)
        });
    let approval = if marks.production {
        if !opened {
            tracing::info!(target: "pw_db", project = project_id, connection = %connection.id, "agent db access refused: a production target the user has not opened");
            return Err(not_opened(project_id, &connection.id, false));
        }
        AgentApproval::Required
    } else if opened || approved {
        AgentApproval::NotNeeded
    } else if asker.may_ask_to_open {
        AgentApproval::Open
    } else {
        tracing::info!(target: "pw_db", project = project_id, connection = %connection.id, "agent db access refused: not opened by the user");
        return Err(not_opened_by_user(project_id, &connection.id));
    };
    Ok(AgentBinding {
        connection_id: connection.id.clone(),
        name: config.name.clone(),
        worktree: None,
        target,
        marks,
        approval,
    })
}

/// MCP 세션의 DB 입구(가칭 `db_grant_for_agent` 그대로). `agent` 는 `pw_core::registry::agent::agent_grant` 를 지난 증거,
/// `session` 은 호출이 온 MCP 세션(`pw_rpc::Ctx::session` — MCP 세션 하나가 Host 연결 하나다), `worktree` 는 호출의 활성 워크트리
/// 꼭대기(없으면 메인 체크아웃), `database` 는 호출이 설정의 DB 대신 고를 DB(없으면 설정의 것).
///
/// 신뢰 게이트(`untrusted`) → 연결 설정 읽기(메인 체크아웃 · 그 워크트리 — 표지를 더한다) → 프로덕션 표지(`[mcp.policy]
/// production` 이 `block` 이면 `productionBlocked`) → 「사람이 연 연결」 또는 그 세션의 「승인받은 연결」(M5-17 — 아니면 묻지 않고
/// `notOpenedByUser`). 프로덕션 대상을 `ask` 로 지난 증거는
/// [`DbGrant::needs_approval`] 이고, 툴은 계획을 만들어 승인([`DbGrant::approve`])을 받기 전에는 그 증거로 연결을 열 수 없다. 없는 연결은 not found, 틀린 설정은 invalid params. 돌려준 증거는 그 연결 id · 워크트리 ·
/// 대상에만 쓰인다 — 연결을 여는 자리가 그때 설정을 다시 읽어 견준다. 블로킹이다(설정 파일 · 레지스트리) — 블로킹 스레드에서 부른다.
pub fn db_grant_for_agent(
    sessions: &DbSessions,
    agent: &AgentGrant,
    session: SessionId,
    worktree: Option<&str>,
    connection_id: &str,
    database: Option<&str>,
) -> Result<DbGrant, RpcError> {
    grant_for_agent(
        sessions,
        agent,
        Asker {
            session: Some(session),
            may_ask_to_open: false,
        },
        worktree,
        connection_id,
        database,
    )
}

/// `db_connection_test` 의 입구(16-m5-plan M5-17 ③, 결정 M5-D21): [`db_grant_for_agent`] 와 같은 판정이되, 사람이 연 적도 그
/// 세션이 승인받은 적도 없는 (비프로덕션) 연결이면 거절하지 않고 「이 연결 열기」 승인을 청하는 증거를 준다
/// ([`DbGrant::needs_open_approval`]). 툴은 계획을 만들어 승인을 받고([`DbGrant::approve`]) 그 세션의 기록에 더한다
/// ([`DbGrant::remember_open_approval`]) — 그 전에는 이 증거로 연결을 열 수 없다. 프로덕션 대상은 묻지 않고 거절한다(`block` 이면
/// `productionBlocked`, `ask` 여도 사람이 연 적이 없으면 `notOpenedByUser`).
pub fn db_grant_to_open(
    sessions: &DbSessions,
    agent: &AgentGrant,
    session: SessionId,
    worktree: Option<&str>,
    connection_id: &str,
) -> Result<DbGrant, RpcError> {
    grant_for_agent(
        sessions,
        agent,
        Asker {
            session: Some(session),
            may_ask_to_open: true,
        },
        worktree,
        connection_id,
        None,
    )
}

fn grant_for_agent(
    sessions: &DbSessions,
    agent: &AgentGrant,
    asker: Asker,
    worktree: Option<&str>,
    connection_id: &str,
    database: Option<&str>,
) -> Result<DbGrant, RpcError> {
    check_connection_id(connection_id)?;
    let project_id = agent.project_id();
    sessions.connections.trust_gate(project_id, worktree)?;
    let loaded = agent_view(sessions, project_id, worktree)?;
    if loaded.connections.is_err() {
        return Err(settings_unreadable(project_id));
    }
    let connection = loaded
        .get(connection_id)
        .ok_or_else(|| crate::config::not_found(connection_id))?;
    let production = production_policy(sessions, agent.project())?;
    let mut binding = judge_for_agent(sessions, &loaded, connection, database, production, asker)?;
    binding.worktree = worktree.map(str::to_owned);
    Ok(DbGrant {
        project_id: project_id.to_owned(),
        agent: Some(Box::new(binding)),
    })
}

/// 승인 계획이 해시에 넣는 대상 값(16-m5-plan M5-12 ② — 연결의 푼 대상 · 표지. 방언은 부른 쪽이 따로 넣는다).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbPlanTarget<'a> {
    pub connection_id: &'a str,
    pub worktree: Option<&'a str>,
    pub target: &'a DbTarget,
    pub marks: DbMarks,
}

impl DbGrant {
    /// 프로덕션 대상을 `production = "ask"` 로 지난 에이전트의 증거다 — 툴은 이 호출의 계획을 만들어 승인을 받아야 한다
    /// ([`DbGrant::approve`]). 그 전에는 연결을 여는 자리 · 스키마 캐시가 이 증거를 받지 않는다.
    pub fn needs_approval(&self) -> bool {
        self.agent
            .as_deref()
            .is_some_and(|a| a.approval == AgentApproval::Required)
    }

    /// 「이 연결 열기」 승인을 청하는 증거다([`db_grant_to_open`] — 사람이 연 적도 그 세션이 승인받은 적도 없는 연결). 툴은 이
    /// 호출의 계획을 만들어 승인을 받아야 한다([`DbGrant::approve`]). 그 전에는 연결을 여는 자리가 이 증거를 받지 않는다.
    pub fn needs_open_approval(&self) -> bool {
        self.agent
            .as_deref()
            .is_some_and(|a| a.approval == AgentApproval::Open)
    }

    /// 계획의 승인([`Approved`] — `pw_core::registry::plan::approve` 밖에서는 만들 수 없다)을 이 증거에 단다.
    pub fn approve(&mut self, approved: &Approved) {
        if let Some(binding) = self.agent.as_deref_mut() {
            match binding.approval {
                AgentApproval::Required => {
                    binding.approval = AgentApproval::Given(approved.hash().to_owned());
                }
                AgentApproval::Open => {
                    binding.approval = AgentApproval::GivenToOpen(approved.hash().to_owned());
                }
                _ => {}
            }
        }
    }

    /// 「이 연결 열기」 승인을 받은 증거면 (그 세션, 프로젝트, 연결 id, 판정한 대상)을 「승인받은 연결」 로 기록한다 — 그 세션의
    /// 다른 DB 툴도 그 연결을 쓴다. 「사람이 연 연결」 기록에는 더하지 않는다. 기록했으면 그 세션이 끝날 때 지운다(`closed` —
    /// `pw_rpc::Ctx::connection_closed`). 승인받지 않은 증거면 아무것도 하지 않고 `false`.
    pub fn remember_open_approval(
        &self,
        sessions: &std::sync::Arc<DbSessions>,
        session: SessionId,
        closed: tokio_util::sync::CancellationToken,
    ) -> bool {
        let Some(binding) = self.agent.as_deref() else {
            return false;
        };
        if !matches!(binding.approval, AgentApproval::GivenToOpen(_)) {
            return false;
        }
        let first = sessions.approved.record(
            session,
            &self.project_id,
            &binding.connection_id,
            binding.target.clone(),
        );
        if first {
            let sessions = std::sync::Arc::clone(sessions);
            tokio::spawn(async move {
                closed.cancelled().await;
                sessions.approved.forget_session(session);
            });
        }
        true
    }

    /// 에이전트의 증거가 묶인 연결 이름. 창의 증거면 `None`.
    pub fn agent_connection_name(&self) -> Option<&str> {
        self.agent.as_deref().map(|a| a.name.as_str())
    }

    /// 승인 계획의 대상 값(에이전트의 증거만).
    pub fn plan_target(&self) -> Option<DbPlanTarget<'_>> {
        self.agent.as_deref().map(|a| DbPlanTarget {
            connection_id: &a.connection_id,
            worktree: a.worktree.as_deref(),
            target: &a.target,
            marks: a.marks,
        })
    }

    /// 연결을 여는 자리 · 캐시를 보는 자리에서: 에이전트의 증거면 그 연결 id · 워크트리 · 지금 설정의 대상이 판정한 것과 같아야 한다.
    /// 아니면 `notOpenedByUser`(그사이 설정이 바뀌었다). `marks` 는 그 자리에서 다시 읽은 설정의 표지에 그 대상의 기억을 조인 것이다 —
    /// 판정(과 승인 계획) 뒤에 표지가 더해졌으면 열지 않는다(프로덕션이면 `productionBlocked`, read-only 면 `readOnlyConnection`).
    /// 승인 뒤 · 연결 전에 설정을 바꿔 끼우는 것을 막는 자리다(16-m5-plan M5-18 — 승인 해시는 판정 때의 값이라 이 틈을 덮지 못한다).
    /// 창의 증거는 늘 지난다.
    pub(crate) fn check_agent_use(
        &self,
        worktree: Option<&str>,
        connection_id: Option<&str>,
        target: &DbTarget,
        marks: DbMarks,
    ) -> Result<(), RpcError> {
        let Some(binding) = self.agent.as_deref() else {
            return Ok(());
        };
        let id = connection_id.unwrap_or("(draft)");
        if binding.approval == AgentApproval::Required {
            // 툴이 승인을 지나지 않고 프로덕션 대상을 열려 했다(프로그래밍 오류) — 열지 않는다(fail-closed).
            tracing::error!(target: "pw_db", project = %self.project_id, connection = id, "agent db access refused: a production target without the call's approval");
            return Err(RpcError::internal(
                "this db call needs approval in Pitwall before it opens a production connection",
            ));
        }
        if binding.approval == AgentApproval::Open {
            // 툴이 「이 연결 열기」 승인을 지나지 않고 열려 했다(프로그래밍 오류) — 열지 않는다(fail-closed).
            tracing::error!(target: "pw_db", project = %self.project_id, connection = id, "agent db access refused: a connection nobody opened, without approval to open it");
            return Err(RpcError::internal(
                "this db call needs approval in Pitwall before it opens this connection",
            ));
        }
        if connection_id != Some(binding.connection_id.as_str())
            || worktree != binding.worktree.as_deref()
            || *target != binding.target
        {
            tracing::info!(target: "pw_db", project = %self.project_id, connection = id, "agent db access refused: the connection is not the one the gate judged");
            return Err(not_opened_by_user(&self.project_id, id));
        }
        if marks.production && !binding.marks.production {
            tracing::info!(target: "pw_db", project = %self.project_id, connection = id, "agent db access refused: marked production after the gate judged it");
            return Err(production_blocked(&self.project_id, id));
        }
        if marks.read_only && !binding.marks.read_only {
            tracing::info!(target: "pw_db", project = %self.project_id, connection = id, "agent db access refused: marked read-only after the gate judged it");
            return Err(marked_read_only(&self.project_id, id));
        }
        Ok(())
    }
}
