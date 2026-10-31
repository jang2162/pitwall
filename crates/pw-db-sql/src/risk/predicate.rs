//! WHERE 가 범위를 보장하지 않는지(항진식 · 서브쿼리 · 열 없는 술어) 본다 — 범위 없는 쓰기([`RiskGrade::UnboundedWrite`])의 판정.
//!
//! 출처: DBX(<https://github.com/t8y2/dbx>) 커밋 `c5e7e0430653aaef198800a48f9dec46a7cfed5e` 의
//! `crates/dbx-sql-core/src/sql_risk.rs` 에서 `predicate_is_obviously_unbounded` 와 그 도우미를 옮겨 수정함. Apache License 2.0 —
//! 고지는 `licenses/vendored.md` 「DBX」.
//!
//! 「뻔한」 것만 잡는다: 술어에 서브쿼리가 있거나 열이 없으면 범위 없음, `AND` 는 모든 항이, `OR` 는 한 항이거나 서로 보완인 항
//! (`x IS NULL OR x IS NOT NULL`, `id = 1 OR id <> 1`, `IN` · `NOT IN`, `BETWEEN` · `NOT BETWEEN`)이나 `AND` 항이 있으면, 비교는
//! 상수끼리의 참이나 같은 식끼리의 `=` · `>=` · `<=` · `<=>`, `LIKE '%'`. 이 밖의 항진식은 범위가 있는 쓰기로 나온다.
//!
//! DBX 에서 바꾼 것: 판정은 같고 셈법만 바꿨다. DBX 는 `OR` 마디마다 사슬 전체를 다시 모아 두 항씩 견주고(제곱) 그것을 사슬
//! 길이만큼 되풀이해(세제곱) `id = 0 OR … OR id = 1999` 하나에 release 로 11 s 가 걸렸고, 사슬을 따라 재귀해 긴 사슬에서 스택이
//! 깊어졌다. 여기서는 `AND` · `OR` 사슬을 한 번 펴서(재귀 없이) 보완 쌍을 해시로 찾는다. 같은 판정인 까닭: 사슬의 부분 사슬에서
//! 찾을 보완 쌍은 전체 사슬에서도 찾히고, 부분 사슬에 열이 없거나 서브쿼리가 있으면 그 안의 어느 한 항이 이미 그렇다.
//!
//! [`RiskGrade::UnboundedWrite`]: pw_db_types::RiskGrade::UnboundedWrite

use std::collections::HashSet;
use std::ops::ControlFlow;

use sqlparser::ast::{BinaryOperator, Expr, UnaryOperator, Value, visit_expressions};

pub(super) fn predicate_is_obviously_unbounded(expr: &Expr) -> bool {
    // `x IS TRUE IS TRUE …` 의 겹은 열 · 서브쿼리를 더하지 않으므로 먼저 벗긴다(DBX 는 겹마다 재귀하며 술어 전체를 다시 훑었다 —
    // 겹 수의 제곱 시간과 그만큼 깊은 스택).
    let mut expr = expr;
    while let Expr::IsTrue(inner) | Expr::IsNotFalse(inner) = expr {
        expr = inner;
    }
    if expr_contains_subquery(expr) {
        return true;
    }
    if !expr_contains_column_reference(expr) {
        return true;
    }

    match expr {
        Expr::Nested(expr) => predicate_is_obviously_unbounded(expr),
        Expr::Value(value) => value_is_truthy(&value.value),
        Expr::UnaryOp {
            op: UnaryOperator::Not,
            expr,
        } => predicate_is_obviously_false(expr),
        Expr::BinaryOp {
            op: BinaryOperator::And,
            ..
        } => flatten(expr, &BinaryOperator::And)
            .into_iter()
            .all(predicate_is_obviously_unbounded),
        Expr::BinaryOp {
            op: BinaryOperator::Or,
            ..
        } => {
            let disjuncts = flatten(expr, &BinaryOperator::Or);
            disjuncts.iter().any(|disjunct| {
                matches!(
                    disjunct,
                    Expr::BinaryOp {
                        op: BinaryOperator::And,
                        ..
                    }
                )
            }) || contains_complementary_null_checks(&disjuncts)
                || contains_complementary_comparisons(&disjuncts)
                || contains_complementary_in_lists(&disjuncts)
                || contains_complementary_between_checks(&disjuncts)
                || disjuncts.into_iter().any(predicate_is_obviously_unbounded)
        }
        Expr::BinaryOp { left, op, right } if is_comparison_operator(op) => {
            constant_comparison_truth(left, op, right).unwrap_or_else(|| {
                left == right
                    && matches!(
                        op,
                        BinaryOperator::Eq
                            | BinaryOperator::GtEq
                            | BinaryOperator::LtEq
                            | BinaryOperator::Spaceship
                    )
            })
        }
        Expr::IsNotDistinctFrom(left, right) => strip_nested_expr(left) == strip_nested_expr(right),
        Expr::Like {
            negated: false,
            any: false,
            expr,
            pattern,
            escape_char: None,
        }
        | Expr::ILike {
            negated: false,
            any: false,
            expr,
            pattern,
            escape_char: None,
        } => expr_contains_column_reference(expr) && like_pattern_matches_all(pattern),
        _ => false,
    }
}

