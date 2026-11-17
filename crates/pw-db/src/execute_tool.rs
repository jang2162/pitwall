//! DB 쓰기 툴 메서드 `db_execute` ← `db.tool.execute`(16-m5-plan.md M5-18, 결정 M5-D4 · M5-D22 · M5-D35, `docs/design/05-mcp.md`
//! 「DB」 표와 그 아래). 그룹 `db`, 등급 `dangerous` — M5 에서 데이터를 바꾸는 유일한 툴이다. 프리셋 `full` 에서 보이고, 다른
//! 프리셋에서도 `[mcp.tools] db_execute = true` 로 그것만 켤 수 있다(노출은 등급으로 — `pw_mcp::exposure`).
//!
//! 입력은 `db_query` 와 같다(`sql` 또는 `file`). 읽기 · 분류 · 실행은 M5-07 의 파이프라인([`crate::script`])에 쓰기 갈래
//! ([`ScriptMode::Write`])를 더한 것이고, 관문은 M5-06 의 입구([`db_grant_for_agent`]) 하나다. 조회와 쓰기가 섞인 글도 받는다.
//!
//! 1. **관문** — 사람이 연 연결만, 프로덕션 대상은 `[mcp.policy] production` 이 `block`(기본)이면 거절, `ask` 면 지난다(이 툴은
//!    어차피 매번 묻는다 — 계획이 프로덕션 표지를 보인다).
//! 2. **읽기 · 분류** — `file` 은 지금 한 번 읽고(파일을 다시 열지 않는다), 연결의 방언으로 나누고 매긴다.
//! 3. **묻기 전에 거절하는 것**(아무것도 묻지도 돌리지도 않는다): 분류기가 읽지 못한 문장 — 파싱 실패 · `tooLarge` · `tooDeep` ·
//!    분류기가 못 읽은 읽기(결정 M5-D22 — 무엇인지 모르는 문장을 승인 다이얼로그에 올리지 않는다, 사람은 SQL 콘솔에서 돌린다,
//!    `data.reason = "unclassifiedStatement"`), 세션의 글 읽기를 바꾸는 문장(`changesSqlReading`), 트랜잭션 제어 문장(`BEGIN` ·
//!    `COMMIT` · `SAVEPOINT` · `SET TRANSACTION` 등 — 호출 하나가 트랜잭션 하나다, `transactionStatement`), read-only 연결의 조회
//!    밖 문장(`readOnlyConnection` — 표지는 그 대상에서 본 가장 엄한 것, 결정 M5-D37).
//! 4. **계획 → 승인**(매번 — 세션 허용도 `auto_approve` 도 없다): [`PlanChange::Sql`](실제로 돌릴 문장 전부 · 문장별 등급 · 연결
//!    이름 · 표지 · `file` 이면 경로와 내용 해시 · 「되돌릴 수 없다」). 해시는 연결의 푼 대상 · 방언 · 표지도 덮는다
//!    ([`crate::tool_plan`]). 첫 호출은 여기서 부작용 없이 「승인 필요」 로 돌아간다 — 연결을 열지 않고 비밀도 읽지 않는다.
//! 5. **실행**(결정 M5-D35) — 새 연결의 트랜잭션 하나. 모두 성공하면 커밋, 문장 하나가 실패하면 롤백하고 그 문장의 오류(DB 의
//!    오류 그대로 — 트랜잭션 안에서 못 도는 PG `CREATE INDEX CONCURRENTLY` 등도)를 돌려준다. 암묵 커밋 문장(MySQL · MariaDB 의
//!    DDL 등)이 있으면 계획이 `irreversible` 이고, 그 문장이 트랜잭션을 끝내면 뒤 문장을 위해 새 트랜잭션을 연다. read-only
//!    연결(조회만 든 글)은 읽기 전용 트랜잭션에서 돌고 롤백한다. 결과는 문장마다 영향 행 수(행을 내는 문장은 행 — `RETURNING` 도,
//!    응답 예산을 읽는 그때 센다).
//! 6. **스키마 캐시** — DDL 등급 이상의 문장이 돌았으면(실패 포함 — MySQL 은 실패한 DDL 도 암묵 커밋한다) 그 연결의 캐시를 버리고
//!    `db.schemaChanged` 를 낸다(창의 탐색기가 새로 읽는다).
//! 7. **감사** — 상세 칸에 문장 수 · 가장 높은 등급 · 문장 해시(`file` 이면 경로와 내용 해시), 끝까지 돈 호출이면 트랜잭션의 끝
//!    (`transaction=committed` · `rolledBack` · `readOnly`). SQL 원문 · 값은 남지 않는다.
//! 8. **취소**(메인 판정 2026-11-17) — 문장이 도는 중이면 인터럽트 · 롤백 · 닫기. 마지막 문장 뒤 **커밋하기로 정하기 전에** 관측된
//!    취소는 롤백한다. 커밋하기로 정하는 자리에서 RPC 의 「되돌릴 수 없는 지점」 표지(`pw_rpc::PointOfNoReturn`)를 세우므로, 그
//!    뒤의 취소는 커밋을 끊지 않고(인터럽트도 보내지 않는다) 핸들러도 버려지지 않는다 — Host 는 커밋 결과(`transaction:
//!    "committed"` 또는 커밋 오류)로 답하고, MCP 세션이 그 답으로 감사 행을 끝낸다(`pw_mcp::server` — 16-m5-plan M5-18b). 런타임이
//!    끝나며 버려진 호출만 `crate::script::CancelOnDrop` 의 경고 로그로 남는다.
//! 9. **연결 직전의 재확인** — 관문 · 승인 뒤 연결을 열 때 설정을 다시 풀어 대상과 표지를 판정한 것과 견준다(`DbGrant`
//!    `check_agent_use`). 그사이 대상이 바뀌었으면 `notOpenedByUser`, 표지가 더해졌으면 `productionBlocked` · `readOnlyConnection` —
//!    승인 해시는 판정 때의 값이라 이 틈(큰 SQL 의 분류 시간 등)을 덮지 못한다.
//!
//! **남는 한계**: 외부 표(postgres_fdw 등)를 거치는 쓰기는 원격 서버의 표지를 보지 않는다(결정 M5-D38 — `db_query` 와 같다). 부작용
//! 함수 목록(`pw_db_sql::risk`)은 조회 판정용이다 — 이 툴에서는 모든 문장이 계획에 보이고 사람이 승인한다.

