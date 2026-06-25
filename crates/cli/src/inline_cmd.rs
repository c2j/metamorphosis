//! `inline` subcommand — replaces SQL parameter placeholders with literal
//! values to produce directly executable SQL.
//!
//! Supports three parameter styles:
//! - **JDBC `?`** positional parameters (`--val` / `--params-file`)
//! - **MyBatis `#{name}`** named parameters (`--param` / `--params-file`)
//! - **Stored proc variables** (ColumnRef): when `--procedure`/`--from-procedure`
//!   is provided, only declared variables are substituted; otherwise, ColumnRef
//!   names that match a `--param` key are substituted as an explicit fallback
//!
//! Use `--param-string key=value` to force the value to String type, bypassing
//! `infer_value` (e.g. for single-digit codes that should remain quoted in SQL).

use std::collections::HashSet;
use std::path::PathBuf;

use metamorphosis_core::inline::{infer_value, inline_statement, InlineParams, InlineValue};
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::{ParseOptions, Parser};

// ── Helpers ────────────────────────────────────────────────────────────────

/// Parse a `--param "key=value"` string into a named parameter.
fn parse_kv(s: &str) -> Option<(String, InlineValue)> {
    let eq_idx = s.find('=')?;
    let key = s[..eq_idx].trim().to_string();
    let value = infer_value(s[eq_idx + 1..].trim());
    Some((key, value))
}

/// Parse a `--param-string "key=value"` pair; value is always `InlineValue::String`.
fn parse_string_kv(s: &str) -> Option<(String, InlineValue)> {
    let eq_idx = s.find('=')?;
    let key = s[..eq_idx].trim().to_string();
    let value = InlineValue::String(s[eq_idx + 1..].trim().to_string());
    Some((key, value))
}

/// Load parameters from a JSON file.
fn load_params_json(path: &PathBuf) -> InlineParams {
    let content = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("Error: cannot read params file '{}': {}", path.display(), e);
        std::process::exit(1);
    });
    let json: serde_json::Value = serde_json::from_str(&content).unwrap_or_else(|e| {
        eprintln!("Error: invalid JSON in params file: {}", e);
        std::process::exit(1);
    });

    let mut params = InlineParams::default();

    if let Some(obj) = json.as_object() {
        for (key, val) in obj {
            if key == "positional" {
                continue;
            }
            params.named.insert(key.clone(), json_to_inline_value(val));
        }
    }

    if let Some(pos_array) = json.get("positional").and_then(|v| v.as_array()) {
        for val in pos_array {
            params.positional.push(json_to_inline_value(val));
        }
    }

    params
}

/// Convert a `serde_json::Value` to an [`InlineValue`].
fn json_to_inline_value(v: &serde_json::Value) -> InlineValue {
    match v {
        serde_json::Value::Null => InlineValue::Null,
        serde_json::Value::Bool(b) => InlineValue::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                InlineValue::Integer(i)
            } else {
                InlineValue::Float(n.to_string())
            }
        }
        serde_json::Value::String(s) => InlineValue::String(s.clone()),
        _ => InlineValue::String(v.to_string()),
    }
}

/// Parse SQL text, optionally with MyBatis support.
fn parse_sql_text(sql: &str, mybatis: bool) -> Vec<Statement> {
    if mybatis {
        let output = Parser::parse_sql_with_options(
            sql,
            ParseOptions {
                preserve_comments: false,
                mybatis_params: true,
            },
        );
        for err in &output.errors {
            eprintln!("Parse warning: {:?}", err);
        }
        output
            .statements
            .into_iter()
            .map(|si| si.statement)
            .collect()
    } else {
        let (stmt_infos, errors) = Parser::parse_sql(sql);
        for err in &errors {
            eprintln!("Parse warning: {:?}", err);
        }
        stmt_infos.into_iter().map(|si| si.statement).collect()
    }
}

// ── Output formatting ──────────────────────────────────────────────────────

/// Print results in `SqlOnly` mode (default).
fn print_sql_only(results: &[metamorphosis_core::inline::InlineResult]) {
    for result in results {
        let sql = SqlFormatter::new()
            .pretty_print(true)
            .format_statement(&result.statement);
        println!("{};", sql);
    }
}

/// Print results in `Text` mode (diagnostics + SQL).
fn print_text(results: &[metamorphosis_core::inline::InlineResult], source_label: &str) {
    for (i, result) in results.iter().enumerate() {
        let stmt_type = match &result.statement {
            Statement::Select(_) => "SELECT",
            Statement::Insert(_) | Statement::InsertAll(_) | Statement::InsertFirst(_) => "INSERT",
            Statement::Update(_) => "UPDATE",
            Statement::Delete(_) => "DELETE",
            Statement::Merge(_) => "MERGE",
            _ => "SQL",
        };

        println!(
            "-- Statement {} ({}) from {}",
            i + 1,
            stmt_type,
            source_label
        );
        println!(
            "-- Replaced: {} named, {} positional",
            result.replaced_named, result.replaced_positional
        );

        if !result.remaining.is_empty() {
            println!("-- Remaining placeholder(s):");
            for r in &result.remaining {
                let detail = match (r.kind, &r.name, r.position) {
                    ("jdbc", _, Some(pos)) => format!("? at position {pos}"),
                    ("parameter", _, Some(pos)) => format!("${pos}"),
                    ("mybatis", Some(name), _) => format!("#{{}}{name}}}", name = name),
                    ("variable", Some(name), _) => format!("variable '{name}'"),
                    (kind, Some(name), _) => format!("{kind} '{name}'"),
                    (kind, None, None) => format!("{kind} (unnamed)"),
                    (kind, None, Some(pos)) => format!("{kind} at {pos}"),
                };
                println!("--   {detail}");
            }
        }

        let sql = SqlFormatter::new()
            .pretty_print(true)
            .format_statement(&result.statement);
        println!("{};", sql);
        println!();
    }
}

