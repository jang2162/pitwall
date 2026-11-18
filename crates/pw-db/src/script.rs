//! 에이전트의 SQL 실행 파이프라인 — `db_query`(16-m5-plan.md M5-07)와 `db_execute`(M5-18)가 함께 쓴다(「공유 부품의 주인」).
//! `docs/design/05-mcp.md` 「DB」 표와 그 아래 「`db_query` · `db_execute` 의 `file`」.
//!
//! 1. **출처**([`ScriptInput`]) — `sql`(글) 또는 `file`(SQL 파일 경로) 하나. 둘 다 있거나 둘 다 없으면 invalid params.
//! 2. **`file` 읽기**([`read_script_file`]) — 프로젝트 루트 안의 파일만. 상대 경로는 프로젝트 루트 기준이고(부른 프로세스의 cwd 가
//!    아니다) 워크트리가 정해진 호출이면 그 워크트리 안의 프로젝트 루트다(`pw_core::registry::path::worktree_root`). 판정은
//!    `fs.read` 와 같은 함수다 — `pw_fs::scope::Roots::readable`(심링크를 끝까지 푼 실제 경로가 루트 안), 일반 파일만, 연 핸들을
//!    다시 판정(`Roots::open_inside`). UTF-8, [`MAX_FILE_BYTES`](5 MiB — 06-roadmap.md 결정 D33)까지. **Host 가 실행하는 그때 한
//!    번 읽고**, 읽은 그 글을 분류하고 그 글을 실행한다 — 파일을 다시 열지 않는다.
//! 3. **분류** — 연결의 드라이버 방언으로 나누고 문장마다 매긴다(`crate::gate::plan` — 분류기는 Host 의 것 하나, 결정 D45). 조회만
//!    받는 툴은 [`refuse_non_queries`] 로 조회가 아닌 문장이 하나라도 있으면 아무것도 돌리지 않고 거절한다. 판정은 `db.query.page`
//!    와 같은 `Planned::is_strict_query` 다(세션 설정 · 트랜잭션 제어 · 분류기가 못 읽은 읽기 · 파싱 실패 · `tooLarge` 는 조회가
//!    아니다 — SQLite 의 `ATTACH` 도 분류가 쓰기다).
//! 4. **실행**([`run_script`]) — 새 연결의 트랜잭션 하나에서 문장을 차례로 돌린다. 문장마다 결과 하나, 행 제한은 문장마다.
//!    문장 하나가 실패하면 거기서 멈추고 그때까지의 결과와 오류(문장 번호 · 줄)를 돌려준다.
//!    - **조회**([`ScriptMode::ReadOnly`] — `db_query`, read-only 연결의 `db_execute`): 드라이버의 읽기 전용 트랜잭션(`begin {
//!      read_only }` — PG `BEGIN READ ONLY` · MySQL `START TRANSACTION READ ONLY` · SQLite `query_only`). 끝나면 롤백한다.
//!    - **쓰기**([`ScriptMode::Write`] — `db_execute`, 16-m5-plan M5-18 ③ · 결정 M5-D35): 읽고 쓰는 트랜잭션 하나. 모두 성공하면
//!      커밋하고, 문장 하나가 실패하면 롤백한다(부른 쪽 — [`run_script`] 는 커밋하지 않는다). 암묵 커밋 문장(MySQL 의 DDL)은
//!      트랜잭션으로 되돌릴 수 없다 — 계획이 「되돌릴 수 없다」 를 보인다. 트랜잭션 안에서 못 도는 문장(PG `CREATE INDEX
//!      CONCURRENTLY` · `VACUUM` 등)은 DB 의 오류가 그대로 그 문장의 오류다. 응답 예산을 다 써도 뒤 문장을 **멈추지 않는다** — 반만
//!      돌린 쓰기를 커밋하지 않게, 남은 문장은 돌리되 결과를 싣지 않고 수와 영향 행 수만 센다([`DbScriptUnlisted`]).
//!
//!    끝나면 연결을 닫는다.
//! 5. **취소** — 실행은 연결을 쥔 태스크에서 돈다. 호출(핸들러 future)이 버려지면(MCP 취소 → `request.cancel`, 세션 끊김 —
//!    05-mcp.md 「호출 취소」) [`CancelOnDrop`] 이 그 연결의 인터럽트를 보내고(`QueryCancel`), 태스크는 드라이버 호출을 버린 뒤
//!    롤백하고 연결을 닫는다.
//! 6. **값 · 크기** — 값은 RPC 타입(`DbValue`, 결정 D46 — 큰 정수 · decimal 은 문자열). 셀은 [`TOOL_CELL_BYTES`](4 KiB)에서 잘리고
//!    `truncated` 값으로 표지가 남는다. 결과 전체가 [`RESPONSE_BUDGET`] 을 넘으면 뒤쪽 행부터 덜고 그 문장의 `truncated` 를 켠다
//!    (결정 M5-D34). 다음 쪽은 없다 — 다시 부르면 다른 스냅숏이다.
//! 7. **감사 상세**([`audit_detail`]) — 감사 로그의 인자 요약은 글자열을 80자에서 자르므로 SQL 원문이 남지 않는다. 대신 문장 수 ·
//!    가장 높은 등급 · 문장 해시(`file` 이면 경로와 내용 해시)를 결과 · 오류 `data` 의 최상위 `audit` 글로 싣는다 — 세션이 그것을
//!    감사 행의 상세 칸으로 옮긴다(`crates/pw-mcp` 머리 주석 「툴 메서드 규약」). SQL 원문 · 값 · 비밀은 넣지 않는다.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use pw_core::registry::Registry;
use pw_core::registry::methods::rpc_error;
use pw_core::registry::path::worktree_root;
use pw_db_driver::budget::{Inactivity, within};
use pw_db_driver::{Budget, QueryCancel, SqlConnection};
use pw_db_sql::split::SqlRange;
use pw_db_types::{
    BeginOptions, CellLimit, ColumnMeta, DbError, DbErrorKind, ExecuteRequest, FetchLimit,
    RiskGrade, Row, SqlDialect, TransactionState,
};
use pw_fs::scope::{Escaped, NotAFile, Roots};
use pw_rpc::{JsNumber, RpcError, code as rpc_code};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::gate::{DbStatementClass, Planned, grade_of, statements_hash};

