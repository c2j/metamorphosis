use metamorphosis_core::types::{MatchResult, RewriteAction, RuleCategory, SafetyLevel};
use metamorphosis_core::{RewriteContext, RewriteRule};
use ogsql_parser::ast::{SetOperation, Spanned, Statement};
use tracing::debug;

/// Rule: Convert `UNION` (without ALL) to `UNION ALL`.
///
/// `UNION` performs implicit deduplication which is expensive. `UNION ALL`
/// skips deduplication and is semantically equivalent when duplicates are
/// not a concern or are known to be absent.
///
/// Safety: Safe — no schema dependency, purely syntactic transformation.
#[derive(Debug)]
pub struct UnionToUnionAll;

impl RewriteRule for UnionToUnionAll {
    fn id(&self) -> &'static str {
        "union-to-union-all"
    }

    fn description(&self) -> &'static str {
        "Convert UNION to UNION ALL for dedup-free set operations"
    }

    fn category(&self) -> RuleCategory {
        RuleCategory::Performance
    }

    fn safety_level(&self) -> SafetyLevel {
        SafetyLevel::Safe
    }

    fn matches(&self, _ctx: &RewriteContext, stmt: &Statement) -> MatchResult {
        match stmt {
            Statement::Select(spanned) => match &spanned.node.set_operation {
                Some(SetOperation::Union { all: false, .. }) => MatchResult::Matched,
                Some(SetOperation::Union { all: true, .. }) => MatchResult::NotMatched {
                    reason: "UNION ALL — already uses UNION ALL, no conversion needed"
                        .to_string(),
                },
                Some(SetOperation::Intersect { .. }) => MatchResult::NotMatched {
                    reason: "INTERSECT — not a UNION operation".to_string(),
                },
                Some(SetOperation::Except { .. }) => MatchResult::NotMatched {
                    reason: "EXCEPT — not a UNION operation".to_string(),
                },
                None => MatchResult::NotMatched {
                    reason: "No set operation present".to_string(),
                },
            },
            other => MatchResult::NotMatched {
                reason: format!("Statement is not a SELECT (got {:?})", other),
            },
        }
    }

    fn apply(&self, _ctx: &RewriteContext, stmt: &Statement) -> Vec<RewriteAction> {
        let spanned = match stmt {
            Statement::Select(s) => s,
            _ => return vec![],
        };

        let mut new_select = spanned.node.clone();

        match &new_select.set_operation {
            Some(SetOperation::Union { all: false, .. }) => {
                debug!("Converting UNION to UNION ALL");
                if let Some(SetOperation::Union { right, .. }) = new_select.set_operation.take() {
                    new_select.set_operation = Some(SetOperation::Union {
                        all: true,
                        right,
                    });
                }
            }
            _ => return vec![],
        }

        vec![RewriteAction::Replace(Box::new(Statement::Select(
            Spanned::without_span(new_select),
        )))]
    }
}
