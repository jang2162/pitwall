//! 계획 등급의 회귀 표(17-m6-plan M6-10 ⑨ — 결정 M6-P9). 세션은 승인을 **계획 등급**으로 판정하고 선언 등급은 상한이다. 이
//! 바뀜이 M5 툴의 승인을 넓히지 않는 근거는 「지금 계획을 내는 툴은 모두 계획 등급 = 선언 등급」 이라는 것이다 — 이 표가 그것을
//! 적는다.
//!
//! 툴 행이 하는 것: 계획을 내는 툴(`Planned` 입력 — `write` 이상 전부와 호출마다 승인을 청하는 `read` 툴)을 더하면 [`PLAN_TIERS`]
//! 에 (선언 등급, 그 툴이 낼 수 있는 계획 등급들)을 이름순 자리에 더한다. 계획 등급을 선언보다 **낮게** 내는 툴(`run_start` 의
//! 프로세스 구성 `exec` 등 — M6-14)은 [`LOWERED`] 에도 까닭과 함께 적는다 — 그 툴에는 `auto_approve` · 세션 허용이 선언 등급보다
//! 낮은 등급으로 닿는다. 표에 없는 계획 툴, 표와 다른 선언 등급, 선언을 넘는 계획 등급, [`LOWERED`] 에 없는 낮춤은 실패한다.

use pw_rpc::Tier::{self, Dangerous, Read, Write};

/// 계획을 내는 툴 전부: (이름, 선언 등급, 낼 수 있는 계획 등급). 이름순. 계획 등급은 툴이 `ToolPlan::builder` 에 넘기는 값이다
/// — 고치면 이 표도 고친다.
const PLAN_TIERS: &[(&str, Tier, &[Tier])] = &[
    // 「이 연결 열기」 · 프로덕션 `ask` 의 시험 — `connection_tool` 의 계획은 늘 선언 그대로 `write`.
    ("db_connection_test", Write, &[Write]),
    ("db_connection_upsert", Write, &[Write]),
    // `db_plan(.., Tier::Dangerous, ..)` — 문장의 분류와 상관없이 `dangerous`(결정 M5-D4).
    ("db_execute", Dangerous, &[Dangerous]),
    // 프로덕션 `ask` 의 조회(`tool_plan::approve_production_read` — `Tier::Read`).
    ("db_kv_get", Read, &[Read]),
    ("db_kv_scan", Read, &[Read]),
    ("db_query", Read, &[Read]),
    ("db_schema", Read, &[Read]),
    ("open_with_upsert", Write, &[Write]),
    ("project_register", Write, &[Write]),
];

/// 계획 등급을 선언보다 낮게 낼 수 있는 툴. 툴마다 까닭을 적는다. 지금(M6-10)은 없다.
const LOWERED: &[&str] = &[];

#[test]
fn every_planning_tool_is_in_the_plan_tier_table() {
    let catalog = pw_mcp::Catalog::linked();
    let planned: Vec<(&str, Tier)> = catalog
        .tools()
        .iter()
        .filter(|t| t.planned)
        .map(|t| (t.name.as_str(), t.tier))
        .collect();
    let table: Vec<(&str, Tier)> = PLAN_TIERS.iter().map(|(n, d, _)| (*n, *d)).collect();
    assert_eq!(
        planned, table,
        "계획을 내는 툴과 그 선언 등급이 회귀 표와 다르다 — 표에 (선언 등급, 계획 등급)을 적는다"
    );
    let mut lowered = Vec::new();
    for (name, declared, plans) in PLAN_TIERS {
        assert!(!plans.is_empty(), "{name}: 계획 등급이 없다");
        assert!(
            plans.iter().all(|p| p <= declared),
            "{name}: 선언을 넘는 계획 등급은 세션이 툴의 버그로 거절한다 — 선언을 올리거나 툴을 나눈다"
        );
        if plans.iter().any(|p| p < declared) {
            lowered.push(*name);
        }
    }
    assert_eq!(
        lowered, LOWERED,
        "계획 등급을 선언보다 낮게 내는 툴은 LOWERED 에 까닭과 함께 적는다"
    );
}