/// `op` 로 이은 사슬을 괄호를 벗겨 가며 항 목록으로 편다(왼쪽부터). 재귀하지 않는다 — 사슬 길이만큼 스택을 쓰지 않게.
fn flatten<'a>(expr: &'a Expr, op: &BinaryOperator) -> Vec<&'a Expr> {
    let mut terms = Vec::new();
    let mut stack = vec![expr];
    while let Some(expr) = stack.pop() {
        match strip_nested_expr(expr) {
            Expr::BinaryOp {
                left,
                op: this,
                right,
            } if this == op => {
                stack.push(right);
                stack.push(left);
            }
            term => terms.push(term),
        }
    }
    terms
}

fn expr_contains_column_reference(expr: &Expr) -> bool {
    visit_expressions(expr, |expr| {
        let is_column = match expr {
            Expr::Identifier(identifier) => !is_sql_value_keyword(&identifier.value),
            Expr::CompoundIdentifier(_) => true,
            _ => false,
        };
        if is_column {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .is_break()
}

fn is_sql_value_keyword(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "current_catalog"
            | "current_date"
            | "current_role"
            | "current_schema"
            | "current_time"
            | "current_timestamp"
            | "current_user"
            | "localtime"
            | "localtimestamp"
            | "session_user"
            | "system_user"
            | "user"
    )
}

fn expr_contains_subquery(expr: &Expr) -> bool {
    visit_expressions(expr, |expr| {
        if matches!(
            expr,
            Expr::InSubquery { .. } | Expr::Exists { .. } | Expr::Subquery(_)
        ) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .is_break()
}

/// 항마다 (열쇠, 보완 열쇠)를 받아 어떤 항의 열쇠가 앞선 항의 보완 열쇠와 같은지 본다(`x IS NULL` 과 `x IS NOT NULL` 류).
fn contains_complement<K: Eq + std::hash::Hash>(keys: impl Iterator<Item = (K, K)>) -> bool {
    let mut wanted = HashSet::new();
    for (key, complement) in keys {
        if wanted.contains(&key) {
            return true;
        }
        wanted.insert(complement);
    }
    false
}

fn contains_complementary_null_checks(disjuncts: &[&Expr]) -> bool {
    contains_complement(
        disjuncts
            .iter()
            .filter_map(|disjunct| null_check(disjunct))
            .map(|(expr, negated)| ((expr, negated), (expr, !negated))),
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum PredicateComparisonOperator {
    Eq,
    NotEq,
    Gt,
    GtEq,
    Lt,
    LtEq,
}

struct PredicateComparisonCheck<'a> {
    left: &'a Expr,
    right: &'a Expr,
    operator: PredicateComparisonOperator,
}

fn contains_complementary_comparisons(disjuncts: &[&Expr]) -> bool {
    contains_complement(
        disjuncts
            .iter()
            .filter_map(|disjunct| comparison_check(disjunct, false))
            .map(|check| {
                (
                    (check.left, check.right, check.operator),
                    (
                        check.left,
                        check.right,
                        complementary_comparison_operator(check.operator),
                    ),
                )
            }),
    )
}

fn comparison_check(expr: &Expr, outer_negated: bool) -> Option<PredicateComparisonCheck<'_>> {
    let expr = strip_nested_expr(expr);
    if let Expr::UnaryOp {
        op: UnaryOperator::Not,
        expr,
    } = expr
    {
        return comparison_check(expr, !outer_negated);
    }
    let Expr::BinaryOp { left, op, right } = expr else {
        return None;
    };
    if !expr_contains_column_reference(expr) {
        return None;
    }

    let mut operator = predicate_comparison_operator(op)?;
    if outer_negated {
        operator = complementary_comparison_operator(operator);
    }
    let (left, right) = (strip_nested_expr(left), strip_nested_expr(right));
    if left <= right {
        Some(PredicateComparisonCheck {
            left,
            right,
            operator,
        })
    } else {
        Some(PredicateComparisonCheck {
            left: right,
            right: left,
            operator: reverse_comparison_operator(operator),
        })
    }
}

fn predicate_comparison_operator(operator: &BinaryOperator) -> Option<PredicateComparisonOperator> {
    match operator {
        BinaryOperator::Eq | BinaryOperator::Spaceship => Some(PredicateComparisonOperator::Eq),
        BinaryOperator::NotEq => Some(PredicateComparisonOperator::NotEq),
        BinaryOperator::Gt => Some(PredicateComparisonOperator::Gt),
        BinaryOperator::GtEq => Some(PredicateComparisonOperator::GtEq),
        BinaryOperator::Lt => Some(PredicateComparisonOperator::Lt),
        BinaryOperator::LtEq => Some(PredicateComparisonOperator::LtEq),
        _ => None,
    }
}

fn complementary_comparison_operator(
    operator: PredicateComparisonOperator,
) -> PredicateComparisonOperator {
    match operator {
        PredicateComparisonOperator::Eq => PredicateComparisonOperator::NotEq,
        PredicateComparisonOperator::NotEq => PredicateComparisonOperator::Eq,
        PredicateComparisonOperator::Gt => PredicateComparisonOperator::LtEq,
        PredicateComparisonOperator::GtEq => PredicateComparisonOperator::Lt,
        PredicateComparisonOperator::Lt => PredicateComparisonOperator::GtEq,
        PredicateComparisonOperator::LtEq => PredicateComparisonOperator::Gt,
    }
}

fn reverse_comparison_operator(
    operator: PredicateComparisonOperator,
) -> PredicateComparisonOperator {
    match operator {
        PredicateComparisonOperator::Eq | PredicateComparisonOperator::NotEq => operator,
        PredicateComparisonOperator::Gt => PredicateComparisonOperator::Lt,
        PredicateComparisonOperator::GtEq => PredicateComparisonOperator::LtEq,
        PredicateComparisonOperator::Lt => PredicateComparisonOperator::Gt,
        PredicateComparisonOperator::LtEq => PredicateComparisonOperator::GtEq,
    }
}

struct PredicateInListCheck<'a> {
    expr: &'a Expr,
    list: Vec<&'a Expr>,
    negated: bool,
}

fn contains_complementary_in_lists(disjuncts: &[&Expr]) -> bool {
    contains_complement(
        disjuncts
            .iter()
            .filter_map(|disjunct| in_list_check(disjunct, false))
            .map(|check| {
                (
                    (check.expr, check.list.clone(), check.negated),
                    (check.expr, check.list, !check.negated),
                )
            }),
    )
}

fn in_list_check(expr: &Expr, outer_negated: bool) -> Option<PredicateInListCheck<'_>> {
    match strip_nested_expr(expr) {
        Expr::UnaryOp {
            op: UnaryOperator::Not,
            expr,
        } => in_list_check(expr, !outer_negated),
        Expr::InList {
            expr,
            list,
            negated,
        } if expr_contains_column_reference(expr) && !list.is_empty() => {
            let mut normalized_list = list.iter().map(strip_nested_expr).collect::<Vec<_>>();
            if normalized_list
                .iter()
                .all(|item| matches!(strip_nested_expr(item), Expr::Value(_)))
            {
                normalized_list.sort_unstable();
                normalized_list.dedup();
            }
            Some(PredicateInListCheck {
                expr: strip_nested_expr(expr),
                list: normalized_list,
                negated: *negated != outer_negated,
            })
        }
        _ => None,
    }
}

struct PredicateBetweenCheck<'a> {
    expr: &'a Expr,
    low: &'a Expr,
    high: &'a Expr,
    negated: bool,
}

fn contains_complementary_between_checks(disjuncts: &[&Expr]) -> bool {
    contains_complement(
        disjuncts
            .iter()
            .filter_map(|disjunct| between_check(disjunct, false))
            .map(|check| {
                (
                    (check.expr, check.low, check.high, check.negated),
                    (check.expr, check.low, check.high, !check.negated),
                )
            }),
    )
}

fn between_check(expr: &Expr, outer_negated: bool) -> Option<PredicateBetweenCheck<'_>> {
    match strip_nested_expr(expr) {
        Expr::UnaryOp {
            op: UnaryOperator::Not,
            expr,
        } => between_check(expr, !outer_negated),
        Expr::Between {
            expr,
            negated,
            low,
            high,
        } if expr_contains_column_reference(expr) => Some(PredicateBetweenCheck {
            expr: strip_nested_expr(expr),
            low: strip_nested_expr(low),
            high: strip_nested_expr(high),
            negated: *negated != outer_negated,
        }),
        _ => None,
    }
}