/// `file` 의 크기 상한 — 편집 상한(06-roadmap.md 결정 D33, `pw_fs::limits::EDITABLE_MAX`)과 같다.
pub const MAX_FILE_BYTES: u64 = pw_fs::limits::EDITABLE_MAX;
/// 셀 하나의 상한(결정 M5-D34). 넘는 값은 앞부분과 원래 크기(`DbValue::Truncated`)로 온다.
pub const TOOL_CELL_BYTES: usize = 4 * 1024;
/// 문장마다 돌려주는 행의 기본 상한(결정 M5-D34). 상한은 `crate::execute::MAX_PAGE_ROWS`.
pub const DEFAULT_MAX_ROWS: u32 = pw_rpc::DEFAULT_LIST_LIMIT;
/// 툴 결과 JSON 의 예산(바이트). 세션의 응답 상한(`pw_mcp::server::MAX_RESPONSE_BYTES`, 256 KiB)에 열 · 봉투 · 이스케이프의 여유를
/// 남긴 값이다 — 데스크톱 크레이트의 시험이 상한보다 작은지 본다.
pub const RESPONSE_BUDGET: usize = 192 * 1024;

/// 한 번에 읽는 몫.
const FETCH: FetchLimit = FetchLimit {
    max_rows: 1000,
    max_bytes: 512 * 1024,
};
const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// 툴 입력의 SQL 출처.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptInput {
    Sql(String),
    /// 프로젝트 루트 기준 경로(절대 경로면 그대로 — 루트 안이어야 한다).
    File(String),
}

impl ScriptInput {
    /// `sql` 과 `file` 가운데 꼭 하나.
    pub fn of(sql: Option<String>, file: Option<String>) -> Result<Self, RpcError> {
        match (sql, file) {
            (Some(sql), None) => Ok(Self::Sql(sql)),
            (None, Some(file)) => Ok(Self::File(file)),
            (Some(_), Some(_)) => Err(RpcError::invalid_params(
                "give either `sql` or `file`, not both",
            )),
            (None, None) => Err(RpcError::invalid_params("give `sql` or `file`")),
        }
    }
}

/// 읽은 SQL 파일.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptFile {
    /// 루트 기준 경로(결과 · 감사 상세에 싣는다). 실제 경로가 루트 밖으로 보이지 않는다.
    pub path: String,
    /// 정규화한 실제 경로.
    pub real: PathBuf,
    /// 읽은 바이트(BOM 포함)의 SHA-256(hex).
    pub content_hash: String,
    /// 읽은 글(UTF-8 BOM 을 뗀 것). 분류 · 실행이 이것만 쓴다.
    pub text: String,
}

/// `file` 을 읽는다(머리 주석 ②). 루트는 `worktree` 가 있으면 그 워크트리 안의 프로젝트 루트, 없으면 프로젝트의 주 루트다.
/// 블로킹 — 블로킹 스레드에서 부른다.
pub fn read_script_file(
    projects: &Registry,
    project_id: &str,
    worktree: Option<&str>,
    path: &str,
) -> Result<ScriptFile, RpcError> {
    let project = projects.get(project_id).map_err(rpc_error)?;
    let root = match worktree {
        Some(top) => worktree_root(project.root(), Path::new(top)).map_err(|_| {
            RpcError::invalid_params(format!(
                "`{top}` is not a worktree of project `{project_id}`"
            ))
        })?,
        None => project.root().to_path_buf(),
    };
    let roots = Roots::confirmed([root.clone()]);
    let given = Path::new(path);
    let requested = if given.is_absolute() {
        given.to_path_buf()
    } else {
        root.join(given)
    };
    let target = roots
        .readable(&requested)
        .map_err(|d| RpcError::invalid_params(format!("file: {d}")))?;
    let real = target.real;
    // `fs.read` 처럼 일반 파일만 연다 — FIFO 는 여는 것부터 막히고 장치는 여는 것만으로 일이 생길 수 있다. 링크를 따라가지 않고
    // 본다(`real` 은 링크를 다 푼 경로). 그 뒤에 바뀌는 것은 `open_inside` 가 다시 본다.
    let meta = fs::symlink_metadata(&real).map_err(|e| io_error(&real, e))?;
    if meta.is_dir() {
        return Err(RpcError::invalid_params(format!(
            "file: `{}` is a folder",
            real.display()
        )));
    }
    if !meta.is_file() {
        return Err(RpcError::invalid_params(format!(
            "file: {}",
            NotAFile(real.clone())
        )));
    }
    let too_large = || {
        RpcError::invalid_params(format!(
            "file: `{}` is larger than {MAX_FILE_BYTES} bytes",
            real.display()
        ))
    };
    if meta.len() > MAX_FILE_BYTES {
        return Err(too_large());
    }
    let snap = pw_fs::read::read_whole(&real, &|p| roots.open_inside(p))
        .map_err(|e| io_error(&real, e))?;
    // 판정과 읽기 사이에 자랐을 수 있다.
    if snap.bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(too_large());
    }
    let body = snap.bytes.strip_prefix(UTF8_BOM).unwrap_or(&snap.bytes);
    let text = std::str::from_utf8(body)
        .map_err(|_| {
            RpcError::invalid_params(format!("file: `{}` is not UTF-8 text", real.display()))
        })?
        .to_owned();
    let shown = real
        .strip_prefix(&root)
        .map_or_else(|_| real.display().to_string(), |p| p.display().to_string());
    Ok(ScriptFile {
        path: shown,
        content_hash: hex(&Sha256::digest(&snap.bytes)),
        real,
        text,
    })
}

