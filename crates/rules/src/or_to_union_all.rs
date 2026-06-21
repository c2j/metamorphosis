//! Rule: split top-level `WHERE OR` into multiple `UNION ALL` queries.
//!
//! Conditional level: UNION ALL may produce duplicates that `OR` would not.
//! The engine verifies preconditions before executing.
//!
//! # Safety Guards
//!
//! Only rewrites when ALL of the following are true:
//! - WHERE clause has a top-level `Expr::BinaryOp { op: "OR", .. }`
//! - No `DISTINCT`, `GROUP BY`, `HAVING`, `ORDER BY`, `LIMIT`
//! - No existing set operation
//! - Single table in FROM (no JOIN)

use metamorphosis_core::types::{MatchResult, RewriteAction, RuleCategory, SafetyLevel};
use metamorphosis_core::{RewriteContext, RewriteRule};
use ogsql_parser::ast::{Expr, SetOperation, Spanned, Statement, TableRef};
use tracing::debug;

/// Rule: convert `WHERE cond1 OR cond2` to `UNION ALL` of two queries.
///
/// Conditional level: requires precondition verification.
#[derive(Debug)]
pub struct OrToUnionAll;

impl RewriteRule for OrToUnionAll {
    fn id(&self) -> &'static str {
        "or-to-union-all"
    }

    fn description(&self) -> &'static str {
        "Split top-level WHERE OR into multiple UNION ALL queries"
    }

    fn category(&self) -> RuleCategory {
        RuleCategory::Performance
    }

    fn safety_level(&self) -> SafetyLevel {
        SafetyLevel::Conditional
    }

    fn matches(&self, _ctx: &RewriteContext, stmt: &Statement) -> MatchResult {
        let select = match stmt {
            Statement::Select(s) => &s.node,
            _ => {
                return MatchResult::NotMatched {
                    reason: "Not a SELECT statement".to_string(),
                };
            }
        };

        let has_top_or = match &select.where_clause {
            Some(Expr::BinaryOp { op, .. }) if op.eq_ignore_ascii_case("OR") => true,
            Some(_) => false,
            None => {
                return MatchResult::NotMatched {
                    reason: "No WHERE clause".to_string(),
                };
            }
        };

        if !has_top_or {
            return MatchResult::NotMatched {
                reason: "WHERE clause does not have a top-level OR".to_string(),
            };
        }

        if select.distinct {
            return MatchResult::NotMatched {
                reason: "SELECT has DISTINCT — cannot split with UNION ALL".to_string(),
            };
        }

        if !select.group_by.is_empty() {
            return MatchResult::NotMatched {
                reason: "SELECT has GROUP BY".to_string(),
            };
        }

        if select.having.is_some() {
            return MatchResult::NotMatched {
                reason: "SELECT has HAVING".to_string(),
            };
        }

        if !select.order_by.is_empty() {
            return MatchResult::NotMatched {
                reason: "SELECT has ORDER BY".to_string(),
            };
        }

        if select.limit.is_some() {
            return MatchResult::NotMatched {
                reason: "SELECT has LIMIT".to_string(),
            };
        }

        if select.set_operation.is_some() {
            return MatchResult::NotMatched {
                reason: "SELECT already has a set operation".to_string(),
            };
        }

        if select.from.len() != 1 || matches!(select.from.first(), Some(TableRef::Join { .. })) {
            return MatchResult::NotMatched {
                reason: "SELECT has JOIN or multiple tables in FROM".to_string(),
            };
        }

        MatchResult::Matched
    }

    fn apply(&self, _ctx: &RewriteContext, stmt: &Statement) -> Vec<RewriteAction> {
        let spanned = match stmt {
            Statement::Select(s) => s,
            _ => return vec![],
        };

        let (left_cond, right_cond) = match &spanned.node.where_clause {
            Some(Expr::BinaryOp { left, op, right }) if op.eq_ignore_ascii_case("OR") => {
                (left.as_ref().clone(), right.as_ref().clone())
            }
            _ => return vec![],
        };

        let mut left_select = spanned.node.clone();
        left_select.where_clause = Some(left_cond);
        left_select.set_operation = None;

        let mut right_select = spanned.node.clone();
        right_select.where_clause = Some(right_cond);
        right_select.set_operation = None;

        left_select.set_operation = Some(SetOperation::Union {
            all: true,
            right: Box::new(right_select),
        });

        debug!("Split top-level WHERE OR into UNION ALL");

        vec![RewriteAction::Replace(Box::new(Statement::Select(
            Spanned::without_span(left_select),
        )))]
    }
}
