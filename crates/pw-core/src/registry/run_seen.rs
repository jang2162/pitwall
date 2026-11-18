//! 「사람이 본 실행 구성」 기록(17-m6-plan M6-11 — 결정 M6-D6, 06-roadmap D68). 05-mcp 「권한 모델」 의 `exec` 행 「신뢰된 프로젝트 +
//! 이미 존재하는 구성: 자동 / 방금 생성 · 수정된 구성: 승인」 의 「이미 존재하는 구성」 을 판정하는 자리다 — 사람이 화면에서
//! 만들거나 · 저장하거나 · 실행했거나, `run_start` 승인에서 [허용] 한 **그 내용**의 해시와 지금 내용의 해시가 같을 때만이다.
//!
//! - **자리**: `state.db` 의 `run_seen`(v4) — 프로젝트 id · 구성 열쇠 → 해시 · 기록한 자리 · 시각. 에이전트의 파일 쓰기 도구가 닿는
//!   평문 파일에 두지 않는다(DB 표지 기억 [`super::db_marks`] 와 같은 자리, 03 「데이터 저장」). 열쇠 하나에 마지막으로 본 해시
//!   하나다 — 사람이 고쳐 저장한 뒤 에이전트가 옛 내용으로 되돌리면 다시 묻는다.
//! - **해시만 든다**: 기록하는 값은 [`SeenHash`] 뿐이고 그것은 [`SeenContent::hash`] 나 [`SeenHash::parse`](꼴 검사)로만 생긴다 —
//!   구성의 값(env 값 · 명령줄)이 이 표 · 로그에 들어갈 길이 없다. [`SeenContent`] 의 `Debug` 도 값을 보이지 않는다.
//! - **쓰는 쪽은 사람의 창 연결뿐이다**: [`Registry::record_run_seen`] 은 [`SeenBy`] 를 받고, 그것은 그 프로젝트 창의 연결 범위
//!   (`ConnectionScope::Project(그 id)` — [`scope_may_write`])에서만 생긴다. 에이전트 범위(MCP 세션) · Host · 파일 창 · 다른
//!   프로젝트 창은 `PERMISSION_DENIED` 다. 기록하는 자리는 셋이다([`SeenPlace`]): 화면의 저장(M6-07 → `run.config.save`), 화면의
//!   실행(M6-02 의 창 경로), `run_start` 승인 다이얼로그의 [허용](M6-14 — 답 `mcp.approval.answer` 는 그 프로젝트 창의 연결로
//!   온다). `run_config_upsert` 의 승인은 기록하지 않는다(M6-13 은 이것을 부르지 않는다).
//! - **판정**: [`Registry::human_saw`] — 신뢰한 프로젝트이고, 저장소를 쓸 수 있고(`readOnly` · `memory` 면 기록이 없다고 본다 —
//!   기록을 더할 수 없는 저장소에서 「본 것」 을 믿지 않는다), 기록의 해시가 지금 해시와 같을 때만 `true`. 실행 툴(M6-14)은 이것을
//!   `PlanRules::policy_auto` 의 재료로 쓴다. 복합 구성 · `before`(M6-21)는 단계마다 부르고 모두가 `true` 일 때만 자동이다.
//!
//! 해시에 무엇이 드는가([`SeenContent`])는 `pw_run::seen` 이 구성에서 채운다 — 이 모듈은 그 값을 해시하고 저장한다.

use std::collections::BTreeMap;
use std::fmt;

use pw_rpc::{ConnectionScope, RpcError, code};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::state::scope_may_write;
use super::{Registry, RegistryError, now_ms};

/// 구성 열쇠의 최대 길이(바이트). 구성 id 는 64 자이고 감지된 태스크(`detected:<감지기>:<이름>`)의 이름은 저장소가 정한다.
pub const MAX_KEY_LEN: usize = 1024;

/// 해시 글의 머리.
const HASH_PREFIX: &str = "sha256:";

/// 「사람이 봤다」 를 기록한 자리.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeenPlace {
    /// 사람이 화면에서 만들거나 고쳐 저장했다(M6-07 → `run.config.save`).
    Save,
    /// 사람이 화면에서 실행했다(M6-02 의 창 경로 — `run.start`).
    Run,
    /// 사람이 에이전트의 `run_start` 승인에서 [허용] 했다(M6-14).
    Approval,
}

impl SeenPlace {
    /// `run_seen.seen_by` 의 값.
    pub fn as_str(self) -> &'static str {
        match self {
            SeenPlace::Save => "save",
            SeenPlace::Run => "run",
            SeenPlace::Approval => "approval",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "save" => SeenPlace::Save,
            "run" => SeenPlace::Run,
            "approval" => SeenPlace::Approval,
            _ => return None,
        })
    }
}