fn io_error(path: &Path, e: io::Error) -> RpcError {
    if let Some(inner) = e.get_ref() {
        if let Some(escaped) = inner.downcast_ref::<Escaped>() {
            return RpcError::invalid_params(format!("file: {escaped}"));
        }
        if let Some(not_a_file) = inner.downcast_ref::<NotAFile>() {
            return RpcError::invalid_params(format!("file: {not_a_file}"));
        }
    }
    match e.kind() {
        io::ErrorKind::NotFound => RpcError::new(
            rpc_code::NOT_FOUND,
            format!("file: `{}` does not exist", path.display()),
        ),
        _ => RpcError::invalid_params(format!("file: `{}`: {e}", path.display())),
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// 등급의 이름(RPC 의 꼴 — `read` · `write` · `unboundedWrite` · `destructive`).
fn grade_name(grade: RiskGrade) -> String {
    serde_json::to_value(grade)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{grade:?}"))
}

/// 감사 상세(머리 주석 ⑦): `statements=<수> grade=<가장 높은 등급> sql=sha256:<문장 해시>`, `file` 이면 그 뒤에
/// `file=<루트 기준 경로> content=sha256:<내용 해시>`. SQL 원문 · 값을 싣지 않는다. 문장 해시는 방언 · 문장 원문의 해시다
/// (`crate::gate::statements_hash` — 세션에 묶지 않는다).
pub(crate) fn audit_detail(
    dialect: SqlDialect,
    planned: &[Planned],
    file: Option<&ScriptFile>,
) -> String {
    let mut detail = format!(
        "statements={} grade={} sql=sha256:{}",
        planned.len(),
        grade_name(grade_of(planned)),
        statements_hash(None, dialect, planned, &[])
    );
    if let Some(file) = file {
        detail.push_str(&format!(
            " file={} content=sha256:{}",
            file.path, file.content_hash
        ));
    }
    detail
}

/// 조회만 받는 툴의 판정(머리 주석 ③): 조회가 아닌 문장이 하나라도 있으면 `PERMISSION_DENIED`(`data.reason = "notAQuery"`,
/// `data.statements` 는 그 문장들 — 번호 · 범위 · 등급 · 까닭, `data.audit` 는 감사 상세). 글에 첫 문장의 번호 · 줄 · 등급과
/// 「쓰기는 `db_execute`」 를 적는다.
pub(crate) fn refuse_non_queries(
    tool: &str,
    source: &str,
    planned: &[Planned],
    audit: &str,
) -> Result<(), RpcError> {
    let refused: Vec<DbStatementClass> = planned
        .iter()
        .filter(|s| !s.is_strict_query())
        .map(Planned::strict_class)
        .collect();
    let Some(first) = refused.first() else {
        return Ok(());
    };
    let message = format!(
        "{tool} runs queries only; nothing ran: statement {} (line {}) is graded `{}`{} — run writes with db_execute",
        first.index,
        line_of(source, first.range),
        grade_name(first.grade),
        if refused.len() > 1 {
            format!(" ({} statements are not queries)", refused.len())
        } else {
            String::new()
        },
    );
    Err(
        RpcError::new(rpc_code::PERMISSION_DENIED, message).with_data(json!({
            "reason": "notAQuery",
            "statements": refused,
            "audit": audit,
        })),
    )
}

/// 범위의 시작 줄(1 부터). `range` 는 `source` 기준 UTF-16 오프셋이다. 글을 한 번 걷는다 — 문장마다 줄을 구하면 [`LineIndex`].
pub(crate) fn line_of(source: &str, range: SqlRange) -> u32 {
    LineIndex::new(source).line(range)
}

/// 글의 줄 시작 표(둘째 줄부터의 UTF-16 오프셋). 문장마다 [`line_of`] 로 글 처음부터 세면 문장 수의 제곱이 든다 — 2 000 문장이
/// debug 에서 1 초를 넘었다. 한 번 만들고 문장마다 이진 탐색한다.
pub(crate) struct LineIndex(Vec<u32>);

impl LineIndex {
    pub(crate) fn new(source: &str) -> Self {
        let mut starts = Vec::new();
        let mut utf16 = 0u32;
        for c in source.chars() {
            utf16 += c.len_utf16() as u32;
            if c == '\n' {
                starts.push(utf16);
            }
        }
        Self(starts)
    }

    /// 범위의 시작 줄(1 부터) — 그 앞에서 끝난 줄바꿈의 수 + 1.
    pub(crate) fn line(&self, range: SqlRange) -> u32 {
        1 + self.0.partition_point(|&start| start <= range.from) as u32
    }
}

/// The result of one statement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbScriptResult {
    /// Which of the split statements it is (from 0 — the order of `db.sql.split`).
    pub index: u32,
    /// Line the statement starts on (from 1 — in `sql` or the file).
    pub line: u32,
    pub grade: RiskGrade,
    /// Columns of a statement that returns rows. Empty for a statement without rows.
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Row>,
    /// Number of rows changed (for a statement without rows — if the driver reports it).
    #[specta(type = Option<JsNumber>)]
    pub rows_affected: Option<u64>,
    /// Cut at the row limit (`maxRows`) or the response size — there was more. There is no next page (narrow the query
    /// and call again).
    pub truncated: bool,
    #[specta(type = JsNumber)]
    pub elapsed_ms: u64,
}

/// The statement that failed. The statements after it did not run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbScriptError {
    /// Which statement it is (from 0).
    pub index: u32,
    /// Line the statement starts on (from 1).
    pub line: u32,
    /// Range of the statement (UTF-16, in the `sql` or file text).
    pub range: SqlRange,
    pub error: DbError,
}

/// 실행의 결과([`run_script`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Ran {
    pub results: Vec<DbScriptResult>,
    pub error: Option<DbScriptError>,
    pub not_run: Option<DbScriptNotRun>,
    /// 쓰기 모드에서 응답 예산을 다 쓴 뒤에 돈 문장들.
    pub unlisted: Option<DbScriptUnlisted>,
    /// [`Statements::stop`] 으로 멈췄다(뒤 문장을 돌리지 않았다).
    pub stopped: bool,
}

/// 트랜잭션의 꼴(머리 주석 ④).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScriptMode {
    /// 읽기 전용 트랜잭션. 응답 예산을 다 쓰면 뒤 문장을 돌리지 않는다(`not_run`).
    ReadOnly,
    /// 읽고 쓰는 트랜잭션. 응답 예산을 다 써도 끝까지 돌리고 뒤 문장의 결과는 싣지 않는다(`unlisted`).
    Write,
}

/// Statements that ran but whose results are not listed because the response budget was used up (write calls only —
/// `count` statements from `index` on). They ran inside the same transaction as the listed ones.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbScriptUnlisted {
    /// Index of the first statement not listed (from 0).
    pub index: u32,
    /// Line the statement starts on (from 1).
    pub line: u32,
    /// Number of statements that ran without being listed.
    pub count: u32,
    /// Sum of the rows they changed (as far as the driver reported it).
    #[specta(type = JsNumber)]
    pub rows_affected: u64,
}

/// Statements not run because of the response budget (header comment ⑥): `count` statements from `index` to the end.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbScriptNotRun {
    /// Index of the first statement not run (from 0).
    pub index: u32,
    /// Line the statement starts on (from 1).
    pub line: u32,
    /// Number of statements not run (from `index` to the end).
    pub count: u32,
    pub reason: DbScriptNotRunReason,
}

/// Why the statements were not run.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbScriptNotRunReason {
    /// The rows of the earlier statements used up the response budget ([`RESPONSE_BUDGET`]).
    ResponseBudget,
}

