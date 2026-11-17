//! 에이전트 DB 툴의 승인 계획(16-m5-plan.md M5-18 ① ④ — 계획 꼴은 `pw_core::registry::plan`, 승인 경로는 `pw_mcp`).
//!
//! 승인을 청하는 DB 툴 호출은 셋이다(셋째 `db_connection_test` 는 `crate::connection_tool`):
//!
//! - **`db_execute`**(`dangerous`) — 늘 묻는다. 계획은 [`PlanChange::Sql`](실제로 돌릴 문장 전부 · 문장별 등급 · 연결 이름 · 표지 ·
//!   `file` 이면 경로와 내용 해시 · 「되돌릴 수 없다」).
//! - **프로덕션 `ask` 의 조회**(`db_schema` · `db_query` · `db_kv_scan` · `db_kv_get`) — 관문이 프로덕션 대상을 `[mcp.policy]
//!   production = "ask"` 로 지나보낸 증거([`DbGrant::needs_approval`])일 때만 계획을 낸다. 그 밖에는 지금처럼 바로 돈다.
//!
//! 둘 다 규칙은 「`auto_approve` 없음 · 기억 없음」(세션 허용이 없다 — 「프로덕션 태그 연결의 판정」 표)이다. 해시는 서버가 채운
//! 입력과 **대상 값**(연결 id · 워크트리 · 푼 대상 · 에이전트에게 적용하는 표지 — [`DbGrant::plan_target`], 그리고 방언)을 덮는다
//! — 승인 뒤 연결 설정의 대상 · 표지가 바뀌면 다시 만든 계획의 해시가 달라 돌지 않는다(`plan_changed`).
//!
//! **TOCTOU**(`pw_core::registry::plan` 머리 주석): 승인 뒤의 부작용은 그 호출에서 계획을 만들 때 읽은 값으로 한다 — `file` 은 다시
//! 읽지 않고 읽은 글을 돌리고, 연결은 관문이 판정한 대상으로 연다(연결을 여는 자리가 그때 다시 읽은 대상이 다르면 열지 않는다 —
//! `DbGrant::check_agent_use`).
//!
//! **다른 툴이 감싸는 법**(M5-17 의 `db_connection_test` 등): 입력 타입을 `Planned<P>` 로 감싸 `rpc_method!` 의 `params` 에 둔다
//! (입력 스키마 · TS 타입은 `P` 그대로 — 카탈로그가 표지를 지운다). 핸들러는 관문을 지난 뒤 부작용 전에 [`db_plan`] 으로 계획을
//! 만들고 `planned.approve(plan)?` 로 [`Approved`] 를 받는다(첫 호출은 여기서 「승인 필요」 로 돌아간다 — 부작용 없이).

use pw_core::registry::plan::{
    ApprovalTicket, Approved, PlanChange, PlanRules, Remember, SqlFile, SqlGrade, SqlStatement,
    SqlTarget, ToolPlan, approve,
};
use pw_db_types::{RiskGrade, SqlDialect};
use pw_rpc::{RpcError, Tier};
use serde::Serialize;

use crate::gate::{DbGrant, Planned};
use crate::script::{ScriptFile, line_of};

/// 승인 계획의 규칙: `auto_approve` 를 적용하지 않고 세션 허용도 없다(매번 묻는다).
pub const EVERY_TIME: PlanRules = PlanRules {
    auto_approve: false,
    remember: Remember::Never,
};

/// DB 툴의 계획. `grant` 는 에이전트의 증거(대상 값을 해시에 넣는다), `dialect` 는 연결의 방언(SQL 드라이버). 규칙은
/// [`EVERY_TIME`] — 다른 규칙이 필요한 툴(`db_connection_test` 의 「이 연결 열기」 — `Remember::Key`)은 [`db_plan_with`].
pub fn db_plan(
    method: &str,
    tier: Tier,
    input: &impl Serialize,
    grant: &DbGrant,
    dialect: Option<SqlDialect>,
    summary: String,
    change: Option<PlanChange>,
) -> ToolPlan {
    db_plan_with(
        method, tier, input, grant, dialect, summary, change, EVERY_TIME,
    )
}

