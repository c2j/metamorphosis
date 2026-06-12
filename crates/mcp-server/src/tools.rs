//! MCP tool implementations — helpers and 5 tool functions.
//!
//! # Helpers
//!
//! - `load_schema` — inline JSON / file path / DDL dir (mutually exclusive)
//! - `build_engine` — creates `RewriteEngine` with all built-in rules
//! - `parse_sql` — parses SQL, returns statements + warnings
//! - `format_stmt` — pretty-print a single AST statement
//!
//! # Tools
//!
//! - `list_rules` — available rewrite rules and metadata
//! - `rewrite_sql` — apply Safe/Conditional rules
//! - `suggest_probes` — extract Manual-level suggestions and probes
//! - `verify_equivalence` — QED or VeriEQL verification
//! - `extract_schema` — DDL-driven schema extraction from a directory

use std::collections::HashSet;
use std::path::Path;

use metamorphosis_core::context::{RewriteConfig, RewriteContext};
use metamorphosis_core::engine::RewriteEngine;
use metamorphosis_core::extractor::extract_schema_from_dir;
use metamorphosis_core::registry::RuleRegistry;
use metamorphosis_core::types::{Confidence, RewriteAction, RuleCategory, SafetyLevel};
use metamorphosis_qed::prover::{ProofResult as QedProofResult, ProverConfig};
use metamorphosis_qed::schema::extract_rich_schema;
use metamorphosis_qed::verify::verify_rewrite;
use metamorphosis_verieql::types::{
    Bound, ColumnType, ProofResult as VerieqlProofResult, Semantics, TableSchema,
};
use metamorphosis_verieql::VeriEql;
use ogsql_parser::analyzer::schema::SchemaMap;
use ogsql_parser::ast::{Statement, StatementInfo};
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::{Parser, ParserError};

use crate::types::{
    ExtractSchemaResponse, ListRulesResponse, MatchFailureInfo, RewriteResponse, RuleInfo,
    SuggestResponse, SuggestionInfo, VerifyResponse,
};

// ── Helper functions ──────────────────────────────────────────────────────

/// Load a [`SchemaMap`] from one of three mutually exclusive sources:
///
/// 1. `schema_json` — inline JSON string
/// 2. `schema_path` — path to a JSON file on disk
/// 3. `sql_dir` — directory of `.sql` DDL files
///
/// Returns `Ok(None)` when no source is provided. Returns an error if more
/// than one source is provided or if the source cannot be read/parsed.
pub fn load_schema(
    schema_json: Option<&str>,
    schema_path: Option<&str>,
    sql_dir: Option<&str>,
) -> Result<Option<SchemaMap>, String> {
    let sources =
        schema_json.is_some() as u8 + schema_path.is_some() as u8 + sql_dir.is_some() as u8;

    if sources > 1 {
        return Err("schema_json, schema_path, and sql_dir are mutually exclusive".to_string());
    }

    if let Some(json_str) = schema_json {
        let map: SchemaMap =
            serde_json::from_str(json_str).map_err(|e| format!("invalid schema_json: {e}"))?;
        return Ok(Some(map));
    }

    if let Some(path) = schema_path {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read schema_path '{path}': {e}"))?;
        let map: SchemaMap = serde_json::from_str(&content)
            .map_err(|e| format!("invalid schema JSON at '{path}': {e}"))?;
        return Ok(Some(map));
    }

    if let Some(dir) = sql_dir {
        let map = extract_schema_from_dir(Path::new(dir))
            .map_err(|e| format!("schema extraction from '{dir}': {e}"))?;
        return Ok(Some(map));
    }

    Ok(None)
}

/// Build a [`RewriteEngine`] with all built-in rules registered.
pub fn build_engine() -> RewriteEngine {
    let rules = metamorphosis_rules::builtin_rules();
    let registry = RuleRegistry::new(rules);
    RewriteEngine::new(registry)
}

/// Parse SQL text into a list of [`StatementInfo`] and warning messages.
pub fn parse_sql(sql: &str) -> (Vec<StatementInfo>, Vec<String>) {
    let (statements, errors) = Parser::parse_sql(sql);
    let warnings: Vec<String> = errors
        .iter()
        .filter(|e| is_warning(e))
        .map(|e| e.to_string())
        .collect();
    (statements, warnings)
}

/// Pretty-print a single AST statement back to SQL text.
pub fn format_stmt(stmt: &Statement) -> String {
    SqlFormatter::new()
        .pretty_print(true)
        .format_statement(stmt)
}

