//! Rule: convert `col BETWEEN v AND v` to `col = v` when the low and high
//! bounds are equal.
//!
//! `BETWEEN x AND x` is semantically equivalent to `= x` but may be less
//! efficient (range scan vs point lookup). This rule detects the degenerate
//! case and replaces it with a simple equality comparison.
//!
//! # Safety
//!
//! Safe — the rewrite is always semantically equivalent.
//!
//! # Example
//!
//! ```sql
//! -- Before:
//! SELECT * FROM t WHERE col BETWEEN 5 AND 5;
//!
//! -- After:
//! SELECT * FROM t WHERE col = 5;
//! ```

use metamorphosis_core::types::{MatchResult, RewriteAction, RuleCategory, SafetyLevel};
use metamorphosis_core::{RewriteContext, RewriteRule};
use ogsql_parser::ast::{Expr, Spanned, Statement};
use tracing::debug;

/// Rule: rewrite degenerate `BETWEEN` (where low == high) to `=` equality.
///
/// Matches any `Expr::Between { negated: false, low, high, .. }` where `low == high`
/// inside the WHERE clause of a SELECT statement. Replaces the first occurrence
/// per invocation; the engine re-runs the rule chain after each replacement.
#[derive(Debug)]
pub struct BetweenToEq;

impl RewriteRule for BetweenToEq {
    fn id(&self) -> &'static str {
        "between-to-eq"
    }

    fn description(&self) -> &'static str {
        "Rewrite col BETWEEN v AND v to col = v when low equals high"
    }

    fn category(&self) -> RuleCategory {
        RuleCategory::Semantic
    }

    fn safety_level(&self) -> SafetyLevel {
        SafetyLevel::Safe
    }

    fn matches(&self, _ctx: &RewriteContext, stmt: &Statement) -> MatchResult {
        let select = match stmt {
            Statement::Select(s) => &s.node,
            _ => {
                return MatchResult::NotMatched {
                    reason: "Statement is not a SELECT".to_string(),
                };
            }
        };

        match &select.where_clause {
            Some(where_clause) => {
                if find_degenerate_between(where_clause) {
                    MatchResult::Matched
                } else {
                    MatchResult::NotMatched {
                        reason: "No degenerate BETWEEN (low == high) found in WHERE clause"
                            .to_string(),
                    }
                }
            }
            None => MatchResult::NotMatched {
                reason: "No WHERE clause".to_string(),
            },
        }
    }

    fn apply(&self, _ctx: &RewriteContext, stmt: &Statement) -> Vec<RewriteAction> {
        let spanned = match stmt {
            Statement::Select(s) => s,
            _ => return vec![],
        };

        let select = &spanned.node;

        if let Some(ref where_clause) = select.where_clause {
            if let Some(new_where) = replace_first_degenerate_between(where_clause) {
                let mut new_select = select.clone();
                new_select.where_clause = Some(new_where);

                debug!("Replaced degenerate BETWEEN (low == high) with = equality");

                return vec![RewriteAction::Replace(Box::new(Statement::Select(
                    Spanned::without_span(new_select),
                )))];
            }
        }

        vec![]
    }
}

/// Recursively checks whether the expression tree contains at least one
/// `BETWEEN` where `low == high` and `negated` is false.
///
/// Handles the common expression nesting cases: `BinaryOp`, `UnaryOp`,
/// `Parenthesized`, and `IsNull`.
fn find_degenerate_between(expr: &Expr) -> bool {
    match expr {
        Expr::Between {
            negated: false,
            low,
            high,
            ..
        } if low == high => true,
        Expr::Between { .. } => false,
        Expr::BinaryOp { left, right, .. } => {
            find_degenerate_between(left) || find_degenerate_between(right)
        }
        Expr::UnaryOp { expr: inner, .. } => find_degenerate_between(inner),
        Expr::Parenthesized(inner) => find_degenerate_between(inner),
        Expr::IsNull { expr: inner, .. } => find_degenerate_between(inner),
        _ => false,
    }
}

/// Recursively replaces the first `Expr::Between { negated: false, low, high, expr }`
/// where `low == high` with `Expr::BinaryOp { left: expr, op: "=", right: low }`.
///
/// Returns `Some(replaced)` if a match was found and replaced, `None` otherwise.
/// Traverses in pre-order (left before right in `BinaryOp`), matching the engine's
/// one-replacement-per-iteration contract.
fn replace_first_degenerate_between(expr: &Expr) -> Option<Expr> {
    match expr {
        Expr::Between {
            expr: between_expr,
            low,
            high,
            negated: false,
        } if low == high => Some(Expr::BinaryOp {
            left: between_expr.clone(),
            op: "=".to_string(),
            right: low.clone(),
        }),
        Expr::Between { .. } => None,
        Expr::BinaryOp { left, right, op } => {
            if let Some(replaced) = replace_first_degenerate_between(left) {
                Some(Expr::BinaryOp {
                    left: Box::new(replaced),
                    op: op.clone(),
                    right: right.clone(),
                })
            } else if let Some(replaced) = replace_first_degenerate_between(right) {
                Some(Expr::BinaryOp {
                    left: left.clone(),
                    op: op.clone(),
                    right: Box::new(replaced),
                })
            } else {
                None
            }
        }
        Expr::UnaryOp {
            op: unary_op,
            expr: inner,
        } => replace_first_degenerate_between(inner).map(|replaced| Expr::UnaryOp {
            op: unary_op.clone(),
            expr: Box::new(replaced),
        }),
        Expr::Parenthesized(inner) => {
            replace_first_degenerate_between(inner)
                .map(|replaced| Expr::Parenthesized(Box::new(replaced)))
        }
        Expr::IsNull {
            expr: inner,
            negated,
        } => replace_first_degenerate_between(inner).map(|replaced| Expr::IsNull {
            expr: Box::new(replaced),
            negated: *negated,
        }),
        _ => None,
    }
}