/// 응답 예산의 셈(머리 주석 ⑥): 행을 읽는 그때 그 행의 JSON 크기를 더한다. 넘는 순간 그 문장을 끊고 뒤 문장을 돌리지 않는다 —
/// 행을 다 모은 뒤 자르면 넓은 행 × 많은 행이 그만큼 메모리에 쌓인다(M5-07 리뷰 B2).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ResponseRoom {
    used: usize,
    limit: usize,
}

impl ResponseRoom {
    /// `fixed` 는 행 · 문장 결과를 뺀 응답의 크기(감사 상세 · 파일 경로 · 오류 몫의 여유 포함).
    pub fn new(fixed: usize) -> Self {
        Self {
            used: fixed,
            limit: RESPONSE_BUDGET,
        }
    }

    fn left(&self) -> usize {
        self.limit.saturating_sub(self.used)
    }

    /// 남은 몫이 없는 예산(쓰기 모드에서 예산을 다 쓴 뒤의 문장 — 결과를 싣지 않고 돌린다).
    fn spent() -> Self {
        Self {
            used: RESPONSE_BUDGET,
            limit: RESPONSE_BUDGET,
        }
    }

    /// `size` 바이트를 쓸 수 있으면 쓴다.
    fn take(&mut self, size: usize) -> bool {
        if size > self.left() {
            return false;
        }
        self.used += size;
        true
    }
}

/// 행 하나가 응답에서 차지하는 크기(JSON 과 구분 쉼표).
fn row_size(row: &Row) -> usize {
    serde_json::to_string(row).map_or(usize::MAX, |s| s.len() + 1)
}

/// 문장 결과 하나의 행을 뺀 크기(소요 · 영향 행 수 자리의 여유 포함).
fn result_overhead(columns: &[ColumnMeta], index: u32, grade: RiskGrade) -> usize {
    let empty = DbScriptResult {
        index,
        line: u32::MAX,
        grade,
        columns: columns.to_vec(),
        rows: Vec::new(),
        rows_affected: Some(u64::MAX),
        truncated: false,
        elapsed_ms: u64::MAX,
    };
    serde_json::to_string(&empty).map_or(usize::MAX, |s| s.len() + 1)
}

/// 핸들러 future 가 버려지면(호출 취소 · 세션 끊김) 실행을 멈춘다 — 그 연결의 인터럽트를 보낸다. 실행 태스크는 드라이버 호출을
/// 버리고 롤백한 뒤 연결을 닫는다. 끝까지 기다린 호출은 [`CancelOnDrop::disarm`] 한다.
///
/// **커밋의 경계**(`db_execute` — 16-m5-plan M5-18, 메인 판정 2026-11-17): 실행 태스크는 커밋하기 전에 [`CommitPoint::begin_commit`]
/// 을 지난다. 그 전에 버려졌으면(취소가 관측됐으면) 커밋하지 않고 롤백한다. 지난 뒤에 버려지면 인터럽트를 보내지 않는다 — 커밋을
/// 취소로 끊지 않는다. RPC 의 취소는 그 앞에서 세운 `pw_rpc::PointOfNoReturn` 이 막아 호출이 버려지지 않으므로, 지난 뒤에 버려지는
/// 것은 런타임이 끝나며 버려진 호출뿐이다(태스크가 경고 로그를 남긴다).
pub(crate) struct CancelOnDrop {
    cancel: Option<QueryCancel>,
    phase: Arc<Mutex<CallPhase>>,
}

/// 호출과 실행 태스크가 함께 보는 단계.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallPhase {
    Running,
    /// 커밋을 정하기 전에 호출이 버려졌다.
    Cancelled,
    /// 커밋하기로 정했다(커밋을 보낸다).
    Committing,
    /// 커밋하기로 정한 뒤 호출이 버려졌다.
    CancelledAfterCommit,
}

/// 실행 태스크 쪽의 커밋 경계.
pub(crate) struct CommitPoint(Arc<Mutex<CallPhase>>);

impl CommitPoint {
    /// 커밋해도 되면 `true` — 그 뒤의 취소는 커밋을 끊지 않는다. 호출이 이미 버려졌으면 `false`(롤백한다).
    pub fn begin_commit(&self) -> bool {
        let mut phase = crate::session::lock(&self.0);
        if *phase == CallPhase::Running {
            *phase = CallPhase::Committing;
            true
        } else {
            false
        }
    }

    /// 커밋하기로 정한 뒤 호출이 버려졌다.
    pub fn cancelled_after_commit(&self) -> bool {
        *crate::session::lock(&self.0) == CallPhase::CancelledAfterCommit
    }
}

impl CancelOnDrop {
    pub fn new(cancel: QueryCancel) -> Self {
        Self {
            cancel: Some(cancel),
            phase: Arc::new(Mutex::new(CallPhase::Running)),
        }
    }

    /// 실행 태스크에 넘길 커밋 경계.
    pub fn commit_point(&self) -> CommitPoint {
        CommitPoint(Arc::clone(&self.phase))
    }

    pub fn disarm(mut self) {
        self.cancel = None;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let Some(cancel) = self.cancel.take() else {
            return;
        };
        {
            let mut phase = crate::session::lock(&self.phase);
            match *phase {
                CallPhase::Running => *phase = CallPhase::Cancelled,
                CallPhase::Committing => {
                    *phase = CallPhase::CancelledAfterCommit;
                    tracing::info!(target: "pw_db", "agent db call dropped after its commit point; the commit goes on");
                    return;
                }
                CallPhase::Cancelled | CallPhase::CancelledAfterCommit => return,
            }
        }
        tracing::info!(target: "pw_db", "agent db call dropped; interrupting its statement");
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    let _ = cancel.cancel().await;
                });
            }
            Err(_) => {
                tracing::warn!(target: "pw_db", "no runtime to interrupt a dropped agent db call")
            }
        }
    }
}

/// 읽기 전용 트랜잭션 하나에서 문장을 차례로 돌린다 — [`run_script`] 의 [`ScriptMode::ReadOnly`].
pub(crate) async fn run_read_only(
    conn: &mut dyn SqlConnection,
    source: &str,
    planned: &[Planned],
    max_rows: u32,
    cell: CellLimit,
    cancel: &QueryCancel,
    room: &mut ResponseRoom,
) -> Result<Ran, DbError> {
    run_script(
        conn,
        ScriptMode::ReadOnly,
        source,
        planned,
        max_rows,
        cell,
        cancel,
        room,
    )
    .await
}