/// 사람의 창 연결에서 기록한다는 증거. [`SeenBy::window`] 밖에서는 만들 수 없다 — `Default` · `Deserialize` · `Clone` · 공개
/// 생성자를 더하지 않는다(더하면 에이전트 범위의 핸들러가 기록할 길이 생긴다).
#[derive(Debug)]
pub struct SeenBy {
    project_id: String,
    place: SeenPlace,
}

impl SeenBy {
    /// 그 프로젝트 창의 연결(`ConnectionScope::Project(project_id)`)만 받는다. 그 밖(에이전트 · Host · 파일 창 · 다른 프로젝트
    /// 창)은 `PERMISSION_DENIED` 다.
    pub fn window(
        scope: &ConnectionScope,
        project_id: &str,
        place: SeenPlace,
    ) -> Result<Self, RpcError> {
        if scope_may_write(scope, project_id) {
            Ok(Self {
                project_id: project_id.to_owned(),
                place,
            })
        } else {
            Err(RpcError::new(
                code::PERMISSION_DENIED,
                "only the project's own window records a run configuration a person saw",
            )
            .with_data(serde_json::json!({ "projectId": project_id })))
        }
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn place(&self) -> SeenPlace {
        self.place
    }
}

/// 내용 해시(`sha256:<64 hex>`). [`SeenContent::hash`] 가 만들고, 계획 · 승인 요청에 실려 온 글은 [`SeenHash::parse`] 로 되살린다
/// — 꼴이 맞지 않는 글(값 원문 등)은 받지 않는다.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SeenHash(String);

impl SeenHash {
    /// `sha256:` 와 소문자 16진 64 자. 아니면 `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.strip_prefix(HASH_PREFIX)?;
        (hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
            .then(|| Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SeenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 해시하는 내용(17-m6-plan M6-11 ①). 채우는 것은 `pw_run::seen` 이다 — 경로는 프로젝트 루트 기준이라 같은 내용이면 워크트리가
/// 달라도 같은 해시다.
///
/// 덮는 것은 「구성이 정한 명령줄과 그 명령 정의」 까지다(결정 M6-D6 ⓒ). 그 명령이 실행하는 코드(소스 · 빌드 스크립트 ·
/// `Makefile` · `justfile` 이 부르는 스크립트)는 덮지 않는다.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeenContent {
    /// 구성 전체를 파싱한 값의 정규형(`pw_run::config::RunConfig::canonical` — 주석 · 공백 · 키 순서가 들지 않고 `before` ·
    /// `configs` · `mode` · `[debug]` 를 포함한다).
    pub config: String,
    /// 풀린 argv(프로필을 얹은 것). 프로젝트 루트 안의 절대 경로(래퍼 등)는 루트 기준으로 적는다.
    pub argv: Vec<String>,
    /// 작업 폴더(프로젝트 루트 기준, `/` 구분).
    pub cwd: String,
    /// 런타임 환경 위에 얹는 env 의 이름과 값(`env_file` → `env` → 프로필의 `env` 를 덮은 결과 — M6-03).
    pub env: BTreeMap<String, String>,
    /// `env_file` 의 경로(구성에 적힌 그대로)와 각 파일 내용의 sha256, 구성의 순서대로.
    pub env_files: Vec<EnvFileDigest>,
    /// 고른 프로필.
    pub profile: Option<String>,
    /// 명령 정의 본문: `npm` 은 `package.json` 의 그 script, `mise-task` 는 `mise tasks ls --json` 의 그 태스크 `run`, 감지된
    /// 태스크는 그 감지 출처의 본문. 정의가 따로 없는 종류는 `None`.
    pub definition: Option<String>,
}

/// `env_file` 하나: 구성에 적힌 경로와 그 내용의 sha256(파일이 없으면 `None`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvFileDigest {
    pub path: String,
    pub sha256: Option<String>,
}

impl EnvFileDigest {
    /// 읽은 내용(`None` = 파일이 없다)의 다이제스트.
    pub fn of(path: impl Into<String>, contents: Option<&[u8]>) -> Self {
        Self {
            path: path.into(),
            sha256: contents.map(|bytes| hex(&Sha256::digest(bytes))),
        }
    }
}

/// 해시 입력의 판. 덮는 것의 꼴을 바꾸면 올린다 — 옛 기록은 새 해시와 맞지 않아 한 번 다시 묻는다.
const CONTENT_VERSION: u32 = 1;

impl SeenContent {
    pub fn hash(&self) -> SeenHash {
        #[derive(Serialize)]
        struct Versioned<'a> {
            v: u32,
            content: &'a SeenContent,
        }
        // 구조체 필드 순서와 `BTreeMap` 의 순서가 고정이라 같은 내용은 같은 글이다(구성은 이미 정규형 글이다).
        let text = serde_json::to_string(&Versioned {
            v: CONTENT_VERSION,
            content: self,
        })
        .expect("seen content serializes to JSON");
        SeenHash(format!(
            "{HASH_PREFIX}{}",
            hex(&Sha256::digest(text.as_bytes()))
        ))
    }
}

/// 값(argv · env 값 · 정의 본문 · 구성 글)을 보이지 않는다 — 로그 · 오류에 원문을 남기지 않는다(M5 의 규약, M6-03 ③).
impl fmt::Debug for SeenContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SeenContent")
            .field("configLen", &self.config.len())
            .field("argc", &self.argv.len())
            .field("cwd", &self.cwd)
            .field("envKeys", &self.env.keys().collect::<Vec<_>>())
            .field("envFiles", &self.env_files)
            .field("profile", &self.profile)
            .field("definitionLen", &self.definition.as_ref().map(String::len))
            .finish()
    }
}