use std::sync::Arc;

use pw_core::registry::agent::agent_grant;
use pw_core::registry::plan::{PlanChange, Planned};
use pw_db_driver::budget::within;
use pw_db_driver::{Budget, DbConnection, QueryCancel};
use pw_db_sql::risk::RiskReason;
use pw_db_types::{CellLimit, DbError, DbErrorKind, RiskGrade};
use pw_rpc::{Ctx, Router, RpcError, Tier, code as rpc_code, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::execute::MAX_PAGE_ROWS;
use crate::gate::{
    DbStatementClass, Planned as Statement, Policy, db_grant_for_agent, judge,
    on_classifier_thread, plan, refuse_reading_changes,
};
use crate::schema::check_text;
use crate::script::{
    CancelOnDrop, DEFAULT_MAX_ROWS, DbScriptError, DbScriptResult, DbScriptUnlisted, ResponseRoom,
    ScriptInput, ScriptMode, TOOL_CELL_BYTES, audit_detail, fit_rows, line_of, read_script_file,
    run_script,
};
use crate::session::{DbSessions, blocking, db_error};
use crate::tool_plan::{db_plan, sql_change};

/// 이 툴의 RPC 이름(계획의 `method`).
pub const METHOD: &str = "db.tool.execute";

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolExecuteParams {
    pub project_id: String,
    /// Top of the worktree (absolute path). Connection settings and `file` are read from that worktree. Empty means the
    /// session's worktree, or else the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Connection id from `db_connection_list`.
    pub connection_id: String,
    /// SQL to run (several statements — split by the dialect's delimiter). Cannot be given together with `file`.
    #[serde(default)]
    pub sql: Option<String>,
    /// SQL file — a path relative to the project root (inside the chosen worktree, if one is given). A UTF-8 regular
    /// file inside the root, up to 5 MiB. Cannot be given together with `sql`.
    #[serde(default)]
    pub file: Option<String>,
    /// Maximum rows returned per statement that returns rows (1 – 10000, default 100). Not paged — past it the
    /// statement is `truncated`.
    #[serde(default)]
    pub max_rows: Option<u32>,
}

/// How the call's transaction ended.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbToolTransaction {
    /// Every statement ran and the transaction was committed.
    Committed,
    /// A statement failed (`error`) and the transaction was rolled back — nothing of this call is kept, except what
    /// statements that commit on their own already committed (`irreversible`).
    RolledBack,
    /// A read-only connection: only queries ran, in a read-only transaction that was rolled back.
    ReadOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbToolExecute {
    /// Number of statements after splitting.
    pub statements: u32,
    /// One result per statement that ran and is listed (in order): rows changed (`rowsAffected`), or the rows of a
    /// statement that returns rows (`RETURNING`, queries).
    pub results: Vec<DbScriptResult>,
    /// If a statement failed, that statement and the database's error. The statements after it did not run and the
    /// transaction was rolled back.
    pub error: Option<DbScriptError>,
    /// Statements that ran but are not listed, because the rows of earlier statements used up the response budget
    /// (about 192 KiB): `count` statements from `index`, with the sum of the rows they changed.
    pub unlisted: Option<DbScriptUnlisted>,
    pub transaction: DbToolTransaction,
    /// The SQL had statements that commit on their own and cannot be rolled back (MySQL · MariaDB DDL and the like).
    pub irreversible: bool,
    /// The file read, when `file` was given (path relative to the root).
    pub file: Option<String>,
    /// Details kept in the audit log — statement count · highest grade · statement hashes (for `file`, its path and
    /// content hash). No SQL text or values.
    pub audit: String,
}

rpc_method! {
    /// Run SQL that changes data (SQL drivers only — PG · MySQL/MariaDB · SQLite). The input is either `sql` (text) or
    /// `file` (an SQL file path relative to the project root — Pitwall reads it once when it runs). Queries and writes
    /// may be mixed. **Every call asks the person in Pitwall first**: the request shows every statement with its risk
    /// grade, the connection and its marks (and the file path and hash for `file`), and the statements run only after
    /// the person allows them — exactly the statements that were shown (if the file or the connection's target · marks
    /// change after approval, nothing runs). There is no "allow for this session" and `auto_approve` does not apply.
    /// The whole call is **one transaction**: if every statement succeeds it is committed; if one fails, the call stops
    /// there, the transaction is rolled back and the database's error is returned (`error`, with statement index ·
    /// line) — including for statements that cannot run inside a transaction (PG `CREATE INDEX CONCURRENTLY`, `VACUUM`,
    /// ...). Statements that commit on their own (MySQL · MariaDB DDL) cannot be rolled back (`irreversible`). Refused
    /// before asking: text that cannot be classified (cannot be parsed, or a statement over 512 KiB — run it in
    /// Pitwall's SQL console instead), transaction control (`BEGIN` · `COMMIT` · `SAVEPOINT` ...), statements that change
    /// how the server reads SQL text, and anything but queries on a read-only connection. Each statement's result is the
    /// number of rows changed, or its rows (`RETURNING`, queries — up to `maxRows`, cells cut at 4 KiB). Uses only
    /// connections the person opened in Pitwall's DB explorer during this app run (`notOpenedByUser`) and does not use a
    /// target marked production (`productionBlocked`) unless the person set production targets to ask. If you cancel a
    /// call after its last statement ran, its commit may already have been sent: the call reports cancelled but the
    /// change may be committed — check with `db_query` before calling again.
    pub DbToolExecuteMethod: "db.tool.execute" {
        params: Planned<DbToolExecuteParams>,
        result: DbToolExecute,
        tier: Dangerous,
        roles: [Owner],
        mcp: { group: "db", name: "db_execute" },
    }
}

/// 승인을 묻기 전에 거절하는 까닭(`data.reason`).
pub const UNCLASSIFIED_STATEMENT: &str = "unclassifiedStatement";
pub const TRANSACTION_STATEMENT: &str = "transactionStatement";

/// 분류기가 읽지 못한 문장(결정 M5-D22): 파싱 실패 · 크기 · 깊이 상한, 그리고 서버가 고를 수 있는 읽기 가운데 분류기가 읽지 못한
/// 것이 있는 조회(`gate::Planned::strict_class` 가 파싱 실패로 싣는 것과 같다).
fn unclassified(statement: &Statement) -> bool {
    statement.unread_reading
        || statement.reasons.iter().any(|r| {
            matches!(
                r,
                RiskReason::Unparsed | RiskReason::TooLarge | RiskReason::TooDeep
            )
        })
}

fn refuse(reason: &str, message: String, refused: Vec<DbStatementClass>, audit: &str) -> RpcError {
    RpcError::new(rpc_code::PERMISSION_DENIED, message).with_data(json!({
        "reason": reason,
        "statements": refused,
        "audit": audit,
    }))
}

/// 묻기 전의 거절(머리 주석 ③ — read-only 는 따로).
fn refuse_before_asking(source: &str, planned: &[Statement], audit: &str) -> Result<(), RpcError> {
    let unknown: Vec<&Statement> = planned.iter().filter(|s| unclassified(s)).collect();
    if let Some(first) = unknown.first() {
        return Err(refuse(
            UNCLASSIFIED_STATEMENT,
            format!(
                "db_execute runs only statements Pitwall can classify; nothing ran: statement {} (line {}) cannot be \
                 read (it does not parse, is over 512 KiB or nests too deep){} — run it in Pitwall's SQL console instead",
                first.index,
                line_of(source, first.range),
                if unknown.len() > 1 {
                    format!(" ({} statements cannot be read)", unknown.len())
                } else {
                    String::new()
                },
            ),
            unknown.iter().map(|s| s.strict_class()).collect(),
            audit,
        ));
    }
    refuse_reading_changes(planned).map_err(|e| crate::tools::with_audit(e, audit))?;
    let control: Vec<&Statement> = planned
        .iter()
        .filter(|s| s.control.is_some() || s.reasons.contains(&RiskReason::TransactionControl))
        .collect();
    if let Some(first) = control.first() {
        return Err(refuse(
            TRANSACTION_STATEMENT,
            format!(
                "db_execute runs the whole call in one transaction, so transaction control is not accepted; nothing \
                 ran: statement {} (line {}) controls the transaction — remove BEGIN / COMMIT / ROLLBACK / SAVEPOINT / \
                 SET TRANSACTION",
                first.index,
                line_of(source, first.range),
            ),
            control.iter().map(|s| s.class()).collect(),
            audit,
        ));
    }
    Ok(())
}

impl DbSessions {
    /// `db_execute`.
    async fn tool_execute(
        self: &Arc<Self>,
        ctx: &Ctx,
        input: Planned<DbToolExecuteParams>,
    ) -> Result<DbToolExecute, RpcError> {
        let p = &input.params;
        check_text("worktree", p.worktree.as_deref())?;
        let script = ScriptInput::of(p.sql.clone(), p.file.clone())?;
        let max_rows = p.max_rows.unwrap_or(DEFAULT_MAX_ROWS);
        if max_rows == 0 || max_rows > MAX_PAGE_ROWS {
            return Err(RpcError::invalid_params(format!(
                "maxRows must be 1-{MAX_PAGE_ROWS}"
            )));
        }
        // 관문(M5-06 의 입구)을 먼저 — 거절할 호출이면 파일도 읽지 않는다.
        let (s, scope, project, worktree, id) = (
            Arc::clone(self),
            ctx.scope().clone(),
            p.project_id.clone(),
            p.worktree.clone(),
            p.connection_id.clone(),
        );
        let mut grant = blocking(move || {
            let agent = agent_grant(&scope, &s.connections.host()?.projects, &project)?;
            db_grant_for_agent(&s, &agent, worktree.as_deref(), &id, None)
        })
        .await?;
        let driver = grant
            .agent_driver()
            .and_then(|kind| self.drivers.get(kind.id()))
            .ok_or_else(|| {
                db_error(DbError::new(
                    DbErrorKind::Unsupported,
                    "the db driver of this connection is not installed",
                ))
            })?;
        let dialect = match driver.dialect() {
            Some(dialect) if driver.capabilities().sql => dialect,
            _ => {
                return Err(db_error(DbError::new(
                    DbErrorKind::Unsupported,
                    format!(
                        "db_execute runs SQL databases only; `{}` is a key-value store",
                        p.connection_id
                    ),
                )));
            }
        };
        // `file` 은 지금 한 번 읽고, 읽은 그 글을 분류하고 계획에 싣고 그 글을 돌린다 — 승인 뒤에도 파일을 다시 열지 않는다.
        let (source, file) = match script {
            ScriptInput::Sql(sql) => (sql, None),
            ScriptInput::File(path) => {
                let (s, project, worktree) =
                    (Arc::clone(self), p.project_id.clone(), p.worktree.clone());
                let file = blocking(move || {
                    read_script_file(
                        &s.connections.host()?.projects,
                        &project,
                        worktree.as_deref(),
                        &path,
                    )
                })
                .await?;
                (file.text.clone(), Some(file))
            }
        };
        let text = source.clone();
        let planned = on_classifier_thread(move || plan(dialect, &text, None)).await??;
        let audit = audit_detail(dialect, &planned, file.as_ref());
        let with_audit = |e: RpcError| crate::tools::with_audit(e, &audit);
        refuse_before_asking(&source, &planned, &audit)?;
        let marks = grant.agent_marks().unwrap_or_default();
        let name = grant.agent_connection_name().unwrap_or("?").to_owned();
        if marks.read_only {
            let policy = Policy {
                read_only: true,
                production: marks.production,
                name: name.clone(),
            };
            judge(&policy, &planned, &[], "", None).map_err(with_audit)?;
        }
        let mode = if marks.read_only {
            ScriptMode::ReadOnly
        } else {
            ScriptMode::Write
        };
        let irreversible =
            mode == ScriptMode::Write && planned.iter().any(|s| s.implicit_commit || s.may_end);
        let summary = format!(
            "Run {} SQL statement{} on the {}DB connection `{name}`{}",
            planned.len(),
            if planned.len() == 1 { "" } else { "s" },
            if marks.production { "production " } else { "" },
            if irreversible {
                " — some cannot be rolled back"
            } else {
                ""
            },
        );
        let change: PlanChange = sql_change(&grant, &source, &planned, file.as_ref(), irreversible);
        let plan = db_plan(
            METHOD,
            Tier::Dangerous,
            p,
            &grant,
            Some(dialect),
            summary,
            Some(change),
        );
        // 첫 호출은 여기서 부작용 없이 돌아간다(연결을 열지 않는다). 승인 뒤에는 위에서 읽은 그 글 · 대상으로만 돈다.
        let approved = input.approve(plan)?;
        grant.approve(&approved);
        let opened =
            crate::connection::connect(self, &grant, p.worktree.as_deref(), &p.connection_id, None)
                .await
                .map_err(with_audit)?;
        let tunnel = opened.tunnel;
        let mut conn = match opened.connection {
            DbConnection::Sql(conn) => conn,
            other => {
                let _ = other.close().await;
                return Err(with_audit(RpcError::invalid_params(
                    "this connection does not run SQL",
                )));
            }
        };
        let caps = &opened.driver.capabilities;
        let unsupported = match mode {
            ScriptMode::ReadOnly if !caps.read_only_transaction => {
                Some("this driver has no read-only transactions")
            }
            ScriptMode::Write if !caps.transactions => Some("this driver has no transactions"),
            _ => None,
        };
        if let Some(why) = unsupported {
            let _ = conn.close().await;
            return Err(with_audit(db_error(DbError::new(
                DbErrorKind::Unsupported,
                format!("{why}, so db_execute cannot run on it"),
            ))));
        }
        let cell = CellLimit(TOOL_CELL_BYTES.min(self.cell_limit().0));
        let file_path = file.map(|f| f.path);
        let mut room = ResponseRoom::new(
            serde_json::to_string(&DbToolExecute {
                statements: u32::MAX,
                results: Vec::new(),
                error: None,
                unlisted: Some(DbScriptUnlisted {
                    index: u32::MAX,
                    line: u32::MAX,
                    count: u32::MAX,
                    rows_affected: u64::MAX,
                }),
                transaction: DbToolTransaction::RolledBack,
                irreversible,
                file: file_path.clone(),
                audit: audit.clone(),
            })
            .map_or(0, |s| s.len())
                + crate::tools::ERROR_ROOM,
        );
        let cancel = QueryCancel::new(conn.interrupter(), Budget::PAGE.cancel_cleanup);
        // 실행은 연결을 쥔 태스크에서 돈다 — 호출이 버려져도(취소 · 세션 끊김) 태스크가 드라이버 호출을 버리고 롤백한 뒤 닫는다.
        let guard = CancelOnDrop::new(cancel.clone());
        let point = guard.commit_point();
        // 커밋을 정하는 그 자리에서 세우는 RPC 의 표지 — 선 뒤의 취소는 핸들러를 버리지 않아 커밋 결과가 답이 된다.
        let no_return = ctx.point_of_no_return();
        let hooks = self.test_hooks.take_commit_hooks();
        let call_cancel = ctx.cancellation().clone();
        // 태스크는 커밋 · 롤백을 마치면 곧바로 답을 넘기고 연결 닫기는 그 뒤에 한다 — 닫기(터널 · 끊긴 네트워크)가 느려도 답이 늦지
        // 않게. 표지 뒤의 답을 기다리는 MCP 세션의 시한(5 s)을 닫기가 먹지 않는다.
        let (answer, answered) = tokio::sync::oneshot::channel();
        {
            let (source, planned) = (source.clone(), planned.clone());
            let (sessions, project, connection) = (
                Arc::clone(self),
                grant.project_id().to_owned(),
                p.connection_id.clone(),
            );
            tokio::spawn(async move {
                let _tunnel = tunnel;
                let ran = run_script(
                    conn.as_mut(),
                    mode,
                    &source,
                    &planned,
                    max_rows,
                    cell,
                    &cancel,
                    &mut room,
                )
                .await;
                let ended = match &ran {
                    Ok(ran) if mode == ScriptMode::Write && ran.error.is_none() => {
                        if let Some(hook) = hooks.before_commit {
                            hook().await;
                        }
                        // 커밋 전에 호출이 취소됐으면 커밋하지 않는다. RPC 의 표지를 먼저 세운다 — 세웠으면 그 뒤의 취소(`request.cancel` ·
                        // 세션 닫힘)는 핸들러를 버리지 않고 커밋 결과로 답하게 한다. 못 세웠으면 취소가 먼저 왔다.
                        if no_return.pass() && point.begin_commit() {
                            if let Some(hook) = hooks.after_commit_point {
                                hook().await;
                            }
                            if let Some(hook) = hooks.after_commit_point_watching {
                                hook(call_cancel).await;
                            }
                            let committed =
                                within(Budget::PAGE.connect, "commit", conn.commit()).await;
                            // 표지 뒤에는 취소로 버려지지 않는다 — 여기 오는 것은 런타임이 끝나며 버려진 호출뿐이다(답할 곳이 없다).
                            if point.cancelled_after_commit() {
                                tracing::warn!(
                                    target: "pw_db",
                                    project = %project,
                                    connection = %connection,
                                    committed = committed.is_ok(),
                                    "db_execute was dropped after its commit was sent; no answer was sent, and the \
                                     transaction {}",
                                    commit_outcome(&committed)
                                );
                            }
                            Some(committed)
                        } else {
                            tracing::info!(target: "pw_db", project = %project, connection = %connection, "db_execute was cancelled before its commit; rolling back");
                            let _ =
                                within(Budget::PAGE.cancel_cleanup, "rollback", conn.rollback())
                                    .await;
                            None
                        }
                    }
                    _ => {
                        let _ =
                            within(Budget::PAGE.cancel_cleanup, "rollback", conn.rollback()).await;
                        None
                    }
                };
                // DDL 이상이 시작됐으면(실패 포함 — MySQL 은 실패한 DDL 도 암묵 커밋한다) 스키마 캐시를 버린다. 호출이 버려졌어도
                // 한다 — 창의 탐색기가 새로 읽는다. 답보다 먼저 — 답을 받은 에이전트가 곧바로 읽는 스키마가 새것이게.
                if let Ok(ran) = &ran {
                    let reached = ran
                        .error
                        .as_ref()
                        .map_or(planned.len(), |e| e.index as usize + 1);
                    if planned
                        .iter()
                        .filter(|s| (s.index as usize) < reached)
                        .any(|s| s.grade >= RiskGrade::Ddl)
                    {
                        sessions.schema_changed(&project, &connection);
                    }
                }
                let _ = answer.send((ran, ended));
                let _ = within(Budget::PAGE.connect, "close", conn.close()).await;
            });
        }
        let (ran, committed) = answered.await.map_err(|_| {
            with_audit(RpcError::internal(
                "the db_execute task ended without an answer",
            ))
        })?;
        guard.disarm();
        let ran = ran.map_err(|e| with_audit(db_error(e)))?;
        if let Some(Err(e)) = committed {
            return Err(commit_failed(e, &audit));
        }
        let transaction = match (mode, &ran.error) {
            (ScriptMode::ReadOnly, _) => DbToolTransaction::ReadOnly,
            (ScriptMode::Write, None) => DbToolTransaction::Committed,
            (ScriptMode::Write, Some(_)) => DbToolTransaction::RolledBack,
        };
        // 감사 상세에 트랜잭션의 끝을 싣는다 — 클라이언트가 커밋 뒤에 취소한 호출도 그 행이 커밋을 보인다(16-m5-plan M5-18b).
        let audit = format!(
            "{audit} transaction={}",
            match transaction {
                DbToolTransaction::Committed => "committed",
                DbToolTransaction::RolledBack => "rolledBack",
                DbToolTransaction::ReadOnly => "readOnly",
            }
        );
        let mut result = DbToolExecute {
            statements: u32::try_from(planned.len()).unwrap_or(u32::MAX),
            results: ran.results,
            error: ran.error,
            unlisted: ran.unlisted,
            transaction,
            irreversible,
            file: file_path,
            audit,
        };
        // 겹방어: 실행 중의 셈이 어긋났으면 행을 마저 던다(영향 행 수는 그대로).
        let rows: Vec<Vec<pw_db_types::Row>> = result
            .results
            .iter_mut()
            .map(|r| std::mem::take(&mut r.rows))
            .collect();
        let fixed = serde_json::to_string(&result).map_or(0, |s| s.len());
        for (r, rows) in result.results.iter_mut().zip(rows) {
            r.rows = rows;
        }
        fit_rows(&mut result.results, fixed);
        Ok(result)
    }
}

/// 커밋 오류가 시한 초과 · 끊김 · 중단이면 COMMIT 을 보낸 뒤라 DB 가 그것을 적용했는지 모른다. DB 가 답한 거절(제약 위반 · `busy`
/// 등 — 연결을 닫으면 트랜잭션이 버려진다)과 트랜잭션이 되돌려졌다는 표시가 있는 오류는 커밋되지 않았다.
fn commit_outcome_unknown(error: &DbError) -> bool {
    !error.rolled_back
        && matches!(
            error.kind,
            DbErrorKind::Timeout
                | DbErrorKind::Disconnected
                | DbErrorKind::Cancelled
                | DbErrorKind::Connect
        )
}

/// 커밋의 끝을 로그 문구로.
fn commit_outcome(committed: &Result<(), DbError>) -> &'static str {
    match committed {
        Ok(()) => "committed",
        Err(e) if commit_outcome_unknown(e) => "may or may not have committed",
        Err(_) => "did not commit",
    }
}