/// Convert a [`SchemaMap`] to CREATE TABLE DDL statements.
///
/// This produces a minimal DDL string that can be re-parsed and passed to
/// [`extract_rich_schema`] for QED verification.
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

/// Convert a [`SchemaMap`] to a [`Vec<TableSchema>`] for VeriEQL.
fn schema_map_to_verieql(schema: &SchemaMap) -> Vec<TableSchema> {
    schema
        .iter()
        .map(|(table_name, columns)| TableSchema {
            name: table_name.clone(),
            columns: columns
                .iter()
                .map(
                    |(col_name, col_type)| metamorphosis_verieql::types::ColumnDef {
                        name: col_name.clone(),
                        col_type: sql_type_to_verieql(col_type),
                    },
                )
                .collect(),
        })
        .collect()
}

/// Map a SQL type string to a VeriEQL [`ColumnType`].
fn sql_type_to_verieql(ty: &str) -> ColumnType {
    let upper = ty.to_uppercase();
    if upper.starts_with("INT")
        || upper.starts_with("BIGINT")
        || upper.starts_with("SMALLINT")
        || upper.starts_with("TINYINT")
        || upper.starts_with("SERIAL")
    {
        ColumnType::Integer
    } else if upper.starts_with("VARCHAR")
        || upper.starts_with("CHAR")
        || upper.starts_with("TEXT")
        || upper.starts_with("CLOB")
    {
        ColumnType::Varchar
    } else if upper.starts_with("BOOL") {
        ColumnType::Boolean
    } else if upper.starts_with("DATE")
        || upper.starts_with("TIMESTAMP")
        || upper.starts_with("TIME")
    {
        ColumnType::Date
    } else if upper.starts_with("FLOAT")
        || upper.starts_with("DOUBLE")
        || upper.starts_with("NUMERIC")
        || upper.starts_with("DECIMAL")
        || upper.starts_with("REAL")
    {
        ColumnType::Float
    } else {
        ColumnType::Integer
    }
}

/// Check whether a [`ParserError`] is a non-fatal warning.
fn is_warning(e: &ParserError) -> bool {
    matches!(
        e,
        ParserError::Warning { .. } | ParserError::ReservedKeywordAsIdentifier { .. }
    )
}

// ── Value mapping helpers ─────────────────────────────────────────────────

fn confidence_to_string(c: &Confidence) -> String {
    match c {
        Confidence::High => "High",
        Confidence::Medium => "Medium",
        Confidence::Low => "Low",
        _ => "Unknown",
    }
    .to_string()
}

fn category_to_string(c: &RuleCategory) -> String {
    match c {
        RuleCategory::Performance => "Performance",
        RuleCategory::DataQuality => "DataQuality",
        RuleCategory::Style => "Style",
        RuleCategory::Semantic => "Semantic",
        RuleCategory::Safety => "Safety",
        _ => "Unknown",
    }
    .to_string()
}

fn safety_level_to_string(s: &SafetyLevel) -> String {
    match s {
        SafetyLevel::Safe => "Safe",
        SafetyLevel::Conditional => "Conditional",
        SafetyLevel::Manual => "Manual",
        _ => "Unknown",
    }
    .to_string()
}

// ── Tool functions ────────────────────────────────────────────────────────

/// List all available rewrite rules with their metadata.
///
/// Calls `metamorphosis_rules::builtin_rules()` and maps each rule to a
/// [`RuleInfo`] struct with id, description, category, safety level, and
/// default enabled state.
pub fn list_rules() -> ListRulesResponse {
    let rules = metamorphosis_rules::builtin_rules();
    let rules_info: Vec<RuleInfo> = rules
        .iter()
        .map(|r| RuleInfo {
            id: r.id().to_string(),
            description: r.description().to_string(),
            category: category_to_string(&r.category()),
            safety_level: safety_level_to_string(&r.safety_level()),
            default_enabled: r.default_enabled(),
        })
        .collect();
    ListRulesResponse { rules: rules_info }
}

