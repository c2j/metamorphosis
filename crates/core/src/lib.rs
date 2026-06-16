//! Metamorphosis — SQL semantic rewriting & data quality probe engine.
//!
//! Consumes AST output from `ogsql-parser` (never parses SQL directly),
//! applies pluggable rewrite rules, and produces diagnostic/rewritten SQL.

pub mod context;
pub mod engine;
pub mod extractor;
pub mod inline;
pub mod registry;
pub mod types;

pub use context::{RewriteConfig, RewriteContext};
pub use engine::RewriteEngine;
pub use inline::{infer_value, inline_statement, InlineParams, InlineResult, InlineValue, RemainingPlaceholder};
pub use registry::{RewriteRule, RuleRegistry};
pub use types::{
    Confidence, MatchFailure, MatchResult, RewriteAction, RewriteResult, RuleCategory, SafetyLevel,
    Severity, Suggestion,
};
