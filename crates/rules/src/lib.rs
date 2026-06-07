//! Built-in rewrite rules for Metamorphosis.
//!
//! Each rule implements `RewriteRule` from `metamorphosis-core`.

pub mod detect_duplicate_eq_keys;
pub mod eliminate_select_star;
mod eq_analyzer;
pub mod extract_candidate_values;
pub mod subquery_to_join;

use metamorphosis_core::RewriteRule;

/// Returns all built-in rules for registration.
pub fn builtin_rules() -> Vec<Box<dyn RewriteRule>> {
    vec![
        Box::new(eliminate_select_star::EliminateSelectStar),
        Box::new(detect_duplicate_eq_keys::DetectDuplicateEqKeys),
        Box::new(extract_candidate_values::ExtractCandidateValues),
        Box::new(subquery_to_join::SubqueryToJoin),
    ]
}