/// 트랜잭션 하나를 열고 문장을 차례로 돌린다(머리 주석 ④). 트랜잭션을 열지 못하면 오류, 문장이 실패하면 그때까지의 결과와 그
/// 오류. 끝난 뒤의 커밋 · 롤백 · 닫기는 부른 쪽이 한다. 드라이버 호출마다 `cancel` 과 비활동 타임아웃(`Budget::PAGE` — 30 s)을
/// 건다. 드라이버에는 문장의 등급을 싣는다 — 조회가 아닌 문장(`INSERT … RETURNING` 등)의 커서를 버릴 때 드라이버가 끝까지 읽어
/// 버린다(멈추면 그 쓰기가 되돌려진다 — `ExecuteRequest::grade`).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_script(
    conn: &mut dyn SqlConnection,
    mode: ScriptMode,
    source: &str,
    planned: &[Planned],
    max_rows: u32,
    cell: CellLimit,
    cancel: &QueryCancel,
    room: &mut ResponseRoom,
) -> Result<Ran, DbError> {
    let mut inactivity = Inactivity::new(Budget::PAGE.inactivity);
    begin(conn, mode == ScriptMode::ReadOnly, cancel, &mut inactivity).await?;
    let mut options = Statements {
        max_rows,
        cell,
        in_transaction: true,
        count_rest: false,
        stop: &|| false,
    };
    Ok(run_statements(
        conn,
        mode,
        source,
        planned,
        &mut options,
        cancel,
        &mut inactivity,
        room,
        &mut |_| {},
    )
    .await)
}

/// 트랜잭션을 연다(비활동 타임아웃이면 서버의 문장을 멈춘다).
async fn begin(
    conn: &mut dyn SqlConnection,
    read_only: bool,
    cancel: &QueryCancel,
    inactivity: &mut Inactivity,
) -> Result<(), DbError> {
    inactivity.rearm();
    let begun = cancel
        .run(inactivity.guard(conn.begin(BeginOptions { read_only })))
        .await;
    interrupt_on_timeout(&begun, cancel).await;
    begun
}

/// [`run_statements`] 의 갈래.
pub(crate) struct Statements<'a> {
    pub max_rows: u32,
    pub cell: CellLimit,
    /// 트랜잭션 안에서 도는가. 아니면(실행 구성 `transaction = "none"` — M6-09) 문장마다 자동 커밋이고, 암묵 커밋 문장 뒤에
    /// 트랜잭션을 다시 열지 않는다.
    pub in_transaction: bool,
    /// `max_rows` 를 넘는 행을 버리되 끝까지 읽어 센다(실행 콘솔의 「조회 행 수」 — 앞 몇 행만 남긴다).
    pub count_rest: bool,
    /// 다음 문장을 시작하기 전에 본다 — 참이면 거기서 멈춘다(실행의 중지, M6-09).
    pub stop: &'a (dyn Fn() -> bool + Sync),
}

/// 문장 하나가 끝났다([`run_statements`] 의 관찰자 — 실행 콘솔이 문장마다 한 줄을 쓴다).
pub(crate) enum StatementEvent<'a> {
    Ran {
        result: &'a DbScriptResult,
        /// 행을 내는 문장이면 읽은 행 수 전체([`Statements::count_rest`] 일 때만).
        rows_total: Option<u64>,
    },
    /// 결과를 싣지 않고 돈 문장([`DbScriptUnlisted`]).
    RanUnlisted,
    Failed(&'a DbScriptError),
}

/// 열린 트랜잭션(또는 자동 커밋)에서 문장을 차례로 돌린다(머리 주석 ④). 문장이 실패하거나 [`Statements::stop`] 이 참이면 멈춘다.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_statements(
    conn: &mut dyn SqlConnection,
    mode: ScriptMode,
    source: &str,
    planned: &[Planned],
    options: &mut Statements<'_>,
    cancel: &QueryCancel,
    inactivity: &mut Inactivity,
    room: &mut ResponseRoom,
    observe: &mut (dyn FnMut(StatementEvent<'_>) + Send),
) -> Ran {
    let (max_rows, cell) = (options.max_rows, options.cell);
    let mut ran = Ran {
        results: Vec::new(),
        error: None,
        not_run: None,
        unlisted: None,
        stopped: false,
    };
    let mut spent = ResponseRoom::spent();
    let lines = LineIndex::new(source);
    for (at, statement) in planned.iter().enumerate() {
        if (options.stop)() {
            ran.stopped = true;
            break;
        }
        let started = Instant::now();
        let line = lines.line(statement.range);
        let listing = ran.unlisted.is_none();
        let budget = if listing { &mut *room } else { &mut spent };
        match run_one(
            conn,
            statement,
            max_rows,
            cell,
            options.count_rest,
            cancel,
            inactivity,
            budget,
        )
        .await
        {
            Ok(out) if listing => {
                let rows_total = out.rows_total;
                ran.results.push(DbScriptResult {
                    index: statement.index,
                    line,
                    grade: statement.grade,
                    columns: out.columns,
                    rows: out.rows,
                    rows_affected: out.rows_affected,
                    truncated: out.truncated,
                    elapsed_ms: elapsed_ms(started),
                });
                if let Some(result) = ran.results.last() {
                    observe(StatementEvent::Ran { result, rows_total });
                }
                if out.budget_spent
                    && let Some(next) = planned.get(at + 1)
                {
                    let count = u32::try_from(planned.len() - at - 1).unwrap_or(u32::MAX);
                    let line = lines.line(next.range);
                    match mode {
                        ScriptMode::ReadOnly => {
                            ran.not_run = Some(DbScriptNotRun {
                                index: next.index,
                                line,
                                count,
                                reason: DbScriptNotRunReason::ResponseBudget,
                            });
                            break;
                        }
                        // 반만 돈 쓰기를 남기지 않게 끝까지 돌리고, 결과는 싣지 않는다.
                        ScriptMode::Write => {
                            ran.unlisted = Some(DbScriptUnlisted {
                                index: next.index,
                                line,
                                count: 0,
                                rows_affected: 0,
                            });
                        }
                    }
                }
            }
            Ok(out) => {
                if let Some(unlisted) = ran.unlisted.as_mut() {
                    unlisted.count += 1;
                    unlisted.rows_affected = unlisted
                        .rows_affected
                        .saturating_add(out.rows_affected.unwrap_or(0));
                }
                observe(StatementEvent::RanUnlisted);
            }
            Err(error) => {
                ran.error = Some(DbScriptError {
                    index: statement.index,
                    line,
                    range: statement.range,
                    error,
                });
                if let Some(error) = &ran.error {
                    observe(StatementEvent::Failed(error));
                }
                break;
            }
        }
        // 암묵 커밋 문장(MySQL 의 DDL 등)이 트랜잭션을 끝냈으면 뒤 문장을 위해 다시 연다 — 앞의 것은 이미 커밋됐다(계획이 「되돌릴
        // 수 없다」 를 보였다). 뒤 문장은 다시 한 트랜잭션에 묶여, 실패하면 거기서부터 롤백된다.
        if mode == ScriptMode::Write
            && options.in_transaction
            && (statement.implicit_commit || statement.may_end)
            && at + 1 < planned.len()
            && conn.transaction_state() == TransactionState::Idle
        {
            if let Err(error) = begin(conn, false, cancel, inactivity).await {
                let next = &planned[at + 1];
                ran.error = Some(DbScriptError {
                    index: next.index,
                    line: lines.line(next.range),
                    range: next.range,
                    error,
                });
                if let Some(error) = &ran.error {
                    observe(StatementEvent::Failed(error));
                }
                break;
            }
        }
    }
    ran
}

/// 파일 여럿 · 트랜잭션 방식(17-m6-plan M6-09 — 실행 구성의 `sql` 종류). 방식은 구성의 것이다.
pub use pw_run::config::SqlTransaction;

/// 읽고 분류한 파일 하나([`run_files`] 의 입력).
#[derive(Debug, Clone)]
pub(crate) struct PlannedFile {
    pub file: ScriptFile,
    pub planned: Vec<Planned>,
}

/// 실행의 다스림(M6-09): 중지와 커밋 경계. 커밋을 시작한 뒤의 중지는 커밋을 끊지 않는다 — 커밋이 끝난 뒤 다음 파일 앞에서 멈춘다.
pub(crate) trait RunControl: Send + Sync {
    /// 중지가 왔다 — 다음 문장 · 다음 파일을 시작하지 않는다.
    fn stopped(&self) -> bool;
    /// 커밋해도 되면 `true`. 이미 중지가 왔으면 `false`(롤백한다).
    fn begin_commit(&self) -> bool;
    /// 커밋이 끝났다(성공이든 아니든).
    fn end_commit(&self);
}

/// How one transaction of a run ended (the same words as `db_execute`'s audit: committed · rolled back · unknown).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum SqlTransactionEnd {
    /// Committed.
    Committed,
    /// Rolled back: nothing of these files is kept (except what statements that commit on their own already committed).
    RolledBack,
    /// The commit was sent and its outcome is unknown (timeout · lost connection).
    Unknown,
    /// Only queries ran, in a read-only transaction that was rolled back.
    ReadOnly,
    /// No transaction (`transaction = "none"`): every statement that ran committed on its own.
    AutoCommitted,
}