/// Print results in `Json` mode.
fn print_json(results: &[metamorphosis_core::inline::InlineResult]) {
    let json_results: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            let sql = SqlFormatter::new()
                .pretty_print(true)
                .format_statement(&r.statement);
            let remaining: Vec<serde_json::Value> = r
                .remaining
                .iter()
                .map(|rem| {
                    serde_json::json!({
                        "kind": rem.kind,
                        "name": rem.name,
                        "position": rem.position,
                    })
                })
                .collect();
            serde_json::json!({
                "statement": sql,
                "replaced_named": r.replaced_named,
                "replaced_positional": r.replaced_positional,
                "remaining": remaining,
            })
        })
        .collect();

    println!(
        "{}",
        serde_json::to_string_pretty(&json_results).expect("JSON serialization failed")
    );
}

// ── Procedure mode ─────────────────────────────────────────────────────────

/// Run inline in `--from-procedure` mode.
fn run_inline_from_procedure(
    file: &std::path::Path,
    params: &InlineParams,
    _mybatis: bool,
    output: &super::OutputFormat,
) {
    let analysis = crate::provenance::analyze_procedure_file(file);
    let known_vars: Option<HashSet<String>> = if analysis.variables.is_empty() {
        None
    } else {
        Some(analysis.variables)
    };

    let items = &analysis.extracted_sql;
    if items.is_empty() {
        eprintln!("No SQL statements found in procedure");
        std::process::exit(1);
    }

    let mut results = Vec::new();
    for (stmt, _prov) in items {
        let result = inline_statement(stmt, params, known_vars.as_ref());
        results.push(result);
    }

    let source_label = file
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file.display().to_string());

    match output {
        super::OutputFormat::Json => print_json(&results),
        super::OutputFormat::Text => print_text(&results, &source_label),
        _ => print_sql_only(&results),
    }
}

// ── Public entrypoint ──────────────────────────────────────────────────────

/// Run the `inline` subcommand.
#[allow(clippy::too_many_arguments)]
pub fn run_inline(
    file: Option<PathBuf>,
    params_named: Vec<String>,
    params_string: Vec<String>,
    params_positional: Vec<String>,
    params_file: Option<PathBuf>,
    mybatis: bool,
    procedure: Option<PathBuf>,
    from_procedure: bool,
    output: super::OutputFormat,
) {
    // 1. Build InlineParams from all sources
    let mut params = InlineParams::default();

    if let Some(ref path) = params_file {
        params = load_params_json(path);
    }

    for kv in &params_named {
        if let Some((k, v)) = parse_kv(kv) {
            params.named.insert(k, v);
        }
    }

    for kv in &params_string {
        if let Some((k, v)) = parse_string_kv(kv) {
            params.named.insert(k, v);
        }
    }

    for v in &params_positional {
        params.positional.push(infer_value(v));
    }

    // 2. Handle from_procedure mode
    if from_procedure {
        let file_path = file.as_deref().unwrap_or_else(|| {
            eprintln!("Error: --from-procedure requires a file argument");
            std::process::exit(1);
        });
        run_inline_from_procedure(file_path, &params, mybatis, &output);
        return;
    }

    // 3. Determine known_variables gating mode.
    //    - --procedure provided → strict whitelist from procedure declarations
    //    - --mybatis without --procedure → empty whitelist (disable ColumnRef
    //      fallback so `WHERE col = #{col}` doesn't double-substitute)
    //    - Neither → None (fallback mode: ColumnRef matches params.named keys)
    let known_vars: Option<HashSet<String>> =
        match crate::load_procedure_variables(procedure) {
            Some(vars) => Some(vars),
            None if mybatis => Some(HashSet::new()),
            None => None,
        };

    // 4. Parse SQL
    let (sql, source_label) = crate::resolve_input(&file);
    let statements = parse_sql_text(&sql, mybatis);

    if statements.is_empty() {
        eprintln!("No SQL statements found");
        std::process::exit(1);
    }

    // 5. Inline each statement
    let mut results = Vec::new();
    for stmt in &statements {
        let result = inline_statement(stmt, &params, known_vars.as_ref());
        results.push(result);
    }

    // 6. Output
    match output {
        super::OutputFormat::Json => print_json(&results),
        super::OutputFormat::Text => print_text(&results, &source_label),
        _ => print_sql_only(&results),
    }
}
