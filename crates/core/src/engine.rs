//! Rewrite engine that orchestrates rule application.

use crate::context::RewriteContext;
use crate::registry::RuleRegistry;
use crate::types::{
    Confidence, MatchFailure, MatchResult, RewriteAction, RewriteResult, SafetyLevel, Suggestion,
};
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use tracing::debug;

/// The rewrite engine: orchestrates rule matching, application, and loop
/// prevention for a set of SQL statements.
#[derive(Debug)]
pub struct RewriteEngine {
    registry: RuleRegistry,
}

impl RewriteEngine {
    /// Create a new engine with the given rule registry.
    pub fn new(registry: RuleRegistry) -> Self {
        Self { registry }
    }

    /// Rewrite a list of statements by applying all matching rules.
    ///
    /// For each statement, the engine iterates up to `max_iterations` times,
    /// applying Safe/Conditional rules first, then collecting Manual suggestions.
    /// After each replacement, matching restarts from the top (priority order).
    pub fn rewrite(&self, ctx: &RewriteContext, stmts: Vec<Statement>) -> RewriteResult {
        let mut result = Vec::with_capacity(stmts.len());
        let mut all_suggestions = Vec::new();
        let mut any_changed = false;
        let mut all_failures = Vec::new();

        for stmt in stmts {
            let (rewritten, suggestions, failures, changed) = self.rewrite_one(ctx, stmt);
            result.push(rewritten);
            all_suggestions.extend(suggestions);
            all_failures.extend(failures);
            if changed {
                any_changed = true;
            }
        }

        RewriteResult {
            statements: result,
            suggestions: all_suggestions,
            changed: any_changed,
            match_failures: all_failures,
        }
    }

    /// Rewrite a single statement with loop prevention.
    fn rewrite_one(
        &self,
        ctx: &RewriteContext,
        mut stmt: Statement,
    ) -> (Statement, Vec<Suggestion>, Vec<MatchFailure>, bool) {
        let rules = self.registry.filtered_rules(ctx);
        let mut suggestions = Vec::new();
        let mut match_failures = Vec::new();
        let mut iteration = 0;
        let mut changed = false;

        let (auto_rules, manual_rules): (Vec<_>, Vec<_>) = rules.into_iter().partition(|r| {
            matches!(
                r.safety_level(),
                SafetyLevel::Safe | SafetyLevel::Conditional
            )
        });

        loop {
            let mut iteration_changed = false;
            iteration += 1;

            for rule in &auto_rules {
                match rule.matches(ctx, &stmt) {
                    MatchResult::Matched => {
                        let actions = rule.apply(ctx, &stmt);
                        for action in actions {
                            if let RewriteAction::Replace(new_stmt) = action {
                                if validate_statement(&new_stmt) {
                                    stmt = *new_stmt;
                                    iteration_changed = true;
                                    changed = true;
                                    debug!(
                                        rule_id = rule.id(),
                                        iteration = iteration,
                                        "Safe rewrite applied"
                                    );
                                    break;
                                }
                            }
                        }
                        if iteration_changed {
                            break;
                        }
                    }
                    MatchResult::NotMatched { reason } => {
                        if iteration == 1 {
                            match_failures.push(MatchFailure {
                                rule_id: rule.id().to_string(),
                                reason,
                            });
                        }
                    }
                }
            }

            if !iteration_changed {
                break;
            }
            if iteration >= ctx.config.max_iterations {
                debug!(
                    max_iterations = ctx.config.max_iterations,
                    "Rewrite loop: max iterations reached"
                );
                break;
            }
        }

        for rule in &manual_rules {
            match rule.matches(ctx, &stmt) {
                MatchResult::Matched => {
                    let actions = rule.apply(ctx, &stmt);
                    for action in actions {
                        suggestions.push(Suggestion {
                            rule_id: rule.id().to_string(),
                            rule_description: rule.description().to_string(),
                            action,
                            confidence: Confidence::High,
                            notes: Vec::new(),
                        });
                    }
                }
                MatchResult::NotMatched { reason } => {
                    match_failures.push(MatchFailure {
                        rule_id: rule.id().to_string(),
                        reason,
                    });
                }
            }
        }

        (stmt, suggestions, match_failures, changed)
    }
}

/// Validate that a rewritten statement can be formatted and re-parsed.
fn validate_statement(stmt: &Statement) -> bool {
    let sql = SqlFormatter::new().format_statement(stmt);
    let (parsed, errors) = ogsql_parser::Parser::parse_sql(&sql);
    !parsed.is_empty()
        && errors.iter().all(|e| {
            use ogsql_parser::parser::ParserError;
            matches!(
                e,
                ParserError::UnexpectedToken { .. }
                    | ParserError::UnexpectedEof { .. }
                    | ParserError::Warning { .. }
                    | ParserError::ReservedKeywordAsIdentifier { .. }
                    | ParserError::TokenizerError(_)
                    | ParserError::UnsupportedSyntax { .. }
            )
        })
}