impl SqlTransactionEnd {
    /// 감사 상세 · 콘솔의 낱말(`db_execute` 의 `transaction=` 과 같은 말).
    pub fn word(self) -> &'static str {
        match self {
            SqlTransactionEnd::Committed => "committed",
            SqlTransactionEnd::RolledBack => "rolledBack",
            SqlTransactionEnd::Unknown => "unknown",
            SqlTransactionEnd::ReadOnly => "readOnly",
            SqlTransactionEnd::AutoCommitted => "autoCommitted",
        }
    }
}

/// 트랜잭션 하나(파일 범위)와 그 끝.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FilesTransaction {
    pub files: std::ops::Range<usize>,
    pub end: SqlTransactionEnd,
    /// 커밋이 실패했으면 그 오류.
    pub commit_error: Option<DbError>,
}

/// [`run_files`] 의 관찰자가 받는 것.
pub(crate) enum FilesEvent<'a> {
    Statement {
        file: usize,
        event: StatementEvent<'a>,
    },
    Ended(&'a FilesTransaction),
}

/// [`run_files`] 의 끝.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct FilesRan {
    pub transactions: Vec<FilesTransaction>,
    /// 실패한 문장(파일 번호와 그 오류).
    pub failed: Option<(usize, DbScriptError)>,
    /// 중지로 멈췄다.
    pub stopped: bool,
    /// 돈 문장 수(실패한 문장 포함).
    pub statements: u32,
}

impl FilesRan {
    /// 마지막 트랜잭션의 끝 — 한 번도 열지 못했으면 `None`.
    pub fn last_end(&self) -> Option<SqlTransactionEnd> {
        self.transactions.last().map(|t| t.end)
    }
}

/// 실행 콘솔에 남기는 조회의 앞 행 수(05-mcp.md 「`sql` 종류 실행 구성」 — 메인 판정 M6-P4).
pub const RUN_ROWS_SHOWN: u32 = 5;
/// 실행 콘솔의 셀 상한.
pub const RUN_CELL_BYTES: usize = 256;