/// Rewrite SQL by applying all matching Safe/Conditional rules.
///
/// Parses the input SQL, loads the schema (if provided), builds the rewrite
/// engine, and runs the rewrite pipeline. Returns the rewritten statements,
/// match failures, and any parse warnings.
pub fn rewrite_sql(params: crate::types::SqlParams) -> Result<RewriteResponse, String> {
    let schema = load_schema(
        params.schema_json.as_deref(),
        params.schema_path.as_deref(),
        params.sql_dir.as_deref(),
    )?;
    let (stmt_infos, warnings) = parse_sql(&params.sql);
    let stmts: Vec<Statement> = stmt_infos.into_iter().map(|si| si.statement).collect();

    let engine = build_engine();

    let mut config = RewriteConfig::default();
    if let Some(rules_str) = &params.rules {
        let enabled: HashSet<String> = rules_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        config.enabled_rules = enabled;
    }

    let ctx = RewriteContext {
        version: params.version.as_deref(),
        schema: schema.as_ref(),
        config: &config,
        source_file: None,
        known_variables: None,
    };

    let result = engine.rewrite(&ctx, stmts);

    let rewritten_sql: Vec<String> = result.statements.iter().map(format_stmt).collect();
    let match_failures: Vec<MatchFailureInfo> = result
        .match_failures
        .into_iter()
        .map(|mf| MatchFailureInfo {
            rule_id: mf.rule_id,
            reason: mf.reason,
        })
        .collect();

    Ok(RewriteResponse {
        changed: result.changed,
        rewritten_sql,
        match_failures,
        warnings,
    })
}

/// Generate suggestions / probes from Manual-level and Generate rules.
///
/// Same pipeline as [`rewrite_sql`] but extracts probe SQL from
/// `RewriteAction::Generate` variants and messages from `RewriteAction::Suggest`
/// variants instead of returning rewritten statements.
pub fn suggest_probes(params: crate::types::SqlParams) -> Result<SuggestResponse, String> {
    let schema = load_schema(
        params.schema_json.as_deref(),
        params.schema_path.as_deref(),
        params.sql_dir.as_deref(),
    )?;
    let (stmt_infos, warnings) = parse_sql(&params.sql);
    let stmts: Vec<Statement> = stmt_infos.into_iter().map(|si| si.statement).collect();

    let engine = build_engine();

    let mut config = RewriteConfig::default();
    if let Some(rules_str) = &params.rules {
        let enabled: HashSet<String> = rules_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        config.enabled_rules = enabled;
    }

    let ctx = RewriteContext {
        version: params.version.as_deref(),
        schema: schema.as_ref(),
        config: &config,
        source_file: None,
        known_variables: None,
    };

    let result = engine.rewrite(&ctx, stmts);

    let suggestions: Vec<SuggestionInfo> = result
        .suggestions
        .into_iter()
        .map(|s| {
            let (probe_sql, message, purpose) = match &s.action {
                RewriteAction::Generate {
                    stmt,
                    purpose,
                    confidence: _,
                } => (Some(format_stmt(stmt)), None, Some(purpose.clone())),
                RewriteAction::Suggest { message, .. } => (None, Some(message.clone()), None),
                RewriteAction::Replace(stmt) => (Some(format_stmt(stmt)), None, None),
                _ => (None, None, None),
            };
            SuggestionInfo {
                rule_id: s.rule_id,
                rule_description: s.rule_description,
                confidence: confidence_to_string(&s.confidence),
                probe_sql,
                message,
                purpose,
            }
        })
        .collect();

    let match_failures: Vec<MatchFailureInfo> = result
        .match_failures
        .into_iter()
        .map(|mf| MatchFailureInfo {
            rule_id: mf.rule_id,
            reason: mf.reason,
        })
        .collect();

    Ok(SuggestResponse {
        suggestions,
        match_failures,
        warnings,
    })
}

/// Verify semantic equivalence between original and rewritten SQL.
///
/// Dispatches to the QED prover (default) or VeriEQL bounded model checker
/// based on `params.engine` (`"qed"` or `"verieql"`).
pub fn verify_equivalence(params: crate::types::VerifyParams) -> Result<VerifyResponse, String> {
    let engine = params.engine.as_deref().unwrap_or("qed").to_lowercase();

    match engine.as_str() {
        "qed" => verify_with_qed(params),
        "verieql" => verify_with_verieql(params),
        other => Err(format!(
            "unknown verification engine: '{other}'. expected 'qed' or 'verieql'"
        )),
    }
}

