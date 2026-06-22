//! Rule: convert full-table DELETE (no WHERE) to TRUNCATE.
//!
//! Conditional level: TRUNCATE has different transaction semantics (cannot
//! be rolled back in some databases, resets auto-increment counters, and
//! cannot be used when foreign keys reference the table unless CASCADE
//! is specified). The engine verifies preconditions before executing.
//!
//! # Safety Guards
//!
//! Only rewrites when ALL of the following are true:
//! - Single target table (no multi-table DELETE)
//! - No WHERE clause
//! - No USING clause
//! - No ORDER BY, LIMIT, or RETURNING

use metamorphosis_core::types::{MatchResult, RewriteAction, RuleCategory, SafetyLevel};
use metamorphosis_core::{RewriteContext, RewriteRule};
use ogsql_parser::ast::{Spanned, Statement, TableRef, TruncateStatement};
use tracing::debug;

/// Rule: convert `DELETE FROM table` (no WHERE) to `TRUNCATE TABLE table`.
///
/// Conditional level: requires precondition verification.
#[derive(Debug)]
pub struct DeleteToTruncate;

impl RewriteRule for DeleteToTruncate {
    fn id(&self) -> &'static str {
        "delete-to-truncate"
    }

    fn description(&self) -> &'static str {
        "Convert full-table DELETE (no WHERE) to TRUNCATE"
    }

    fn category(&self) -> RuleCategory {
        RuleCategory::Performance
    }

    fn safety_level(&self) -> SafetyLevel {
        SafetyLevel::Conditional
    }

    fn matches(&self, _ctx: &RewriteContext, stmt: &Statement) -> MatchResult {
        let delete = match stmt {
            Statement::Delete(d) => &d.node,
            _ => {
                return MatchResult::NotMatched {
                    reason: "Not a DELETE statement".to_string(),
                };
            }
        };

        if delete.where_clause.is_some() {
            return MatchResult::NotMatched {
                reason: "DELETE has a WHERE clause".to_string(),
            };
        }

        if !delete.using.is_empty() {
            return MatchResult::NotMatched {
                reason: "DELETE has a USING clause".to_string(),
            };
        }

        if delete.order_by.is_some() {
            return MatchResult::NotMatched {
                reason: "DELETE has an ORDER BY clause".to_string(),
            };
        }

        if delete.limit.is_some() {
            return MatchResult::NotMatched {
                reason: "DELETE has a LIMIT clause".to_string(),
            };
        }

        if !delete.returning.is_empty() {
            return MatchResult::NotMatched {
                reason: "DELETE has a RETURNING clause".to_string(),
            };
        }

        if delete.tables.len() != 1 {
            return MatchResult::NotMatched {
                reason: format!(
                    "DELETE targets {} table(s); expected exactly 1",
                    delete.tables.len()
                ),
            };
        }

        if !matches!(delete.tables.first(), Some(TableRef::Table { .. })) {
            return MatchResult::NotMatched {
                reason: "DELETE target is not a base table".to_string(),
            };
        }

        // (8) No WITH (CTE).
        if delete.with.is_some() {
            return MatchResult::NotMatched {
                reason: "DELETE has a WITH (CTE) clause".to_string(),
            };
        }

        debug!("DELETE without WHERE detected, eligible for TRUNCATE");
        MatchResult::Matched
    }

    fn apply(&self, _ctx: &RewriteContext, stmt: &Statement) -> Vec<RewriteAction> {
        let delete = match stmt {
            Statement::Delete(d) => &d.node,
            _ => return vec![],
        };

        // Re-check all match conditions (engine may call apply without matches).
        if delete.where_clause.is_some()
            || !delete.using.is_empty()
            || delete.order_by.is_some()
            || delete.limit.is_some()
            || !delete.returning.is_empty()
            || delete.tables.len() != 1
            || delete.with.is_some()
        {
            return vec![];
        }

        let table_name = match delete.tables.first() {
            Some(TableRef::Table { name, .. }) => name.clone(),
            _ => return vec![],
        };

        debug!("Converting DELETE to TRUNCATE");

        let truncate = TruncateStatement {
            tables: vec![table_name],
            cascade: false,
            restart_identity: false,
            continue_identity: false,
        };

        vec![RewriteAction::Replace(Box::new(Statement::Truncate(
            Spanned::without_span(truncate),
        )))]
    }
}
