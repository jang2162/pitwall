//! WHERE 가 범위를 보장하지 않는지(항진식 · 서브쿼리 · 열 없는 술어) 본다 — 범위 없는 쓰기([`RiskGrade::UnboundedWrite`])의 판정.
//!
//! 출처: DBX(<https://github.com/t8y2/dbx>) 커밋 `c5e7e0430653aaef198800a48f9dec46a7cfed5e` 의
//! `crates/dbx-sql-core/src/sql_risk.rs` 에서 `predicate_is_obviously_unbounded` 와 그 도우미를 옮겨 수정함(공개 범위 · 이 머리
//! 주석만 바꿨다 — 판정은 같다). Apache License 2.0 — 고지는 `licenses/vendored.md` 「DBX」.
//!
//! 「뻔한」 것만 잡는다: 술어에 서브쿼리가 있거나 열이 없으면 범위 없음, `AND` 는 양쪽 다, `OR` 는 한쪽이거나 서로 보완인 갈래
//! (`x IS NULL OR x IS NOT NULL`, `id = 1 OR id <> 1`, `IN` · `NOT IN`, `BETWEEN` · `NOT BETWEEN`), 비교는 상수끼리의 참이나
//! 같은 식끼리의 `=` · `>=` · `<=` · `<=>`, `LIKE '%'`. 이 밖의 항진식은 범위가 있는 쓰기로 나온다.
//!
//! [`RiskGrade::UnboundedWrite`]: pw_db_types::RiskGrade::UnboundedWrite

use std::ops::ControlFlow;

use sqlparser::ast::{BinaryOperator, Expr, UnaryOperator, Value, visit_expressions};

pub(super) fn predicate_is_obviously_unbounded(expr: &Expr) -> bool {
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
            left,
            op: BinaryOperator::And,
            right,
        } => predicate_is_obviously_unbounded(left) && predicate_is_obviously_unbounded(right),
        Expr::BinaryOp {
            left,
            op: BinaryOperator::Or,
            right,
        } => {
            or_contains_conjunctive_branch(expr)
                || or_contains_complementary_null_checks(expr)
                || or_contains_complementary_comparisons(expr)
                || or_contains_complementary_in_lists(expr)
                || or_contains_complementary_between_checks(expr)
                || predicate_is_obviously_unbounded(left)
                || predicate_is_obviously_unbounded(right)
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
        Expr::IsTrue(expr) | Expr::IsNotFalse(expr) => predicate_is_obviously_unbounded(expr),
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

fn or_contains_conjunctive_branch(expr: &Expr) -> bool {
    match strip_nested_expr(expr) {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::Or,
            right,
        } => or_contains_conjunctive_branch(left) || or_contains_conjunctive_branch(right),
        Expr::BinaryOp {
            op: BinaryOperator::And,
            ..
        } => true,
        _ => false,
    }
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

fn or_contains_complementary_null_checks(expr: &Expr) -> bool {
    let mut checks = Vec::new();
    collect_or_null_checks(expr, &mut checks);
    checks
        .iter()
        .enumerate()
        .any(|(index, (candidate, negated))| {
            checks[index + 1..]
                .iter()
                .any(|(other, other_negated)| candidate == other && negated != other_negated)
        })
}

#[derive(Clone, Copy, PartialEq, Eq)]
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

fn or_contains_complementary_comparisons(expr: &Expr) -> bool {
    let mut checks = Vec::new();
    collect_or_comparison_checks(expr, &mut checks);
    checks.iter().enumerate().any(|(index, candidate)| {
        checks[index + 1..].iter().any(|other| {
            candidate.left == other.left
                && candidate.right == other.right
                && comparison_operators_are_complementary(candidate.operator, other.operator)
        })
    })
}

fn collect_or_comparison_checks<'a>(
    expr: &'a Expr,
    checks: &mut Vec<PredicateComparisonCheck<'a>>,
) {
    let expr = strip_nested_expr(expr);
    if let Expr::BinaryOp {
        left,
        op: BinaryOperator::Or,
        right,
    } = expr
    {
        collect_or_comparison_checks(left, checks);
        collect_or_comparison_checks(right, checks);
    } else if let Some(check) = comparison_check(expr, false) {
        checks.push(check);
    }
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

fn comparison_operators_are_complementary(
    left: PredicateComparisonOperator,
    right: PredicateComparisonOperator,
) -> bool {
    complementary_comparison_operator(left) == right
}

struct PredicateInListCheck<'a> {
    expr: &'a Expr,
    list: Vec<&'a Expr>,
    negated: bool,
}

fn or_contains_complementary_in_lists(expr: &Expr) -> bool {
    let mut checks = Vec::new();
    collect_or_in_list_checks(expr, &mut checks);
    checks.iter().enumerate().any(|(index, candidate)| {
        checks[index + 1..].iter().any(|other| {
            candidate.expr == other.expr
                && candidate.list == other.list
                && candidate.negated != other.negated
        })
    })
}

fn collect_or_in_list_checks<'a>(expr: &'a Expr, checks: &mut Vec<PredicateInListCheck<'a>>) {
    let expr = strip_nested_expr(expr);
    if let Expr::BinaryOp {
        left,
        op: BinaryOperator::Or,
        right,
    } = expr
    {
        collect_or_in_list_checks(left, checks);
        collect_or_in_list_checks(right, checks);
    } else if let Some(check) = in_list_check(expr, false) {
        checks.push(check);
    }
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

fn or_contains_complementary_between_checks(expr: &Expr) -> bool {
    let mut checks = Vec::new();
    collect_or_between_checks(expr, &mut checks);
    checks.iter().enumerate().any(|(index, candidate)| {
        checks[index + 1..].iter().any(|other| {
            candidate.expr == other.expr
                && candidate.low == other.low
                && candidate.high == other.high
                && candidate.negated != other.negated
        })
    })
}

fn collect_or_between_checks<'a>(expr: &'a Expr, checks: &mut Vec<PredicateBetweenCheck<'a>>) {
    let expr = strip_nested_expr(expr);
    if let Expr::BinaryOp {
        left,
        op: BinaryOperator::Or,
        right,
    } = expr
    {
        collect_or_between_checks(left, checks);
        collect_or_between_checks(right, checks);
    } else if let Some(check) = between_check(expr, false) {
        checks.push(check);
    }
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

fn collect_or_null_checks<'a>(expr: &'a Expr, checks: &mut Vec<(&'a Expr, bool)>) {
    let expr = strip_nested_expr(expr);
    if let Expr::BinaryOp {
        left,
        op: BinaryOperator::Or,
        right,
    } = expr
    {
        collect_or_null_checks(left, checks);
        collect_or_null_checks(right, checks);
    } else if let Some(check) = null_check(expr) {
        checks.push(check);
    }
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
            left,
            op: BinaryOperator::And,
            right,
        } => predicate_is_obviously_false(left) || predicate_is_obviously_false(right),
        Expr::BinaryOp {
            left,
            op: BinaryOperator::Or,
            right,
        } => predicate_is_obviously_false(left) && predicate_is_obviously_false(right),
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
