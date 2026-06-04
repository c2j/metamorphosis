//! Rule trait, category enum, and registry.

use crate::context::RewriteContext;
use crate::types::{RewriteAction, RuleCategory, SafetyLevel};
use ogsql_parser::ast::Statement;
use std::fmt::Debug;

/// Trait that every rewrite rule must implement.
///
/// Engine dispatches via trait object — supports runtime registration.
pub trait RewriteRule: Debug + Send + Sync {
    /// Unique rule identifier, e.g., "eliminate-select-star".
    fn id(&self) -> &'static str;

    /// Human-readable description.
    fn description(&self) -> &'static str;

    /// Category for UI grouping and permission control.
    fn category(&self) -> RuleCategory;

    /// Whether this rule is enabled by default.
    fn default_enabled(&self) -> bool {
        true
    }

    /// Safety level: determines how the engine handles matched results.
    fn safety_level(&self) -> SafetyLevel;

    /// Check whether this rule applies to the given statement.
    fn matches(&self, ctx: &RewriteContext, stmt: &Statement) -> bool;

    /// Execute the rewrite, returning an action if the rule matched.
    fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Option<RewriteAction>;
}

/// Registry holding all available rules from multiple sources.
#[derive(Debug, Default)]
pub struct RuleRegistry {
    builtin: Vec<Box<dyn RewriteRule>>,
}

impl RuleRegistry {
    /// Create a new registry with the given built-in rules.
    pub fn new(rules: Vec<Box<dyn RewriteRule>>) -> Self {
        Self { builtin: rules }
    }

    /// Return all registered rules.
    pub fn all_rules(&self) -> &[Box<dyn RewriteRule>] {
        &self.builtin
    }

    /// Return rules filtered by version compatibility and config.
    pub fn filtered_rules<'a>(&'a self, ctx: &RewriteContext) -> Vec<&'a dyn RewriteRule> {
        self.builtin
            .iter()
            .filter(|r| {
                let enabled = ctx.config.enabled_rules.is_empty()
                    || ctx.config.enabled_rules.contains(r.id());
                let not_disabled = !ctx.config.disabled_rules.contains(r.id());
                enabled && not_disabled && r.default_enabled()
            })
            .map(|r| r.as_ref())
            .collect()
    }
}
