//! SQL 언어 서비스(`docs/design/02-tech-stack.md` §2.7). 모듈마다 주인 행이 다르다 — 겹치지 않게 나눠 채운다(14-m3-plan.md
//! 「공유 부품의 주인」).
//!
//! - [`risk`]: 문장 하나의 위험도 분류 `classify_statement`(M3-03, DBX 차용 — 결정 M3-D5). 분류기는 이것 하나다.
//! - [`split`]: 문장 나누기 · 현재 문장, `db.sql.split` · `db.sql.statementAt`(M3-04).
//! - [`edit`]: 그리드 편집의 방언별 DML 과 미리보기 글(M3-12).

pub mod edit;
pub mod risk;
pub mod split;