/// 커밋이 실패한 호출의 오류. 감사 상세 끝에 트랜잭션의 끝(`transaction=rolledBack` · `unknown`)을 싣고, 끝을 모르면 에이전트에게
/// 「커밋을 보냈고 결과를 모른다 — `db_query` 로 확인하라」 를 말한다(16-m5-plan M5-18b — 리뷰 F1).
fn commit_failed(error: DbError, audit: &str) -> RpcError {
    let unknown = commit_outcome_unknown(&error);
    let audit = format!(
        "{audit} transaction={}",
        if unknown { "unknown" } else { "rolledBack" }
    );
    let mut rpc = db_error(error);
    if unknown {
        rpc.message = format!(
            "commit sent, outcome unknown: {}. The changes may or may not be in the database; check with \
             db_query before calling again",
            rpc.message
        );
    }
    crate::tools::with_audit(rpc, &audit)
}

/// 툴 메서드를 붙인다(`crate::session::register` 가 부른다).
pub(crate) fn register(router: &mut Router, sessions: Arc<DbSessions>) {
    router.unary(
        DbToolExecuteMethod,
        move |ctx, p: Planned<DbToolExecuteParams>| {
            let s = Arc::clone(&sessions);
            async move { s.tool_execute(&ctx, p).await }
        },
    );
}