fn like_pattern_matches_all(expr: &Expr) -> bool {
    matches!(strip_nested_expr(expr), Expr::Value(value) if matches!(&value.value, Value::SingleQuotedString(pattern) if !pattern.is_empty() && pattern.chars().all(|character| character == '%')))
}

fn null_check(expr: &Expr) -> Option<(&Expr, bool)> {
    null_check_with_negation(expr, false)
}

fn null_check_with_negation(expr: &Expr, outer_negated: bool) -> Option<(&Expr, bool)> {
    match strip_nested_expr(expr) {
        Expr::UnaryOp {
            op: UnaryOperator::Not,
            expr,
        } => null_check_with_negation(expr, !outer_negated),
        Expr::IsNull(expr) => Some((strip_nested_expr(expr), outer_negated)),
        Expr::IsNotNull(expr) => Some((strip_nested_expr(expr), !outer_negated)),
        _ => None,
    }
}

fn strip_nested_expr(mut expr: &Expr) -> &Expr {
    while let Expr::Nested(inner) = expr {
        expr = inner;
    }
    expr
}

fn predicate_is_obviously_false(expr: &Expr) -> bool {
    match expr {
        Expr::Nested(expr) => predicate_is_obviously_false(expr),
        Expr::Value(value) => value_is_falsy(&value.value),
        Expr::UnaryOp {
            op: UnaryOperator::Not,
            expr,
        } => predicate_is_obviously_unbounded(expr),
        Expr::BinaryOp {
            op: BinaryOperator::And,
            ..
        } => flatten(expr, &BinaryOperator::And)
            .into_iter()
            .any(predicate_is_obviously_false),
        Expr::BinaryOp {
            op: BinaryOperator::Or,
            ..
        } => flatten(expr, &BinaryOperator::Or)
            .into_iter()
            .all(predicate_is_obviously_false),
        Expr::BinaryOp { left, op, right } if is_comparison_operator(op) => {
            constant_comparison_truth(left, op, right).is_some_and(|result| !result)
                || (left == right
                    && matches!(
                        op,
                        BinaryOperator::NotEq | BinaryOperator::Gt | BinaryOperator::Lt
                    ))
        }
        Expr::IsDistinctFrom(left, right) => strip_nested_expr(left) == strip_nested_expr(right),
        Expr::IsFalse(expr) | Expr::IsNotTrue(expr) => predicate_is_obviously_unbounded(expr),
        _ => false,
    }
}