/// Verify equivalence using the QED prover (embedded Z3).
fn verify_with_qed(params: crate::types::VerifyParams) -> Result<VerifyResponse, String> {
    let schema = load_schema(
        params.schema_json.as_deref(),
        params.schema_path.as_deref(),
        params.sql_dir.as_deref(),
    )?
    .ok_or_else(|| {
        "schema required for QED verification: provide schema_json, schema_path, or sql_dir"
            .to_string()
    })?;

    // Parse both SQL strings into single statements
    let (orig_infos, _) = Parser::parse_sql(&params.original_sql);
    let (rew_infos, _) = Parser::parse_sql(&params.rewritten_sql);

    let orig_stmt = orig_infos
        .into_iter()
        .next()
        .ok_or_else(|| "original_sql produced no statements".to_string())?
        .statement;
    let rew_stmt = rew_infos
        .into_iter()
        .next()
        .ok_or_else(|| "rewritten_sql produced no statements".to_string())?
        .statement;

    // Build DDL from SchemaMap, parse it, and extract rich schema
    let ddl = schema_map_to_ddl(&schema);
    let (ddl_infos, _) = Parser::parse_sql(&ddl);
    let ddl_stmts: Vec<Statement> = ddl_infos.into_iter().map(|si| si.statement).collect();
    let rich_schema = extract_rich_schema(&ddl_stmts);

    let config = ProverConfig::default();
    let vr = verify_rewrite("mcp-verify", &orig_stmt, &rew_stmt, &rich_schema, &config)
        .map_err(|e| format!("QED verification failed: {e}"))?;

    let (outcome, counterexample, column_details) = match vr.proof {
        QedProofResult::Equivalent => ("Equivalent".to_string(), None, None),
        QedProofResult::NotEquivalent { counterexample: ce } => {
            let col_details = match (&vr.original_columns, &vr.rewritten_columns) {
                (Some(orig), Some(rew)) => Some(serde_json::json!({
                    "original_columns": orig,
                    "rewritten_columns": rew,
                })),
                _ => None,
            };
            ("NotEquivalent".to_string(), ce, col_details)
        }
        QedProofResult::Unknown { reason } => ("Unknown".to_string(), Some(reason), None),
        QedProofResult::Timeout { seconds } => (
            "Unknown".to_string(),
            Some(format!("timeout after {seconds}s")),
            None,
        ),
        _ => (
            "Unknown".to_string(),
            Some("unexpected proof result".to_string()),
            None,
        ),
    };

    Ok(VerifyResponse {
        result: outcome,
        engine: "qed".to_string(),
        original_sql: vr.original_sql,
        rewritten_sql: vr.rewritten_sql,
        elapsed_ms: Some(vr.elapsed_ms),
        bound: None,
        counterexample,
        column_details,
    })
}

/// Verify equivalence using VeriEQL bounded model checking.
fn verify_with_verieql(params: crate::types::VerifyParams) -> Result<VerifyResponse, String> {
    let schema = load_schema(
        params.schema_json.as_deref(),
        params.schema_path.as_deref(),
        params.sql_dir.as_deref(),
    )?
    .ok_or_else(|| {
        "schema required for VeriEQL verification: provide schema_json, schema_path, or sql_dir"
            .to_string()
    })?;

    let bound = params.bound.unwrap_or(2);
    let verieql_schema = schema_map_to_verieql(&schema);
    let constraints = serde_json::json!(null);

    let report = VeriEql::verify(
        &params.original_sql,
        &params.rewritten_sql,
        &verieql_schema,
        &constraints,
        Bound(bound),
        Semantics::Bag,
    )
    .map_err(|e| format!("VeriEQL verification failed: {e}"))?;

    let elapsed_ms = Some(report.translate_ms + report.solve_ms);

    let (outcome, counterexample, column_details) = match &report.result {
        VerieqlProofResult::Equivalent => ("Equivalent".to_string(), None, None),
        VerieqlProofResult::NotEquivalent { counterexample: ce } => {
            let ce_str = serde_json::to_string(ce).ok();
            let col_details = serde_json::to_value(ce).ok();
            ("NotEquivalent".to_string(), ce_str, col_details)
        }
        VerieqlProofResult::Unknown { reason } => {
            ("Unknown".to_string(), Some(reason.clone()), None)
        }
    };

    Ok(VerifyResponse {
        result: outcome,
        engine: "verieql".to_string(),
        original_sql: params.original_sql,
        rewritten_sql: params.rewritten_sql,
        elapsed_ms,
        bound: Some(bound),
        counterexample,
        column_details,
    })
}

/// Extract schema from a directory of DDL SQL files.
///
/// Calls `extract_schema_from_dir` and packages the result into an
/// [`ExtractSchemaResponse`] with table count and full schema map.
pub fn extract_schema(
    params: crate::types::ExtractSchemaParams,
) -> Result<ExtractSchemaResponse, String> {
    let schema = extract_schema_from_dir(Path::new(&params.sql_dir))
        .map_err(|e| format!("schema extraction failed: {e}"))?;

    let table_count = schema.len();
    let schema_value =
        serde_json::to_value(&schema).map_err(|e| format!("schema serialization failed: {e}"))?;

    Ok(ExtractSchemaResponse {
        table_count,
        schema: schema_value,
    })
}