/// 기록 한 행.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSeen {
    pub hash: SeenHash,
    pub place: SeenPlace,
    /// 기록한 시각(유닉스 ms).
    pub seen_at: i64,
}

impl Registry {
    /// 사람이 본 내용을 기록한다(열쇠마다 마지막 하나 — 머리 주석). `entries` 는 (구성 열쇠, 해시)이고 복합 구성은 단계마다 하나다.
    /// 레지스트리에 없는 프로젝트면 [`RegistryError::NotFound`], 쓰지 못하는 저장소(`readOnly` · `memory`)면
    /// [`RegistryError::Unavailable`] — 부른 쪽은 실행 · 저장을 막지 않고 기록만 건너뛴다(다음 에이전트 실행이 한 번 묻는다).
    pub fn record_run_seen(
        &self,
        by: &SeenBy,
        entries: &[(String, SeenHash)],
    ) -> Result<(), RegistryError> {
        for (key, _) in entries {
            if key.is_empty() || key.len() > MAX_KEY_LEN {
                return Err(RegistryError::Db(format!(
                    "a run configuration key must be 1 to {MAX_KEY_LEN} bytes"
                )));
            }
        }
        self.ensure_writable()?;
        let now = now_ms();
        self.write(|tx| {
            let known = tx
                .query_row(
                    "SELECT 1 FROM projects WHERE id = ?1",
                    [&by.project_id],
                    |_| Ok(()),
                )
                .optional()?;
            if known.is_none() {
                return Err(RegistryError::NotFound(by.project_id.clone()));
            }
            for (key, hash) in entries {
                tx.execute(
                    "INSERT INTO run_seen (project_id, config_key, hash, seen_by, seen_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT (project_id, config_key) DO UPDATE SET
                         hash = excluded.hash, seen_by = excluded.seen_by, seen_at = excluded.seen_at",
                    params![by.project_id, key, hash.as_str(), by.place.as_str(), now],
                )?;
            }
            Ok(())
        })
    }

    /// 그 구성 열쇠의 기록. 없으면 `None`.
    pub fn run_seen(&self, project_id: &str, key: &str) -> Result<Option<RunSeen>, RegistryError> {
        self.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT hash, seen_by, seen_at FROM run_seen WHERE project_id = ?1 AND config_key = ?2",
                    [project_id, key],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .optional()?;
            let Some((hash, place, seen_at)) = row else {
                return Ok(None);
            };
            match (SeenHash::parse(&hash), SeenPlace::parse(&place)) {
                (Some(hash), Some(place)) => Ok(Some(RunSeen {
                    hash,
                    place,
                    seen_at,
                })),
                _ => Err(RegistryError::Db(format!(
                    "run_seen row for `{key}` is not readable by this app"
                ))),
            }
        })
    }

    /// 정책 자동(M6-14 ③)의 재료: 신뢰한 프로젝트이고, 저장소를 쓸 수 있고, 그 열쇠의 기록이 이 해시인가. 셋 중 하나라도 아니면
    /// (읽기 오류 포함) `false` — 묻는다(fail-closed).
    pub fn human_saw(&self, project_id: &str, key: &str, hash: &SeenHash) -> bool {
        if self.ensure_writable().is_err() || !self.is_trusted(project_id) {
            return false;
        }
        matches!(self.run_seen(project_id, key), Ok(Some(seen)) if seen.hash == *hash)
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}
