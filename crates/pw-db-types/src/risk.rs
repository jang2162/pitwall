//! 문장 위험도 등급(04 F6-9 의 다섯 갈래, 14-m3-plan.md 결정 M3-D5).
//!
//! 값을 채우는 것은 M3-03 의 분류기 `pw_db_sql::risk::classify_statement` 하나다 — 이 크레이트는 타입만 둔다. 파이프라인(M3-05)이
//! 문장마다 분류한 등급을 [`ExecuteRequest::grade`](crate::ExecuteRequest::grade) 로 드라이버에 넘기고, 드라이버는 그것으로
//! 「커서 버리기」 의 방법을 고른다(조회면 서버에서 멈추고, 조회가 아니면 남은 행을 끝까지 읽어 버린다 — M3-07). 실행 관문의
//! 확인 · 거절 정책(결정 M3-D20)도 이 등급을 쓴다.

use serde::{Deserialize, Serialize};

/// 등급. 순서가 위험도 순서다 — 여러 문장의 합산은 가장 높은 등급(`Ord::max`).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RiskGrade {
    /// 조회 — 데이터를 바꾸지 않는다.
    Read,
    /// 쓰기(키로 한정한 `UPDATE` · `INSERT` · 부작용 함수가 든 `SELECT` · 트랜잭션 제어 등).
    Write,
    /// 범위 없는 쓰기(WHERE 가 없거나 항진식인 `UPDATE` · `DELETE`).
    UnboundedWrite,
    Ddl,
    /// 파괴(`DROP` · `TRUNCATE`). 파서가 읽지 못한 문장도 여기다(까닭 `unparsed`, fail-closed).
    Destructive,
}

impl RiskGrade {
    /// 조회인가. 드라이버는 이것으로 커서 버리기의 방법을 고른다.
    pub fn is_read(self) -> bool {
        self == Self::Read
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grades_order_by_risk_and_use_camel_case() {
        let all = [
            RiskGrade::Read,
            RiskGrade::Write,
            RiskGrade::UnboundedWrite,
            RiskGrade::Ddl,
            RiskGrade::Destructive,
        ];
        assert!(all.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(all.iter().copied().max(), Some(RiskGrade::Destructive));
        assert_eq!(
            serde_json::to_value(RiskGrade::UnboundedWrite).unwrap(),
            "unboundedWrite"
        );
        assert!(RiskGrade::Read.is_read() && !RiskGrade::Write.is_read());
    }
}
