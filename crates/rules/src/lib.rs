//! Built-in rewrite rules for Metamorphosis.
//!
//! Each rule implements `RewriteRule` from `metamorphosis-core`.

pub mod detect_duplicate_eq_keys;
pub mod eliminate_select_star;
mod eq_analyzer;
pub mod extract_candidate_values;
pub mod subquery_to_join;

pub mod between_to_eq;
pub mod delete_to_truncate;
pub mod nvl_to_case;
pub mod or_to_union_all;
pub mod reject_no_where_dml;
pub mod union_to_union_all;
pub mod probe_data_skew;
pub mod probe_join_integrity;
pub mod probe_null_ratio;
pub mod probe_param_range;

use metamorphosis_core::RewriteRule;

/// Returns all built-in rules for registration.
pub fn builtin_rules() -> Vec<Box<dyn RewriteRule>> {
    vec![
        Box::new(eliminate_select_star::EliminateSelectStar),
        Box::new(detect_duplicate_eq_keys::DetectDuplicateEqKeys),
        Box::new(extract_candidate_values::ExtractCandidateValues),
        Box::new(subquery_to_join::SubqueryToJoin),
        Box::new(union_to_union_all::UnionToUnionAll),
        Box::new(between_to_eq::BetweenToEq),
        Box::new(nvl_to_case::NvlToCase),
        Box::new(delete_to_truncate::DeleteToTruncate),
        Box::new(or_to_union_all::OrToUnionAll),
        Box::new(reject_no_where_dml::RejectNoWhereDml),
        Box::new(probe_param_range::ProbeParamRange),
        Box::new(probe_null_ratio::ProbeNullRatio),
        Box::new(probe_data_skew::ProbeDataSkew),
        Box::new(probe_join_integrity::ProbeJoinIntegrity),
    ]
}
