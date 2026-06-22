//! Rule: warn on UPDATE or DELETE statements without a WHERE clause.
//!
//! Safety level: Manual — this rule never rewrites SQL, only generates a
//! critical suggestion indicating that the statement will affect all rows.

use metamorphosis_core::types::{MatchResult, RewriteAction, RuleCategory, SafetyLevel, Severity};
use metamorphosis_core::{RewriteContext, RewriteRule};
use ogsql_parser::ast::Statement;
use tracing::debug;

/// Rule: warns when an UPDATE or DELETE has no WHERE clause.
///
/// # Safety
///
/// This is a `Manual`-level rule because it is a safety best-practice check,
/// not a semantic transformation. The rule only produces suggestions and does
/// not modify the AST.
#[derive(Debug)]
pub struct RejectNoWhereDml;

impl RewriteRule for RejectNoWhereDml {
    fn id(&self) -> &'static str {
        "reject-no-where-dml"
    }

    fn description(&self) -> &'static str {
        "Warn on UPDATE/DELETE without WHERE clause"
    }

    fn category(&self) -> RuleCategory {
        RuleCategory::Safety
    }

    fn safety_level(&self) -> SafetyLevel {
        SafetyLevel::Manual
    }

    fn matches(&self, _ctx: &RewriteContext, stmt: &Statement) -> MatchResult {
        match stmt {
            Statement::Delete(delete) if delete.node.where_clause.is_none() => {
                debug!("DELETE without WHERE clause detected");
                MatchResult::Matched
            }
            Statement::Update(update) if update.node.where_clause.is_none() => {
                debug!("UPDATE without WHERE clause detected");
                MatchResult::Matched
            }
            Statement::Truncate(_) => {
                debug!("TRUNCATE detected (converted from DELETE without WHERE)");
                MatchResult::Matched
            }
            Statement::Delete(_) => MatchResult::NotMatched {
                reason: "DELETE has a WHERE clause".to_string(),
            },
            Statement::Update(_) => MatchResult::NotMatched {
                reason: "UPDATE has a WHERE clause".to_string(),
            },
            _ => MatchResult::NotMatched {
                reason: "Statement is not a DELETE or UPDATE".to_string(),
            },
        }
    }

    fn apply(&self, _ctx: &RewriteContext, stmt: &Statement) -> Vec<RewriteAction> {
        let message = match stmt {
            Statement::Delete(_) => {
                "DELETE without WHERE clause will affect all rows".to_string()
            }
            Statement::Update(_) => {
                "UPDATE without WHERE clause will affect all rows".to_string()
            }
            Statement::Truncate(_) => {
                "TRUNCATE removes all rows without WHERE — operation is non-transactional and resets identity counters"
                    .to_string()
            }
            _ => return vec![],
        };

        vec![RewriteAction::Suggest {
            message,
            severity: Severity::Critical,
        }]
    }
}
