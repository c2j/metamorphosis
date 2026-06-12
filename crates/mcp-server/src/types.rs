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

/// Error response body.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}
