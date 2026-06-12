# Metamorphosis MCP Server Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add MCP (Model Context Protocol) server support so AI assistants can call metamorphosis's SQL rewriting, suggestion, verification, and schema extraction capabilities as MCP tools via stdio transport.

**Architecture:** New `metamorphosis-mcp` crate at Layer 4 (sibling to `cli`). Uses `rmcp` v0.16.0 SDK with macro-based tool registration. Five tools map directly to existing engine APIs. The existing `cli` crate gains a `mcp` subcommand that delegates to this library.

**Tech Stack:** Rust 2021, `rmcp` 0.16 (official MCP Rust SDK), `tokio` 1.x, `serde`/`serde_json`, `schemars`, existing metamorphosis crates (`core`, `rules`, `qed`, `verieql`).

**Design Decisions (confirmed by user):**
- Crate name: `metamorphosis-mcp`
- Transport: stdio only (MVP)
- All 5 tools: rewrite, suggest, list_rules, verify, extract_schema
- Schema input: both inline JSON string AND file path supported

---

### Task 1: Create Crate Skeleton

**Files:**
- Create: `crates/mcp-server/Cargo.toml`
- Create: `crates/mcp-server/src/lib.rs`
- Modify: `Cargo.toml` (add workspace member)

**Step 1: Create `crates/mcp-server/Cargo.toml`**

```toml
[package]
name = "metamorphosis-mcp"
version = "0.1.18"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
metamorphosis-core = { path = "../core" }
metamorphosis-rules = { path = "../rules" }
metamorphosis-qed = { path = "../qed" }
metamorphosis-verieql = { path = "../verieql" }
ogsql-parser = { git = "https://github.com/c2j/ogsql-parser" }
rmcp = { version = "0.16", features = ["server", "macros", "schemars", "transport-io"] }
tokio = { version = "1", features = ["io-std", "rt", "macros"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "1"
tracing = "0.1"
```

Note: `rmcp` feature flags `server` + `macros` + `schemars` + `transport-io` are needed. The stdio transport is at `rmcp::transport::io::stdio` — if the path differs in your rmcp version, check `rmcp::transport` module for re-exports.

**Step 2: Create `crates/mcp-server/src/lib.rs`**

```rust
//! Metamorphosis MCP Server — Model Context Protocol integration.
//!
//! Exposes metamorphosis SQL rewriting, suggestion, verification,
//! and schema extraction as MCP tools over stdio transport.

pub mod server;
pub mod tools;
pub mod types;

pub use server::run_stdio;
```

**Step 3: Add workspace member**

In the root `Cargo.toml`, add `"crates/mcp-server"` to the `members` array:

```toml
[workspace]
members = ["crates/core", "crates/rules", "crates/cli", "crates/qed", "crates/verieql", "crates/mcp-server"]
resolver = "2"
```

**Step 4: Create stub modules**

Create empty stubs so the crate compiles:

`crates/mcp-server/src/types.rs`:
```rust
//! MCP tool parameter and response types.
```

`crates/mcp-server/src/tools.rs`:
```rust
//! Tool implementations.
```

`crates/mcp-server/src/server.rs`:
```rust
//! MCP server handler and transport.

pub async fn run_stdio() -> Result<(), Box<dyn std::error::Error>> {
    todo!("implemented in Task 8")
}
```

**Step 5: Verify it compiles**

