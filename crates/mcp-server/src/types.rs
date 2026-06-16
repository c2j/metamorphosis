//! MCP tool parameter and response types.
//!
//! All parameter types derive `Deserialize` + `JsonSchema` for automatic
//! MCP tool input schema generation. Response types derive `Serialize`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters for SQL rewriting and suggestion tools.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SqlParams {
    pub sql: String,
    pub version: Option<String>,
    pub schema_json: Option<String>,
    pub schema_path: Option<String>,
    pub sql_dir: Option<String>,
    pub rules: Option<String>,
}

/// Parameters for semantic equivalence verification.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct VerifyParams {
    pub original_sql: String,
    pub rewritten_sql: String,
    pub engine: Option<String>,
    pub bound: Option<usize>,
    pub schema_json: Option<String>,
    pub schema_path: Option<String>,
    pub sql_dir: Option<String>,
}

/// Parameters for schema extraction.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExtractSchemaParams {
    pub sql_dir: String,
}

/// Response from the rewrite tool.
#[derive(Debug, Serialize)]
pub struct RewriteResponse {
    pub changed: bool,
    pub rewritten_sql: Vec<String>,
    pub match_failures: Vec<MatchFailureInfo>,
    pub warnings: Vec<String>,
}

/// Response from the suggest tool.
#[derive(Debug, Serialize)]
pub struct SuggestResponse {
    pub suggestions: Vec<SuggestionInfo>,
    pub match_failures: Vec<MatchFailureInfo>,
    pub warnings: Vec<String>,
}

/// A single suggestion or probe from a Manual or generate rule.
#[derive(Debug, Serialize)]
pub struct SuggestionInfo {
    pub rule_id: String,
    pub rule_description: String,
    pub confidence: String,
    pub probe_sql: Option<String>,
    pub message: Option<String>,
    pub purpose: Option<String>,
}

/// Reason why a rule did not match.
#[derive(Debug, Serialize)]
pub struct MatchFailureInfo {
    pub rule_id: String,
    pub reason: String,
}

/// Response from the list-rules tool.
#[derive(Debug, Serialize)]
pub struct ListRulesResponse {
    pub rules: Vec<RuleInfo>,
}

/// Metadata for a single rewrite rule.
#[derive(Debug, Serialize)]
pub struct RuleInfo {
    pub id: String,
    pub description: String,
    pub category: String,
    pub safety_level: String,
    pub default_enabled: bool,
}

/// Response from the verify tool.
#[derive(Debug, Serialize)]
pub struct VerifyResponse {
    pub result: String,
    pub engine: String,
    pub original_sql: String,
    pub rewritten_sql: String,
    pub elapsed_ms: Option<u64>,
    pub bound: Option<usize>,
    pub counterexample: Option<String>,
    pub column_details: Option<serde_json::Value>,
}

/// Response from the extract-schema tool.
#[derive(Debug, Serialize)]
pub struct ExtractSchemaResponse {
    pub table_count: usize,
    pub schema: serde_json::Value,
}

/// Parameters for the inline_sql MCP tool.
///
/// Replaces parameter placeholders in SQL with literal values to produce
/// directly executable SQL. Supports named parameters (MyBatis #{name},
/// stored procedure variables), positional parameters (JDBC ?), and
/// numbered parameters ($1, $2).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct InlineSqlParams {
    /// SQL text (supports multiple statements).
    pub sql: String,
    /// Named parameters: {"status": "active", "count": 42, "flag": true, "note": null}.
    #[serde(default)]
    pub named: std::collections::HashMap<String, serde_json::Value>,
    /// Positional parameters for JDBC ? (in order): ["en", 1, null, true].
    #[serde(default)]
    pub positional: Vec<serde_json::Value>,
    /// Enable MyBatis #{name} / ${name} parsing.
    #[serde(default)]
    pub mybatis: bool,
    /// Known variable names for stored procedure mode (distinguishes variables
    /// from column names). Required to safely replace bare identifiers
    /// (Expr::ColumnRef) that are actually PL/pgSQL variables.
    pub known_variables: Option<Vec<String>>,
}

/// Response from the inline_sql tool.
#[derive(Debug, Serialize)]
pub struct InlineResponse {
    /// Inlined SQL statements (one per input statement).
    pub inlined_sql: Vec<String>,
    /// Total number of parameters replaced across all statements.
    pub total_replaced: usize,
    /// Placeholders that were NOT replaced (no matching parameter value).
    pub remaining_placeholders: Vec<RemainingPlaceholderInfo>,
    /// Parse warnings.
    pub warnings: Vec<String>,
}

/// Information about a placeholder that was not replaced.
#[derive(Debug, Serialize)]
pub struct RemainingPlaceholderInfo {
    /// Kind: "jdbc" | "mybatis" | "parameter" | "variable".
    pub kind: String,
    /// Parameter name (for named params) or None (for positional).
    pub name: Option<String>,
    /// Position index (for positional params).
    pub position: Option<usize>,
    /// Which statement (0-indexed) this placeholder belongs to.
    pub statement_index: usize,
}

/// Error response body.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}
