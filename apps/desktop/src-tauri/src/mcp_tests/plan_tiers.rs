//! 계획 등급의 회귀 표(17-m6-plan M6-10 ⑨ — 결정 M6-P9). 세션은 승인을 **계획 등급**으로 판정하고 선언 등급은 상한이다. 이
//! 바뀜이 M5 툴의 승인을 넓히지 않는 근거는 「지금 계획을 내는 툴은 모두 계획 등급 = 선언 등급」 이라는 것이다 — 이 표가 그것을
//! 적는다.
//!
//! 강제는 런타임에 있다: 세션은 선언보다 낮은 계획 등급을 `pw_mcp::catalog::LOWERED_PLAN_TIERS` 에 적힌 툴 · 등급만 받고, 그 밖의
//! 낮춤은 툴의 버그로 거절한다(`server` 의 `plan_problem`). 그래서 이미 있는 툴이 코드에서 계획 등급을 낮추면 그 툴의 왕복 시험이
//! 「Pitwall bug」 로 실패한다. 이 표는 그 목록과 카탈로그(이름 · 선언 등급)가 표와 같은지를 본다.
//!
//! 툴 행이 하는 것: 계획을 내는 툴(`Planned` 입력 — `write` 이상 전부와 호출마다 승인을 청하는 `read` 툴)을 더하면 [`PLAN_TIERS`]
//! 에 (선언 등급, 그 툴이 낼 수 있는 계획 등급들)을 이름순 자리에 더한다. 선언보다 낮은 등급을 내는 툴(`run_start` ·
//! `run_restart` — M6-14)은 그 등급을 `LOWERED_PLAN_TIERS` 에도 까닭과 함께 더한다 — 둘이 다르면 실패한다.
//!
//! 이 표가 보지 않는 것: 선언과 **같은** 등급의 계획을 툴이 실제로 내는지는 보지 않는다(낮추면 세션이, 높이면 세션이 거절한다).

use pw_rpc::Tier::{self, Dangerous, Exec, Read, Write};

/// 계획을 내는 툴 전부: (이름, 선언 등급, 낼 수 있는 계획 등급). 이름순.
const PLAN_TIERS: &[(&str, Tier, &[Tier])] = &[
    // 그 줄의 브레이크포인트(17-m6-plan M6-20) — 식이 있는 것은 따로 `exec` 툴이다(메인 판정 M6-P17).
    ("breakpoint_set", Write, &[Write]),
    ("breakpoint_set_expr", Exec, &[Exec]),
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
    ("run_config_delete", Write, &[Write]),
    ("run_config_upsert", Write, &[Write]),
    // 실행 툴(M6-14 ①): 프로세스 구성 `exec`, `sql` 구성은 조회만이면 `read` · 그 밖 `dangerous`.
    ("run_restart", Dangerous, &[Read, Exec, Dangerous]),
    ("run_start", Dangerous, &[Read, Exec, Dangerous]),
    // 끄기는 늘 `exec`.
    ("run_stop", Exec, &[Exec]),
];

#[test]
fn every_planning_tool_is_in_the_plan_tier_table() {
    // 앱 세션과 같은 카탈로그(`Catalog::linked` — 낮춤 목록을 싣는다).
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
    for (name, declared, plans) in PLAN_TIERS {
        assert!(!plans.is_empty(), "{name}: 계획 등급이 없다");
        assert!(
            plans.iter().all(|p| p <= declared),
            "{name}: 선언을 넘는 계획 등급은 세션이 툴의 버그로 거절한다 — 선언을 올리거나 툴을 나눈다"
        );
        let mut below: Vec<Tier> = plans.iter().copied().filter(|p| p < declared).collect();
        below.sort();
        let tool = catalog.get(name).unwrap();
        assert_eq!(
            tool.lowered, below,
            "{name}: 선언보다 낮은 계획 등급은 pw_mcp::catalog::LOWERED_PLAN_TIERS 와 이 표에 같이 적는다"
        );
    }
    // 낮춤 목록의 줄은 모두 앱 카탈로그의 계획 툴이다(링크되지 않은 이름은 카탈로그가 말없이 버린다).
    for (name, _) in pw_mcp::catalog::LOWERED_PLAN_TIERS {
        assert!(
            PLAN_TIERS.iter().any(|(n, ..)| n == name),
            "{name}: 낮춤 목록에 있는데 앱의 계획 툴이 아니다"
        );
    }
}