/// [`db_plan`] 에 승인 규칙을 준다.
#[allow(
    clippy::too_many_arguments,
    reason = "the plan's parts are separate values; db_plan keeps the shorter form"
)]
pub fn db_plan_with(
    method: &str,
    tier: Tier,
    input: &impl Serialize,
    grant: &DbGrant,
    dialect: Option<SqlDialect>,
    summary: String,
    change: Option<PlanChange>,
    rules: PlanRules,
) -> ToolPlan {
    let mut builder = ToolPlan::builder(method, tier, input)
        .summary(summary)
        .target("db", &grant.plan_target())
        .target("dialect", &dialect)
        .rules(rules);
    if let Some(change) = change {
        builder = builder.change(change);
    }
    builder.build()
}

/// 프로덕션 `ask` 의 조회(머리 주석): 증거가 승인을 청하면 이 호출의 계획을 만들어 승인 표와 견주고, 맞으면 증거에 단다. 표가 없거나
/// 다르면 계획을 실은 「승인 필요」 오류다(부작용 없이). 증거가 승인을 청하지 않으면 아무것도 하지 않는다.
pub(crate) fn approve_production_read(
    grant: &mut DbGrant,
    method: &str,
    input: &impl Serialize,
    ticket: Option<&ApprovalTicket>,
    dialect: Option<SqlDialect>,
    what: &str,
    change: Option<PlanChange>,
) -> Result<(), RpcError> {
    if !grant.needs_approval() {
        return Ok(());
    }
    let name = grant.agent_connection_name().unwrap_or("?").to_owned();
    let summary = format!("Read {what} on the production DB connection `{name}`");
    let plan = db_plan(method, Tier::Read, input, grant, dialect, summary, change);
    let approved: Approved = approve(ticket, plan)?;
    grant.approve(&approved);
    Ok(())
}

/// `RiskGrade` → 계획의 등급(같은 이름).
pub fn sql_grade(grade: RiskGrade) -> SqlGrade {
    match grade {
        RiskGrade::Read => SqlGrade::Read,
        RiskGrade::Write => SqlGrade::Write,
        RiskGrade::UnboundedWrite => SqlGrade::UnboundedWrite,
        RiskGrade::Ddl => SqlGrade::Ddl,
        RiskGrade::Destructive => SqlGrade::Destructive,
    }
}

/// 실제로 돌릴 문장들의 [`PlanChange::Sql`].
pub(crate) fn sql_change(
    grant: &DbGrant,
    source: &str,
    planned: &[Planned],
    file: Option<&ScriptFile>,
    irreversible: bool,
) -> PlanChange {
    let marks = grant.agent_marks().unwrap_or_default();
    PlanChange::Sql {
        connection: SqlTarget {
            name: grant.agent_connection_name().unwrap_or("?").to_owned(),
            production: marks.production,
            read_only: marks.read_only,
        },
        file: file.map(|f| SqlFile {
            path: f.path.clone(),
            sha256: f.content_hash.clone(),
        }),
        statements: planned
            .iter()
            .map(|s| SqlStatement {
                index: s.index,
                line: line_of(source, s.range),
                grade: sql_grade(s.grade),
                text: s.text.clone(),
            })
            .collect(),
        irreversible,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_grades_keep_the_risk_grade_names() {
        for grade in [
            RiskGrade::Read,
            RiskGrade::Write,
            RiskGrade::UnboundedWrite,
            RiskGrade::Ddl,
            RiskGrade::Destructive,
        ] {
            assert_eq!(
                serde_json::to_value(sql_grade(grade)).unwrap(),
                serde_json::to_value(grade).unwrap()
            );
        }
    }
}