fn is_comparison_operator(operator: &BinaryOperator) -> bool {
    matches!(
        operator,
        BinaryOperator::Eq
            | BinaryOperator::NotEq
            | BinaryOperator::Gt
            | BinaryOperator::Lt
            | BinaryOperator::GtEq
            | BinaryOperator::LtEq
            | BinaryOperator::Spaceship
    )
}

fn constant_comparison_truth(left: &Expr, operator: &BinaryOperator, right: &Expr) -> Option<bool> {
    let (Expr::Value(left), Expr::Value(right)) = (left, right) else {
        return None;
    };
    match operator {
        BinaryOperator::Eq | BinaryOperator::Spaceship => Some(left.value == right.value),
        BinaryOperator::NotEq => Some(left.value != right.value),
        BinaryOperator::Gt | BinaryOperator::Lt | BinaryOperator::GtEq | BinaryOperator::LtEq => {
            let (Value::Number(left, _), Value::Number(right, _)) = (&left.value, &right.value)
            else {
                return None;
            };
            let left = left.parse::<f64>().ok()?;
            let right = right.parse::<f64>().ok()?;
            Some(match operator {
                BinaryOperator::Gt => left > right,
                BinaryOperator::Lt => left < right,
                BinaryOperator::GtEq => left >= right,
                BinaryOperator::LtEq => left <= right,
                _ => unreachable!(),
            })
        }
        _ => None,
    }
}

fn value_is_truthy(value: &Value) -> bool {
    match value {
        Value::Boolean(value) => *value,
        Value::Number(value, _) => value.parse::<f64>().is_ok_and(|number| number != 0.0),
        _ => false,
    }
}

fn value_is_falsy(value: &Value) -> bool {
    match value {
        Value::Boolean(value) => !*value,
        Value::Number(value, _) => value.parse::<f64>().is_ok_and(|number| number == 0.0),
        Value::Null => true,
        _ => false,
    }
}