/// 파일들을 트랜잭션 방식대로 돌린다(17-m6-plan M6-09 ①): `all` 은 모든 파일이 트랜잭션 하나, `per_file` 은 파일마다 하나,
/// `none` 은 트랜잭션 없이 문장마다 자동 커밋이다. `read_only` 면(조회만 — 방식과 무관하게) 읽기 전용 트랜잭션 하나에서 돌고
/// 롤백한다. 실패하면 늘 멈춘다 — `all` 은 모든 파일을, `per_file` 은 그 파일을 롤백하고(앞 파일은 커밋됐다), `none` 은 실패 문장
/// 앞까지 남는다. 조회는 앞 [`RUN_ROWS_SHOWN`] 행만 남기고 끝까지 세며, 결과 행 전체는 남기지 않는다. 커밋 · 롤백 · 닫기의
/// 시한은 `db_execute` 와 같다. 연결은 닫지 않는다(부른 쪽).
pub(crate) async fn run_files(
    conn: &mut dyn SqlConnection,
    files: &[PlannedFile],
    transaction: SqlTransaction,
    read_only: bool,
    cancel: &QueryCancel,
    control: &dyn RunControl,
    observe: &mut (dyn FnMut(FilesEvent<'_>) + Send),
) -> FilesRan {
    let mut inactivity = Inactivity::new(Budget::PAGE.inactivity);
    let mut room = ResponseRoom {
        used: 0,
        limit: usize::MAX,
    };
    let in_transaction = read_only || transaction != SqlTransaction::None;
    let groups: Vec<std::ops::Range<usize>> = if read_only || transaction == SqlTransaction::All {
        vec![0..files.len()]
    } else {
        (0..files.len()).map(|i| i..i + 1).collect()
    };
    let mode = if read_only {
        ScriptMode::ReadOnly
    } else {
        ScriptMode::Write
    };
    let stop = || control.stopped();
    let mut out = FilesRan::default();
    for group in groups {
        if control.stopped() {
            out.stopped = true;
            break;
        }
        if in_transaction && let Err(error) = begin(conn, read_only, cancel, &mut inactivity).await
        {
            if let Some(first) = files[group.start].planned.first() {
                let failed = DbScriptError {
                    index: first.index,
                    line: LineIndex::new(&files[group.start].file.text).line(first.range),
                    range: first.range,
                    error,
                };
                observe(FilesEvent::Statement {
                    file: group.start,
                    event: StatementEvent::Failed(&failed),
                });
                out.failed = Some((group.start, failed));
            }
            let ended = FilesTransaction {
                files: group,
                end: SqlTransactionEnd::RolledBack,
                commit_error: None,
            };
            observe(FilesEvent::Ended(&ended));
            out.transactions.push(ended);
            break;
        }
        let mut broke = false;
        for at in group.clone() {
            let part = &files[at];
            let mut options = Statements {
                max_rows: RUN_ROWS_SHOWN,
                cell: CellLimit(RUN_CELL_BYTES),
                in_transaction,
                count_rest: true,
                stop: &stop,
            };
            let ran = run_statements(
                conn,
                mode,
                &part.file.text,
                &part.planned,
                &mut options,
                cancel,
                &mut inactivity,
                &mut room,
                &mut |event| observe(FilesEvent::Statement { file: at, event }),
            )
            .await;
            out.statements += ran.results.len() as u32 + u32::from(ran.error.is_some());
            if let Some(error) = ran.error {
                out.failed = Some((at, error));
                broke = true;
                break;
            }
            if ran.stopped {
                out.stopped = true;
                broke = true;
                break;
            }
        }
        let (end, commit_error) = if !in_transaction {
            (SqlTransactionEnd::AutoCommitted, None)
        } else if read_only {
            rollback(conn).await;
            (SqlTransactionEnd::ReadOnly, None)
        } else if broke {
            rollback(conn).await;
            (SqlTransactionEnd::RolledBack, None)
        } else if control.begin_commit() {
            let committed = within(Budget::PAGE.connect, "commit", conn.commit()).await;
            control.end_commit();
            match committed {
                Ok(()) => (SqlTransactionEnd::Committed, None),
                Err(e) if commit_outcome_unknown(&e) => (SqlTransactionEnd::Unknown, Some(e)),
                Err(e) => {
                    rollback(conn).await;
                    (SqlTransactionEnd::RolledBack, Some(e))
                }
            }
        } else {
            out.stopped = true;
            rollback(conn).await;
            (SqlTransactionEnd::RolledBack, None)
        };
        let failed_commit = commit_error.is_some();
        let ended = FilesTransaction {
            files: group,
            end,
            commit_error,
        };
        observe(FilesEvent::Ended(&ended));
        out.transactions.push(ended);
        if broke || out.stopped || failed_commit {
            break;
        }
    }
    if control.stopped() {
        out.stopped = true;
    }
    out
}

async fn rollback(conn: &mut dyn SqlConnection) {
    let _ = within(Budget::PAGE.cancel_cleanup, "rollback", conn.rollback()).await;
}

/// 커밋 오류가 시한 초과 · 끊김 · 중단이면 COMMIT 을 보낸 뒤라 DB 가 그것을 적용했는지 모른다. DB 가 답한 거절(제약 위반 · `busy`
/// 등 — 연결을 닫으면 트랜잭션이 버려진다)과 트랜잭션이 되돌려졌다는 표시가 있는 오류는 커밋되지 않았다. `db_execute` 와 실행
/// 구성의 `sql` 종류가 같이 쓴다(16-m5-plan M5-18b · 17-m6-plan M6-09).
pub(crate) fn commit_outcome_unknown(error: &DbError) -> bool {
    !error.rolled_back
        && matches!(
            error.kind,
            DbErrorKind::Timeout
                | DbErrorKind::Disconnected
                | DbErrorKind::Cancelled
                | DbErrorKind::Connect
        )
}

/// The result of one statement.
struct StatementRows {
    columns: Vec<ColumnMeta>,
    rows: Vec<Row>,
    rows_affected: Option<u64>,
    truncated: bool,
    /// 응답 예산을 다 썼다 — 뒤 문장은 돌리지 않는다.
    budget_spent: bool,
    /// 읽은 행 수 전체(`count_rest` 이고 행을 내는 문장일 때).
    rows_total: Option<u64>,
}

/// 문장 하나: 실행 → 행을 읽는 그때 응답 예산(`room`)에 더하며 `max_rows` 까지 읽고, 한 행을 더 읽어 넘는지 본다 → 넘거나 예산이
/// 다하면 커서를 버린다. 한 번에 읽는 몫도 남은 예산으로 줄인다 — 드라이버가 읽는 양이 예산 몫(+ 한 묶음)에 머문다.
async fn run_one(
    conn: &mut dyn SqlConnection,
    statement: &Planned,
    max_rows: u32,
    cell: CellLimit,
    count_rest: bool,
    cancel: &QueryCancel,
    inactivity: &mut Inactivity,
    room: &mut ResponseRoom,
) -> Result<StatementRows, DbError> {
    let mut request = ExecuteRequest::new(statement.text.clone(), statement.grade);
    request.cell_limit = cell;
    inactivity.rearm();
    let started = cancel.run(inactivity.guard(conn.execute(request))).await;
    interrupt_on_timeout(&started, cancel).await;
    let started = started?;
    let mut out = StatementRows {
        columns: started.columns,
        rows: Vec::new(),
        rows_affected: None,
        truncated: false,
        budget_spent: false,
        rows_total: None,
    };
    // 결과 하나(열 포함)가 들어갈 자리도 없으면 행 없이 끊는다.
    let header = result_overhead(&out.columns, statement.index, statement.grade);
    if !room.take(header) {
        out.budget_spent = true;
        out.truncated = started.end.is_none();
        if started.end.is_none() {
            within(
                Budget::PAGE.cancel_cleanup,
                "discard",
                conn.discard_cursor(),
            )
            .await?;
        }
        out.rows_affected = started.end.and_then(|e| e.rows_affected);
        return Ok(out);
    }
    if let Some(end) = started.end {
        out.rows_affected = end.rows_affected;
        return Ok(out);
    }
    let limit = max_rows as usize;
    let mut total = 0u64;
    loop {
        let wanted = if count_rest {
            FETCH.max_rows as usize
        } else {
            limit + 1 - out.rows.len()
        };
        let batch = cancel
            .run(inactivity.guard(conn.fetch(FetchLimit {
                max_rows: wanted.min(FETCH.max_rows as usize) as u32,
                max_bytes: (FETCH.max_bytes as usize).min(room.left().max(1)) as u32,
            })))
            .await;
        interrupt_on_timeout(&batch, cancel).await;
        let batch = batch?;
        let empty = batch.rows.is_empty();
        let mut stop = false;
        for row in batch.rows {
            if out.rows.len() == limit {
                // `max_rows` 를 넘는 행이 있었다. 세는 갈래는 버리고 센다.
                out.truncated = true;
                if count_rest {
                    total += 1;
                    continue;
                }
                stop = true;
                break;
            }
            if !room.take(row_size(&row)) {
                out.truncated = true;
                out.budget_spent = true;
                stop = true;
                break;
            }
            total += 1;
            out.rows.push(row);
        }
        if count_rest {
            out.rows_total = Some(total);
        }
        if stop {
            if batch.end.is_none() {
                within(
                    Budget::PAGE.cancel_cleanup,
                    "discard",
                    conn.discard_cursor(),
                )
                .await?;
            }
            return Ok(out);
        }
        if let Some(end) = batch.end {
            out.rows_affected = end.rows_affected;
            return Ok(out);
        }
        if empty {
            return Err(DbError::new(
                DbErrorKind::Invalid,
                "the driver returned no rows",
            ));
        }
    }
}

/// 비활동 타임아웃이면 서버에서 도는 문장을 멈춘다(그대로 두면 롤백 · 닫기가 그 문장을 기다린다).
async fn interrupt_on_timeout<T>(result: &Result<T, DbError>, cancel: &QueryCancel) {
    if matches!(result, Err(e) if e.kind == DbErrorKind::Timeout) {
        let _ = cancel.cancel().await;
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// 결과가 [`RESPONSE_BUDGET`] 안에 들게 행을 던다(머리 주석 ⑥). `fixed` 는 행을 뺀 결과의 크기(바이트)다. 앞 문장의 앞 행부터 채우고,
/// 넘는 자리부터 그 문장과 뒤 문장들의 행을 버리며 `truncated` 를 켠다.
pub(crate) fn fit_rows(results: &mut [DbScriptResult], fixed: usize) {
    let mut used = fixed;
    let mut full = false;
    for result in results {
        if full {
            if !result.rows.is_empty() {
                result.rows.clear();
                result.truncated = true;
            }
            continue;
        }
        let mut keep = result.rows.len();
        for (i, row) in result.rows.iter().enumerate() {
            // 행 하나의 JSON 과 구분 쉼표.
            let size = serde_json::to_string(row).map_or(usize::MAX, |s| s.len() + 1);
            if used.saturating_add(size) > RESPONSE_BUDGET {
                keep = i;
                full = true;
                break;
            }
            used += size;
        }
        if keep < result.rows.len() {
            result.rows.truncate(keep);
            result.truncated = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pw_db_types::DbValue;

    #[test]
    fn lines_count_from_one_in_utf16_offsets() {
        let source = "select 1;\n-- 한글\nselect 2;\n\nselect 3";
        let at = |needle: &str| {
            let byte = source.find(needle).unwrap();
            let from = source[..byte].encode_utf16().count() as u32;
            line_of(source, SqlRange { from, to: from })
        };
        assert_eq!(at("select 1"), 1);
        assert_eq!(at("select 2"), 3);
        assert_eq!(at("select 3"), 5);
    }

    /// 한 번 만든 줄 표가 글 처음부터 센 줄과 모든 오프셋에서 같다 — 줄바꿈 앞 · 위 · 뒤, 대리쌍, 글 끝 너머.
    #[test]
    fn one_line_index_answers_every_offset_like_counting_from_the_start() {
        let source = "select 1;\n-- 한글 😀\r\nselect 2;\n\nselect 3\n";
        let counted = |from: u32| {
            let mut utf16 = 0u32;
            let mut line = 1u32;
            for c in source.chars() {
                if utf16 >= from {
                    break;
                }
                if c == '\n' {
                    line += 1;
                }
                utf16 += c.len_utf16() as u32;
            }
            line
        };
        let lines = LineIndex::new(source);
        let end = source.encode_utf16().count() as u32;
        for from in 0..=end + 2 {
            let range = SqlRange { from, to: from };
            assert_eq!(lines.line(range), counted(from), "offset {from}");
            assert_eq!(line_of(source, range), counted(from), "offset {from}");
        }
    }

    #[test]
    fn input_is_sql_or_file_but_not_both() {
        assert_eq!(
            ScriptInput::of(Some("select 1".into()), None).unwrap(),
            ScriptInput::Sql("select 1".into())
        );
        assert_eq!(
            ScriptInput::of(None, Some("a.sql".into())).unwrap(),
            ScriptInput::File("a.sql".into())
        );
        for (sql, file) in [(Some("x".to_owned()), Some("y".to_owned())), (None, None)] {
            let e = ScriptInput::of(sql, file).unwrap_err();
            assert_eq!(e.code, rpc_code::INVALID_PARAMS);
        }
    }

    fn result(index: u32, rows: usize) -> DbScriptResult {
        DbScriptResult {
            index,
            line: 1,
            grade: RiskGrade::Read,
            columns: Vec::new(),
            rows: (0..rows)
                .map(|_| vec![DbValue::Text("x".repeat(1000))])
                .collect(),
            rows_affected: None,
            truncated: false,
            elapsed_ms: 0,
        }
    }

    #[test]
    fn rows_are_dropped_from_the_back_to_fit_the_budget() {
        let mut results = vec![result(0, 100), result(1, 100), result(2, 0)];
        fit_rows(&mut results, 1000);
        let total: usize = results
            .iter()
            .flat_map(|r| &r.rows)
            .map(|r| serde_json::to_string(r).unwrap().len() + 1)
            .sum();
        assert!(total + 1000 <= RESPONSE_BUDGET);
        assert_eq!(results[0].rows.len(), 100);
        assert!(!results[0].truncated);
        assert!(results[1].truncated);
        assert!(results[1].rows.len() < 100 && !results[1].rows.is_empty());
        assert!(
            !results[2].truncated,
            "a statement without rows lost nothing"
        );
    }
}