Run: `cargo check -p metamorphosis-mcp`
Expected: compiles with `todo!()` in server.rs (may get warnings about unused imports — that's fine)

**Step 6: Commit**

```
feat(mcp): add metamorphosis-mcp crate skeleton
```

---

### Task 2: Define Parameter and Response Types

**Files:**
- Create: `crates/mcp-server/src/types.rs` (replace stub)

**Step 1: Write the types**

All parameter types derive `Deserialize` + `JsonSchema` (required by rmcp macros).
All response types derive `Serialize` (returned as JSON text).

```rust
//! MCP tool parameter and response types.

use serde::{Deserialize, Serialize};
use schemars::JsonSchema;

// ── Shared Parameters ──────────────────────────────────────────────────

/// Parameters for SQL rewriting and suggestion tools.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SqlParams {
    /// SQL statement(s) to analyze or rewrite.
    pub sql: String,
    /// GaussDB version string (e.g., "5.0").
    pub version: Option<String>,
    /// Schema as inline JSON string: `{ "table": { "col": "type" } }`.
    /// Mutually exclusive with `schema_path`.
    pub schema_json: Option<String>,
    /// Path to a schema JSON file on disk.
    /// Mutually exclusive with `schema_json`.
    pub schema_path: Option<String>,
    /// Path to a directory of DDL .sql files for schema extraction.
    /// Mutually exclusive with `schema_json` and `schema_path`.
    pub sql_dir: Option<String>,
    /// Comma-separated rule IDs to enable (empty = all).
    pub rules: Option<String>,
}

/// Parameters for semantic equivalence verification.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct VerifyParams {
    /// Original SQL query.
    pub original_sql: String,
    /// Rewritten SQL query.
    pub rewritten_sql: String,
    /// Verification engine: "qed" (default) or "verieql".
    pub engine: Option<String>,
    /// Bound for VeriEQL bounded verification (default: 2).
    pub bound: Option<usize>,
    /// Schema as inline JSON string.
    pub schema_json: Option<String>,
    /// Path to a schema JSON file on disk.
    pub schema_path: Option<String>,
    /// Path to a directory of DDL .sql files.
    pub sql_dir: Option<String>,
}

/// Parameters for schema extraction.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExtractSchemaParams {
    /// Path to a directory containing DDL .sql files.
    pub sql_dir: String,
}

// ── Response Types ─────────────────────────────────────────────────────

/// Response for `rewrite_sql` tool.
#[derive(Debug, Serialize)]
pub struct RewriteResponse {
    /// Whether any rewrite was applied.
    pub changed: bool,
    /// List of rewritten SQL statements (formatted).
    pub rewritten_sql: Vec<String>,
    /// List of rules that were checked but did not match.
    pub match_failures: Vec<MatchFailureInfo>,
    /// Parse warnings encountered.
    pub warnings: Vec<String>,
}

/// Response for `suggest_probes` tool.
#[derive(Debug, Serialize)]
pub struct SuggestResponse {
    /// Generated suggestions (probe SQL).
    pub suggestions: Vec<SuggestionInfo>,
    /// Rules checked but not matched.
    pub match_failures: Vec<MatchFailureInfo>,
    /// Parse warnings.
    pub warnings: Vec<String>,
}

/// A single suggestion from a Manual-level rule.
#[derive(Debug, Serialize)]
pub struct SuggestionInfo {
    /// Rule that generated this suggestion.
    pub rule_id: String,
    /// Human-readable rule description.
    pub rule_description: String,
    /// Confidence level: "High", "Medium", or "Low".
    pub confidence: String,
    /// Generated probe SQL (formatted).
    pub probe_sql: Option<String>,
    /// Text suggestion message (if not a probe).
    pub message: Option<String>,
    /// Purpose of the generated SQL.
    pub purpose: Option<String>,
}

/// Why a rule did not match.
#[derive(Debug, Serialize)]
pub struct MatchFailureInfo {
    pub rule_id: String,
    pub reason: String,
}

/// Response for `list_rules` tool.
#[derive(Debug, Serialize)]
pub struct ListRulesResponse {
    pub rules: Vec<RuleInfo>,
}

/// Metadata about a single rewrite rule.
#[derive(Debug, Serialize)]
pub struct RuleInfo {
    pub id: String,
    pub description: String,
    pub category: String,
    pub safety_level: String,
    pub default_enabled: bool,
}

/// Response for `verify_equivalence` tool.
#[derive(Debug, Serialize)]
pub struct VerifyResponse {
    /// "Equivalent", "NotEquivalent", "Unknown", or "Timeout".
    pub result: String,
    /// Engine used: "qed" or "verieql".
    pub engine: String,
    /// Original SQL.
    pub original_sql: String,
    /// Rewritten SQL.
    pub rewritten_sql: String,
    /// Elapsed time in milliseconds (Qed only).
    pub elapsed_ms: Option<u64>,
    /// Bound used (VeriEQL only).
    pub bound: Option<usize>,
    /// Counterexample details (if not equivalent).
    pub counterexample: Option<String>,
    /// Column mismatch details (Qed only).
    pub column_details: Option<serde_json::Value>,
}

/// Response for `extract_schema` tool.
#[derive(Debug, Serialize)]
pub struct ExtractSchemaResponse {
    /// Number of tables extracted.
    pub table_count: usize,
    /// Schema map: table name → column name → type string.
    pub schema: serde_json::Value,
}

/// Error response (used by all tools).
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}
```

**Step 2: Verify it compiles**

Run: `cargo check -p metamorphosis-mcp`
Expected: PASS

**Step 3: Commit**

```
feat(mcp): add tool parameter and response types
```

---

### Task 3: Implement Helper Functions (Schema Loading + Engine Construction)

**Files:**
- Create: `crates/mcp-server/src/tools.rs` (replace stub with helpers + tool implementations)

This task establishes the shared helpers that all tools use, then implements the simplest tool (`list_rules`) to validate the pattern.

**Step 1: Write the helper functions and `list_rules`**

```rust
//! Tool implementations for the MCP server.

use std::collections::HashSet;
use std::path::Path;

use metamorphosis_core::extractor::extract_schema_from_dir;
use metamorphosis_core::types::RewriteAction;
use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry};
use metamorphosis_rules::builtin_rules;
use ogsql_parser::analyzer::schema::SchemaMap;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::{ParseOptions, Parser};

use crate::types::*;

// ── Helpers ─────────────────────────────────────────────────────────────

/// Load schema from one of three sources (inline JSON, file path, or DDL directory).
/// Returns `None` if no schema source is provided.
pub fn load_schema(
    schema_json: Option<&str>,
    schema_path: Option<&str>,
    sql_dir: Option<&str>,
) -> Result<Option<SchemaMap>, String> {
    match (schema_json, schema_path, sql_dir) {
        (Some(json), None, None) => {
            let map: SchemaMap =
                serde_json::from_str(json).map_err(|e| format!("invalid schema JSON: {e}"))?;
            Ok(Some(map))
        }
        (None, Some(path), None) => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read schema file '{}': {e}", path))?;
            let map: SchemaMap = serde_json::from_str(&content)
                .map_err(|e| format!("invalid schema JSON in '{}': {e}", path))?;
            Ok(Some(map))
        }
        (None, None, Some(dir)) => {
            let schema = extract_schema_from_dir(Path::new(dir))
                .map_err(|e| format!("schema extraction failed: {e}"))?;
            Ok(Some(schema))
        }
        (None, None, None) => Ok(None),
        _ => Err("schema_json, schema_path, and sql_dir are mutually exclusive".to_string()),
    }
}

/// Build a rewrite engine with optional rule filtering.
pub fn build_engine(rules_opt: Option<&str>) -> RewriteEngine {
    let all_rules = builtin_rules();

    let registry = if let Some(rules_str) = rules_opt {
        let enabled: HashSet<String> = rules_str.split(',').map(|s| s.trim().to_string()).collect();
        let filtered: Vec<Box<dyn metamorphosis_core::RewriteRule>> =
            all_rules.into_iter().filter(|r| enabled.contains(r.id())).collect();
        RuleRegistry::new(filtered)
    } else {
        RuleRegistry::new(all_rules)
    };

    RewriteEngine::new(registry)
}

/// Parse SQL and return (statements, warnings).
pub fn parse_sql(sql: &str) -> (Vec<ogsql_parser::StatementInfo>, Vec<String>) {
    let output = Parser::parse_sql_with_options(
        sql,
        ParseOptions {
            preserve_comments: false,
            mybatis_params: false,
        },
    );
    let warnings: Vec<String> = output.errors.iter().map(|e| format!("{e:?}")).collect();
    (output.statements, warnings)
}

/// Format a statement to a pretty-printed SQL string.
pub fn format_stmt(stmt: &ogsql_parser::ast::Statement) -> String {
    SqlFormatter::new().pretty_print(true).format_statement(stmt)
}

// ── Tool Implementations ────────────────────────────────────────────────

/// List all available rewrite rules.
pub fn list_rules() -> ListRulesResponse {
    let rules = builtin_rules();
    let rule_infos: Vec<RuleInfo> = rules
        .iter()
        .map(|r| RuleInfo {
            id: r.id().to_string(),
            description: r.description().to_string(),
            category: format!("{:?}", r.category()).to_lowercase(),
            safety_level: format!("{:?}", r.safety_level()),
            default_enabled: r.default_enabled(),
        })
        .collect();
    ListRulesResponse { rules: rule_infos }
}

/// Rewrite SQL using Safe/Conditional rules.
pub fn rewrite_sql(params: &SqlParams) -> Result<RewriteResponse, String> {
    let schema = load_schema(
        params.schema_json.as_deref(),
        params.schema_path.as_deref(),
        params.sql_dir.as_deref(),
    )?;

    let engine = build_engine(params.rules.as_deref());
    let config = RewriteConfig::default();
    let ctx = RewriteContext {
        version: params.version.as_deref(),
        schema: schema.as_ref(),
        config: &config,
        source_file: Some("<mcp>"),
        known_variables: None,
    };

    let (stmt_infos, warnings) = parse_sql(&params.sql);
    if stmt_infos.is_empty() {
        return Err("no SQL statements to process".to_string());
    }

    let mut rewritten_sql = Vec::new();
    let mut all_failures = Vec::new();
    let mut any_changed = false;

    for si in &stmt_infos {
        let result = engine.rewrite(&ctx, vec![si.statement.clone()]);
        if result.changed {
            any_changed = true;
            for stmt in &result.statements {
                rewritten_sql.push(format!("{};", format_stmt(stmt)));
            }
        } else {
            rewritten_sql.push(format!("{};", format_stmt(&si.statement)));
        }
        for f in &result.match_failures {
            all_failures.push(MatchFailureInfo {
                rule_id: f.rule_id.clone(),
                reason: f.reason.clone(),
            });
        }
    }

    Ok(RewriteResponse {
        changed: any_changed,
        rewritten_sql,
        match_failures: all_failures,
        warnings,
    })
}

/// Generate suggestions (Manual-level rules only).
pub fn suggest_probes(params: &SqlParams) -> Result<SuggestResponse, String> {
    let schema = load_schema(
        params.schema_json.as_deref(),
        params.schema_path.as_deref(),
        params.sql_dir.as_deref(),
    )?;

    let engine = build_engine(params.rules.as_deref());
    let config = RewriteConfig::default();
    let ctx = RewriteContext {
        version: params.version.as_deref(),
        schema: schema.as_ref(),
        config: &config,
        source_file: Some("<mcp>"),
        known_variables: None,
    };

    let (stmt_infos, warnings) = parse_sql(&params.sql);
    if stmt_infos.is_empty() {
        return Err("no SQL statements to process".to_string());
    }

    let mut all_suggestions = Vec::new();
    let mut all_failures = Vec::new();

    for si in &stmt_infos {
        let result = engine.rewrite(&ctx, vec![si.statement.clone()]);

        for s in &result.suggestions {
            let (probe_sql, message, purpose) = match &s.action {
                RewriteAction::Generate { stmt, purpose, .. } => {
                    (Some(format!("{};", format_stmt(stmt))), None, Some(purpose.clone()))
                }
                RewriteAction::Suggest { message, .. } => {
                    (None, Some(message.clone()), None)
                }
                RewriteAction::Replace(_) => (None, None, None),
            };
            all_suggestions.push(SuggestionInfo {
                rule_id: s.rule_id.clone(),
                rule_description: s.rule_description.clone(),
                confidence: format!("{:?}", s.confidence),
                probe_sql,
                message,
                purpose,
            });
        }

        for f in &result.match_failures {
            all_failures.push(MatchFailureInfo {
                rule_id: f.rule_id.clone(),
                reason: f.reason.clone(),
            });
        }
    }

    Ok(SuggestResponse {
        suggestions: all_suggestions,
        match_failures: all_failures,
        warnings,
    })
}

/// Verify semantic equivalence of two SQL queries.
pub fn verify_equivalence(params: &VerifyParams) -> Result<VerifyResponse, String> {
    let engine_name = params.engine.as_deref().unwrap_or("qed");
    let schema = load_schema(
        params.schema_json.as_deref(),
        params.schema_path.as_deref(),
        params.sql_dir.as_deref(),
    )?;

    // Schema is required for verification
    let schema = schema.ok_or("schema is required for verification (use schema_json, schema_path, or sql_dir)")?;

    match engine_name {
        "qed" => verify_with_qed(&params.original_sql, &params.rewritten_sql, &schema),
        "verieql" => verify_with_verieql(
            &params.original_sql,
            &params.rewritten_sql,
            &schema,
            params.bound.unwrap_or(2),
        ),
        _ => Err(format!("unknown engine '{}', use 'qed' or 'verieql'", engine_name)),
    }
}

fn verify_with_qed(
    original_sql: &str,
    rewritten_sql: &str,
    schema: &SchemaMap,
) -> Result<VerifyResponse, String> {
    use metamorphosis_qed::prover::ProverConfig;
    use metamorphosis_qed::schema::{extract_rich_schema, RichSchema};
    use metamorphosis_qed::verify::verify_rewrite;

    // Parse both SQLs
    let (orig_stmts, orig_warns) = parse_sql(original_sql);
    let (rew_stmts, _) = parse_sql(rewritten_sql);

    let orig_stmt = orig_stmts
        .iter()
        .next()
        .ok_or("original SQL contains no statements")?;
    let rew_stmt = rew_stmts
        .iter()
        .next()
        .ok_or("rewritten SQL contains no statements")?;

    if orig_stmts.len() > 1 || rew_stmts.len() > 1 {
        return Err("verification requires exactly one statement per query".to_string());
    }

    // Convert SchemaMap → DDL → RichSchema
    let ddl = schema_map_to_ddl(schema);
    let (ddl_stmts, _) = parse_sql(&ddl);
    let ddl_ast: Vec<ogsql_parser::ast::Statement> =
        ddl_stmts.iter().map(|si| si.statement.clone()).collect();
    let rich_schema: RichSchema = extract_rich_schema(&ddl_ast);

    let config = ProverConfig::default();
    let result = verify_rewrite("mcp-verify", &orig_stmt.statement, &rew_stmt.statement, &rich_schema, &config)
        .map_err(|e| format!("verification failed: {e}"))?;

    let mut warnings = orig_warns;
    if !result.original_columns.is_none() || !result.rewritten_columns.is_none() {
        // no extra warnings needed
    }

    let (outcome, counterexample, column_details) = match &result.proof {
        metamorphosis_qed::prover::ProofResult::Equivalent => {
            ("Equivalent".to_string(), None, None)
        }
        metamorphosis_qed::prover::ProofResult::NotEquivalent { counterexample } => {
            let mut details = serde_json::Map::new();
            if let Some(orig) = &result.original_columns {
                details.insert("original_columns".to_string(), serde_json::json!(orig));
            }
            if let Some(rew) = &result.rewritten_columns {
                details.insert("rewritten_columns".to_string(), serde_json::json!(rew));
            }
            ("NotEquivalent".to_string(), counterexample.clone(), Some(serde_json::Value::Object(details)))
        }
        metamorphosis_qed::prover::ProofResult::Unknown { reason } => {
            ("Unknown".to_string(), Some(reason.clone()), None)
        }
        metamorphosis_qed::prover::ProofResult::Timeout { seconds } => {
            ("Timeout".to_string(), Some(format!("timed out after {seconds}s")), None)
        }
        _ => ("Unknown".to_string(), None, None),
    };

    Ok(VerifyResponse {
        result: outcome,
        engine: "qed".to_string(),
        original_sql: result.original_sql,
        rewritten_sql: result.rewritten_sql,
        elapsed_ms: Some(result.elapsed_ms),
        bound: None,
        counterexample,
        column_details,
    })
}

fn verify_with_verieql(
    original_sql: &str,
    rewritten_sql: &str,
    schema: &SchemaMap,
    bound: usize,
) -> Result<VerifyResponse, String> {
    use metamorphosis_verieql::types::*;
    use metamorphosis_verieql::VeriEql;

    let table_schemas = schema_map_to_verieql(schema);
    let constraints = serde_json::json!(null);

    let report = VeriEql::verify(
        original_sql,
        rewritten_sql,
        &table_schemas,
        &constraints,
        Bound(bound),
        Semantics::Bag,
    )
    .map_err(|e| format!("VeriEQL verification failed: {e}"))?;

    let (outcome, counterexample) = match &report.result {
        ProofResult::Equivalent => ("Equivalent".to_string(), None),
        ProofResult::NotEquivalent { counterexample } => {
            let ce_str = if counterexample.tables.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&counterexample).unwrap_or_default())
            };
            ("NotEquivalent".to_string(), ce_str)
        }
        ProofResult::Unknown { reason } => ("Unknown".to_string(), Some(reason.clone())),
    };

    Ok(VerifyResponse {
        result: outcome,
        engine: "verieql".to_string(),
        original_sql: original_sql.to_string(),
        rewritten_sql: rewritten_sql.to_string(),
        elapsed_ms: None,
        bound: Some(report.bound.0),
        counterexample,
        column_details: None,
    })
}

/// Extract schema from a DDL directory.
pub fn extract_schema(params: &ExtractSchemaParams) -> Result<ExtractSchemaResponse, String> {
    let schema = extract_schema_from_dir(Path::new(&params.sql_dir))
        .map_err(|e| format!("schema extraction failed: {e}"))?;

    let table_count = schema.len();
    let schema_json = serde_json::to_value(&schema)
        .map_err(|e| format!("schema serialization failed: {e}"))?;

    Ok(ExtractSchemaResponse {
        table_count,
        schema: schema_json,
    })
}

// ── Internal Helpers ────────────────────────────────────────────────────

/// Convert a SchemaMap to DDL statements for QED rich schema extraction.
fn schema_map_to_ddl(schema: &SchemaMap) -> String {
    let mut ddl = String::new();
    for (table_name, columns) in schema {
        ddl.push_str("CREATE TABLE ");
        ddl.push_str(table_name);
        ddl.push_str(" (");
        let col_defs: Vec<String> = columns
            .iter()
            .map(|(name, typ)| format!("{} {}", name, typ.to_uppercase()))
            .collect();
        ddl.push_str(&col_defs.join(", "));
        ddl.push_str(");\n");
    }
    ddl
}

/// Convert a SchemaMap to VeriEQL TableSchema format.
fn schema_map_to_verieql(schema: &SchemaMap) -> Vec<metamorphosis_verieql::types::TableSchema> {
    schema
        .iter()
        .map(|(table_name, columns)| metamorphosis_verieql::types::TableSchema {
            name: table_name.clone(),
            columns: columns
                .iter()
                .map(|(col_name, col_type)| metamorphosis_verieql::types::ColumnDef {
                    name: col_name.clone(),
                    col_type: sql_type_to_verieql(col_type),
                })
                .collect(),
        })
        .collect()
}

/// Map SQL type string to VeriEQL ColumnType.
fn sql_type_to_verieql(ty: &str) -> metamorphosis_verieql::types::ColumnType {
    use metamorphosis_verieql::types::ColumnType;
    match ty.to_uppercase() {
        t if t.starts_with("INT") || t.starts_with("BIGINT") || t.starts_with("SMALLINT") => {
            ColumnType::Integer
        }
        t if t.starts_with("VARCHAR")
            || t.starts_with("CHAR")
            || t.starts_with("TEXT")
            || t.starts_with("CLOB") =>
        {
            ColumnType::Varchar
        }
        t if t.starts_with("BOOL") => ColumnType::Boolean,
        t if t.starts_with("DATE") || t.starts_with("TIMESTAMP") || t.starts_with("TIME") => {
            ColumnType::Date
        }
        t if t.starts_with("FLOAT")
            || t.starts_with("DOUBLE")
            || t.starts_with("NUMERIC")
            || t.starts_with("DECIMAL")
            || t.starts_with("REAL") =>
        {
            ColumnType::Float
        }
        _ => ColumnType::Integer,
    }
}
```

**Step 2: Verify it compiles**

Run: `cargo check -p metamorphosis-mcp`
Expected: PASS (with possible warnings about unused functions — tools are called from server.rs in Task 8)

**Step 3: Commit**

```
feat(mcp): implement tool logic — helpers + all 5 tools
```

---

### Task 4: Implement MCP Server Handler

**Files:**
- Create: `crates/mcp-server/src/server.rs` (replace stub)

**Step 1: Implement the server**

Uses `rmcp` macros: `tool_router` with `server_handler` flag auto-generates `ServerHandler` impl.

```rust
//! MCP server handler and stdio transport.

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools;
use crate::types::*;

/// Metamorphosis MCP server.
///
/// Stateless — all engine operations are performed per-request.
#[derive(Clone, Default)]
pub struct MetamorphosisServer;

#[tool_router(server_handler)]
impl MetamorphosisServer {
    #[tool(
        name = "rewrite_sql",
        description = "Rewrite SQL using Safe and Conditional semantic rules. \
            Returns rewritten SQL statements with match diagnostics."
    )]
    fn rewrite_sql(
        &self,
        Parameters(params): Parameters<SqlParams>,
    ) -> String {
        match tools::rewrite_sql(&params) {
            Ok(result) => serde_json::to_string_pretty(&result)
                .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {e}\"}}")),
            Err(e) => serde_json::to_string_pretty(&ErrorResponse { error: e })
                .unwrap_or_else(|_| format!("{{\"error\": \"unknown error\"}}")),
        }
    }

    #[tool(
        name = "suggest_probes",
        description = "Generate data quality probe SQL suggestions using Manual-level rules. \
            Returns probe SQL statements with confidence levels and match diagnostics."
    )]
    fn suggest_probes(
        &self,
        Parameters(params): Parameters<SqlParams>,
    ) -> String {
        match tools::suggest_probes(&params) {
            Ok(result) => serde_json::to_string_pretty(&result)
                .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {e}\"}}")),
            Err(e) => serde_json::to_string_pretty(&ErrorResponse { error: e })
                .unwrap_or_else(|_| format!("{{\"error\": \"unknown error\"}}")),
        }
    }

    #[tool(
        name = "list_rules",
        description = "List all available rewrite rules with their metadata: \
            id, description, category, and safety level."
    )]
    fn list_rules(&self) -> String {
        let result = tools::list_rules();
        serde_json::to_string_pretty(&result)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {e}\"}}"))
    }

    #[tool(
        name = "verify_equivalence",
        description = "Verify semantic equivalence of two SQL queries using Z3 SMT solver. \
            Supports two engines: 'qed' (default, rich schema constraints) and 'verieql' (bounded verification). \
            Schema is required."
    )]
    fn verify_equivalence(
        &self,
        Parameters(params): Parameters<VerifyParams>,
    ) -> String {
        match tools::verify_equivalence(&params) {
            Ok(result) => serde_json::to_string_pretty(&result)
                .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {e}\"}}")),
            Err(e) => serde_json::to_string_pretty(&ErrorResponse { error: e })
                .unwrap_or_else(|_| format!("{{\"error\": \"unknown error\"}}")),
        }
    }

    #[tool(
        name = "extract_schema",
        description = "Extract table schema (column names and types) from a directory \
            of DDL SQL files. Returns a JSON schema map suitable for use with \
            rewrite_sql and suggest_probes."
    )]
    fn extract_schema(
        &self,
        Parameters(params): Parameters<ExtractSchemaParams>,
    ) -> String {
        match tools::extract_schema(&params) {
            Ok(result) => serde_json::to_string_pretty(&result)
                .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {e}\"}}")),
            Err(e) => serde_json::to_string_pretty(&ErrorResponse { error: e })
                .unwrap_or_else(|_| format!("{{\"error\": \"unknown error\"}}")),
        }
    }
}

/// Run the MCP server over stdio transport.
///
/// This function blocks until the transport is closed (e.g., stdin EOF).
pub async fn run_stdio() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use rmcp::ServiceExt;

    let server = MetamorphosisServer;
    let transport = rmcp::transport::io::stdio();
    let service = server.serve(transport).await?;
    service.waiting().await?;
    Ok(())
}
```

**Step 2: Verify it compiles**

Run: `cargo check -p metamorphosis-mcp`
Expected: PASS

If you get a compilation error about `serve` method not found, ensure:
- The `rmcp` features include `"server"` and `"transport-io"`
- `ServiceExt` is in scope via `use rmcp::ServiceExt`
- The transport type matches: `rmcp::transport::stdio()` returns `(Stdin, Stdout)`

If `Parameters` or `Json` are not found, the import paths may differ in your rmcp version. Check `rmcp::handler::server::wrapper` module. Alternative import: `use rmcp::{tool, tool_router, ServerHandler};` and remove the `server_handler` flag — implement `ServerHandler` manually with `#[tool_handler]`.

**Step 3: Commit**

```
feat(mcp): implement MCP server handler with 5 tools + stdio transport
```

---

### Task 5: Add `mcp` Subcommand to CLI

**Files:**
- Modify: `crates/cli/Cargo.toml`
- Modify: `crates/cli/src/main.rs`

**Step 1: Add dependencies to CLI**

In `crates/cli/Cargo.toml`, add:

```toml
metamorphosis-mcp = { path = "../mcp-server" }
tokio = { version = "1", features = ["rt", "macros"] }
```

**Step 2: Add `Mcp` command variant**

In `crates/cli/src/main.rs`, add to the `Command` enum:

```rust
/// Start MCP server over stdio (for AI assistant integration)
Mcp,
```

**Step 3: Handle the `Mcp` command**

In the `match cli.command` block, add:

```rust
Command::Mcp => {
    if let Err(e) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to create tokio runtime")
        .block_on(metamorphosis_mcp::run_stdio())
    {
        eprintln!("MCP server error: {e}");
        std::process::exit(1);
    }
}
```

**Step 4: Verify it compiles**

Run: `cargo check -p metamorphosis-cli`
Expected: PASS

**Step 5: Commit**

```
feat(cli): add `mcp` subcommand for MCP server
```

---

### Task 6: Build and Manual Smoke Test

**Step 1: Build the full workspace**

Run: `cargo build --workspace`
Expected: PASS with 0 errors

**Step 2: Test `list_rules` via automated smoke test**

Run a minimal JSON-RPC exchange to verify the MCP server starts and responds:

```bash
# Send MCP initialize + tools/list request, capture output
echo '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1.0"}}}
{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | cargo run -p metamorphosis-cli -- mcp 2>/dev/null | head -5
```

Expected: JSON-RPC responses containing tool names (`rewrite_sql`, `suggest_probes`, `list_rules`, `verify_equivalence`, `extract_schema`).

If the server doesn't start or the transport path is wrong, check:
1. `rmcp::transport::io::stdio()` exists (run `cargo doc -p rmcp --open` to browse API)
2. `transport-io` feature is enabled in Cargo.toml
3. `ServiceExt::serve` method is available

**Step 3: Configure MCP client (optional)**

For interactive testing with Claude Desktop / Cursor:

```json
{
  "mcpServers": {
    "metamorphosis": {
      "command": "cargo",
      "args": ["run", "-p", "metamorphosis-cli", "--", "mcp"]
    }
  }
}
```

Or with the built binary:

```json
{
  "mcpServers": {
    "metamorphosis": {
      "command": "/path/to/metamorphosis",
      "args": ["mcp"]
    }
  }
}
```

**Step 4: Commit if any fixes were needed**

---

### Task 7: End-to-End Tests

**Files:**
- Create: `crates/mcp-server/tests/integration.rs`

**Step 1: Write integration tests for the tool logic**

These test the underlying tool functions directly (not the MCP protocol layer), since the protocol layer is handled by `rmcp`.

```rust
//! Integration tests for MCP tool logic.

use metamorphosis_mcp::tools;

#[test]
fn test_list_rules() {
    let result = tools::list_rules();
    assert!(!result.rules.is_empty());
    assert_eq!(result.rules.len(), 4);

    let ids: Vec<&str> = result.rules.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"eliminate-select-star"));
    assert!(ids.contains(&"detect-duplicate-eq-keys"));
    assert!(ids.contains(&"extract-candidate-values"));
    assert!(ids.contains(&"subquery-to-join"));

    // Check structure
    let rule = result.rules.iter().find(|r| r.id == "eliminate-select-star").unwrap();
    assert_eq!(rule.safety_level, "Safe");
    assert_eq!(rule.category, "semantic");
}

#[test]
fn test_rewrite_sql_no_change() {
    let result = tools::rewrite_sql(&metamorphosis_mcp::types::SqlParams {
        sql: "SELECT id, name FROM users WHERE id = 1".to_string(),
        version: None,
        schema_json: None,
        schema_path: None,
        sql_dir: None,
        rules: None,
    }).unwrap();
    assert!(!result.changed);
    assert_eq!(result.rewritten_sql.len(), 1);
}

#[test]
fn test_rewrite_sql_with_select_star() {
    let schema = r#"{"users": {"id": "integer", "name": "varchar", "email": "varchar"}}"#;
    let result = tools::rewrite_sql(&metamorphosis_mcp::types::SqlParams {
        sql: "SELECT * FROM users WHERE id = 1".to_string(),
        version: None,
        schema_json: Some(schema.to_string()),
        schema_path: None,
        sql_dir: None,
        rules: None,
    }).unwrap();
    assert!(result.changed);
    assert!(result.rewritten_sql[0].contains("id"));
    assert!(result.rewritten_sql[0].contains("name"));
    assert!(result.rewritten_sql[0].contains("email"));
}

#[test]
fn test_suggest_probes_detect_duplicate() {
    let result = tools::suggest_probes(&metamorphosis_mcp::types::SqlParams {
        sql: "SELECT trade_code INTO v_code FROM trades WHERE account_date = :date AND account_seqno = :seq".to_string(),
        version: None,
        schema_json: None,
        schema_path: None,
        sql_dir: None,
        rules: None,
    }).unwrap();
    assert!(!result.suggestions.is_empty());

    let probe = result.suggestions.iter().find(|s| s.rule_id == "detect-duplicate-eq-keys");
    assert!(probe.is_some());
    let probe = probe.unwrap();
    assert!(probe.probe_sql.is_some());
    assert!(probe.probe_sql.as_ref().unwrap().contains("GROUP BY"));
}

#[test]
fn test_extract_schema_error_invalid_dir() {
    let result = tools::extract_schema(&metamorphosis_mcp::types::ExtractSchemaParams {
        sql_dir: "/nonexistent/path".to_string(),
    });
    assert!(result.is_err());
}

#[test]
fn test_load_schema_inline_json() {
    let schema = r#"{"users": {"id": "integer", "name": "varchar"}}"#;
    let result = tools::load_schema(Some(schema), None, None);
    assert!(result.is_ok());
    let map = result.unwrap().unwrap();
    assert!(map.contains_key("users"));
}

#[test]
fn test_load_schema_mutual_exclusion() {
    let result = tools::load_schema(
        Some("{}"),
        Some("path.json"),
        None,
    );
    assert!(result.is_err());
}
```

**Step 2: Run the tests**

Run: `cargo test -p metamorphosis-mcp`
Expected: All tests PASS

**Step 3: Commit**

```
test(mcp): add integration tests for tool logic
```

---

### Task 8: Build Full Workspace and Final Verification

**Step 1: Run all workspace tests**

Run: `cargo test --workspace`
Expected: All tests pass (including pre-existing tests in other crates)

**Step 2: Check for warnings**

Run: `cargo clippy --workspace -- -D warnings`

Fix any clippy warnings in the new crate. Common issues:
- Unused imports
- Missing documentation on public items (we'll add docs later if needed)

**Step 3: Verify the MCP binary starts**

Run: `echo '' | timeout 2 cargo run -p metamorphosis-cli -- mcp || true`
Expected: The process starts without panicking. It may hang waiting for MCP protocol input, which is expected.

**Step 4: Final commit**

```
chore: workspace-wide clippy fixes for mcp crate
```

---

## MCP Client Configuration

After implementation, users can configure their MCP client:

### Claude Desktop (`claude_desktop_config.json`)

```json
{
  "mcpServers": {
    "metamorphosis": {
      "command": "/path/to/metamorphosis",
      "args": ["mcp"]
    }
  }
}
```

### Cursor / other MCP clients

Same pattern: command = path to `metamorphosis` binary, args = `["mcp"]`.

## Future Enhancements (out of scope for MVP)

- HTTP/SSE transport for remote deployments
- Tool annotations (`readOnlyHint`, `idempotentHint`)
- Progress notifications for long-running verifications
- Prompt templates for common SQL rewriting workflows
- Resource exposure (rule documentation, example SQL)
