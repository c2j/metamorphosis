//! Rewrite context and configuration.

use ogsql_parser::analyzer::schema::SchemaMap;
use std::collections::HashSet;

/// Configuration for the rewrite engine.
#[derive(Debug, Clone)]
pub struct RewriteConfig {
    /// Rules explicitly enabled (empty = all enabled).
    pub enabled_rules: HashSet<String>,
    /// Rules explicitly disabled.
    pub disabled_rules: HashSet<String>,
    /// Maximum rewrite iterations per statement (loop prevention).
    pub max_iterations: usize,
    /// Whether to preserve comments (requires ogsql-parser trivia support).
    pub preserve_comments: bool,
    /// Default LIMIT for generated probe SQL.
    pub probe_default_limit: usize,
}

impl Default for RewriteConfig {
    fn default() -> Self {
        Self {
            enabled_rules: HashSet::new(),
            disabled_rules: HashSet::new(),
            max_iterations: 10,
            preserve_comments: false,
            probe_default_limit: 10,
        }
    }
}

/// Context provided to each rule during matching and application.
#[derive(Debug, Clone)]
pub struct RewriteContext<'a> {
    /// Database version (for version-gated rules).
    pub version: Option<&'a str>,
    /// Table schema information (for SELECT * expansion, type inference).
    pub schema: Option<&'a SchemaMap>,
    /// User configuration.
    pub config: &'a RewriteConfig,
    /// Source file name for provenance.
    pub source_file: Option<&'a str>,
    /// Known PL/pgSQL variable names extracted from the parent stored procedure.
    /// When present, rules use this set to distinguish variables from table columns
    /// instead of relying solely on table alias heuristics.
    pub known_variables: Option<&'a HashSet<String>>,
}
