//! `verify` subcommand — uses embedded Z3 SMT solver to prove semantic
//! equivalence between two SQL queries.
//!
//! Supports two verification engines:
//! - **qed** (default): metamorphosis-qed prover with rich schema constraints
//! - **verieql**: bounded equivalence verification (OOPSLA 2024 algorithm)
//!
//! Requires DDL schema (via `--sql-dir` or `--schema` JSON).

use std::path::{Path, PathBuf};

use metamorphosis_core::extractor::collect_sql_files;
use metamorphosis_qed::prover::ProverConfig;
use metamorphosis_qed::schema::{extract_rich_schema, RichSchema};
use metamorphosis_qed::verify::{verify_rewrite, VerificationResult};
use metamorphosis_verieql::types::*;
use metamorphosis_verieql::VeriEql;
use ogsql_parser::ast::Statement;
use ogsql_parser::{ParseOptions, Parser};
use serde::Deserialize;

/// Available verification engines.
pub enum Engine {
    Qed,
    Verieql,
}

/// A table's schema entry in JSON — supports both legacy and new formats.
///
/// # Legacy format (backward compatible)
/// `{"id": "INTEGER", "name": "VARCHAR(100)"}`
///
/// # New format (with primary key)
/// `{"columns": {"id": "INTEGER", "name": "VARCHAR(100)"}, "primary_key": ["id"]}`
#[derive(Deserialize)]
#[serde(untagged)]
enum TableSchemaEntry {
    /// Legacy flat format: column names mapped to type strings.
    Legacy(std::collections::HashMap<String, String>),
    /// New structured format with optional primary key declaration.
    Structured {
        columns: std::collections::HashMap<String, String>,
        #[serde(default)]
        primary_key: Vec<String>,
    },
}

impl TableSchemaEntry {
    /// Returns the column-to-type map regardless of format.
    fn columns(&self) -> &std::collections::HashMap<String, String> {
        match self {
            Self::Legacy(cols) => cols,
            Self::Structured { columns, .. } => columns,
        }
    }

    /// Returns the primary key columns (empty for legacy format).
    fn primary_key(&self) -> &[String] {
        match self {
            Self::Legacy(_) => &[],
            Self::Structured { primary_key, .. } => primary_key,
        }
    }
}

// ── Public entrypoint ────────────────────────────────────────────────────

/// Run the `verify` subcommand.
///
/// Reads both SQL files, loads the DDL schema, invokes the selected
/// verification engine, and prints the result.
pub fn run_verify(
    original: PathBuf,
    rewritten: PathBuf,
    schema_path: Option<PathBuf>,
    sql_dir: Option<PathBuf>,
    output: &str,
    engine: Engine,
    bound: usize,
) {
    let original_sql = read_file(&original);
    let rewritten_sql = read_file(&rewritten);

    match engine {
        Engine::Qed => run_verify_qed(
            &original_sql,
            &original,
            &rewritten_sql,
            &rewritten,
            schema_path,
            sql_dir,
            output,
        ),
        Engine::Verieql => run_verify_verieql(
            &original_sql,
            &rewritten_sql,
            schema_path,
            sql_dir,
            output,
            bound,
        ),
    }
}

// ── QED engine ──────────────────────────────────────────────────────────

fn run_verify_qed(
    original_sql: &str,
    original_path: &Path,
    rewritten_sql: &str,
    rewritten_path: &Path,
    schema_path: Option<PathBuf>,
    sql_dir: Option<PathBuf>,
    output: &str,
) {
    let schema = load_rich_schema(schema_path, sql_dir);

    let original_stmt = parse_single_query(original_sql, original_path);
    let rewritten_stmt = parse_single_query(rewritten_sql, rewritten_path);

    let config = ProverConfig::default();
    let result = match verify_rewrite(
        "cli-verify",
        &original_stmt,
        &rewritten_stmt,
        &schema,
        &config,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: verification failed: {e}");
            std::process::exit(1);
        }
    };

    match output {
        "json" => print_qed_json(&result),
        _ => print_qed_text(&result),
    }
}

// ── VeriEQL engine ──────────────────────────────────────────────────────

fn run_verify_verieql(
    original_sql: &str,
    rewritten_sql: &str,
    schema_path: Option<PathBuf>,
    sql_dir: Option<PathBuf>,
    output: &str,
    bound: usize,
) {
    let schema = load_verieql_schema(schema_path, sql_dir);
    let constraints = serde_json::json!(null);

    let result = match VeriEql::verify(
        original_sql,
        rewritten_sql,
        &schema,
        &constraints,
        Bound(bound),
        Semantics::Bag,
    ) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("Error: VeriEQL verification failed: {e}");
            std::process::exit(1);
        }
    };

    match output {
        "json" => print_verieql_json(&result, original_sql, rewritten_sql),
        _ => print_verieql_text(&result, original_sql, rewritten_sql),
    }
}

// ── Schema loading ───────────────────────────────────────────────────────

/// Load a [`RichSchema`] for the QED engine.
fn load_rich_schema(schema_path: Option<PathBuf>, sql_dir: Option<PathBuf>) -> RichSchema {
    match (schema_path, sql_dir) {
        (Some(p), None) => load_schema_from_json(p),
        (None, Some(dir)) => load_schema_from_dir(dir),
        (None, None) => {
            eprintln!("Error: --schema or --sql-dir is required for verify");
            std::process::exit(1);
        }
        (Some(_), Some(_)) => {
            eprintln!("Error: --schema and --sql-dir are mutually exclusive");
            std::process::exit(1);
        }
    }
}

/// Load a [`Vec<TableSchema>`] for the VeriEQL engine.
fn load_verieql_schema(schema_path: Option<PathBuf>, sql_dir: Option<PathBuf>) -> Vec<TableSchema> {
    match (schema_path, sql_dir) {
        (Some(p), None) => load_verieql_schema_from_json(p),
        (None, Some(dir)) => load_verieql_schema_from_dir(dir),
        (None, None) => {
            eprintln!("Error: --schema or --sql-dir is required for verify");
            std::process::exit(1);
        }
        (Some(_), Some(_)) => {
            eprintln!("Error: --schema and --sql-dir are mutually exclusive");
            std::process::exit(1);
        }
    }
}

/// Build DDL statements from schema entries, including `PRIMARY KEY` clauses
/// when primary key info is present (new JSON format).
fn schema_entries_to_ddl(schema: &std::collections::HashMap<String, TableSchemaEntry>) -> String {
    let mut ddl = String::new();
    for (table_name, entry) in schema {
        ddl.push_str("CREATE TABLE ");
        ddl.push_str(table_name);
        ddl.push_str(" (");
        let mut defs: Vec<String> = entry
            .columns()
            .iter()
            .map(|(name, typ)| format!("{} {}", name, typ.to_uppercase()))
            .collect();
        let pk = entry.primary_key();
        if !pk.is_empty() {
            defs.push(format!("PRIMARY KEY ({})", pk.join(", ")));
        }
        ddl.push_str(&defs.join(", "));
        ddl.push_str(");\n");
    }
    ddl
}

/// Build a [`RichSchema`] from a JSON schema file.
///
/// Supports both legacy format (`{"table": {"col": "type"}}`) and the new
/// format (`{"table": {"columns": {...}, "primary_key": [...]}}`) with
/// primary key declarations.
fn load_schema_from_json(path: PathBuf) -> RichSchema {
    let content = read_file(&path);

    let schema_map: std::collections::HashMap<String, TableSchemaEntry> =
        serde_json::from_str(&content).unwrap_or_else(|e| {
            eprintln!("Error: invalid schema JSON '{}': {}", path.display(), e);
            std::process::exit(1);
        });

    if schema_map.is_empty() {
        eprintln!("Error: schema JSON '{}' is empty", path.display());
        std::process::exit(1);
    }

    let ddl = schema_entries_to_ddl(&schema_map);
    parse_and_extract(&ddl)
}

/// Build a [`RichSchema`] from a directory of `.sql` DDL files.
///
/// Reads all `.sql` files (sorted), concatenates them, parses the
/// combined DDL, and extracts the rich schema.
fn load_schema_from_dir(dir: PathBuf) -> RichSchema {
    let files = match collect_sql_files(&dir) {
        Ok(f) => f,
        Err(e) => {
            eprintln!(
                "Error: cannot scan schema directory '{}': {}",
                dir.display(),
                e
            );
            std::process::exit(1);
        }
    };

    let mut all_ddl = String::new();
    for path in &files {
        let content = read_sql_file(path);
        if content.is_empty() {
            eprintln!("Warning: empty content in '{}'", path.display());
        } else {
            all_ddl.push_str(&content);
            all_ddl.push('\n');
        }
    }

    if all_ddl.trim().is_empty() {
        eprintln!(
            "Error: no readable content in .sql files under '{}'",
            dir.display()
        );
        std::process::exit(1);
    }

    parse_and_extract(&all_ddl)
}

/// Parse a DDL string and extract a [`RichSchema`].
fn parse_and_extract(ddl: &str) -> RichSchema {
    let (stmt_infos, _errors) = Parser::parse_sql(ddl);
    let stmts: Vec<Statement> = stmt_infos.into_iter().map(|si| si.statement).collect();
    extract_rich_schema(&stmts)
}

fn load_verieql_schema_from_json(path: PathBuf) -> Vec<TableSchema> {
    let content = read_file(&path);

    let schema_map: std::collections::HashMap<String, TableSchemaEntry> =
        serde_json::from_str(&content).unwrap_or_else(|e| {
            eprintln!("Error: invalid schema JSON '{}': {}", path.display(), e);
            std::process::exit(1);
        });

    if schema_map.is_empty() {
        eprintln!("Error: schema JSON '{}' is empty", path.display());
        std::process::exit(1);
    }

    schema_map_to_verieql(&schema_map)
}

fn load_verieql_schema_from_dir(dir: PathBuf) -> Vec<TableSchema> {
    let rich = load_schema_from_dir(dir);
    rich_schema_to_verieql(&rich)
}

fn schema_map_to_verieql(
    map: &std::collections::HashMap<String, TableSchemaEntry>,
) -> Vec<TableSchema> {
    map.iter()
        .map(|(table_name, entry)| TableSchema {
            name: table_name.clone(),
            columns: entry
                .columns()
                .iter()
                .map(|(col_name, col_type)| ColumnDef {
                    name: col_name.clone(),
                    col_type: sql_type_to_verieql(col_type),
                })
                .collect(),
        })
        .collect()
}

fn rich_schema_to_verieql(schema: &RichSchema) -> Vec<TableSchema> {
    schema
        .tables
        .iter()
        .map(|(table_name, info)| TableSchema {
            name: table_name.clone(),
            columns: info
                .columns
                .iter()
                .map(|col| ColumnDef {
                    name: col.name.clone(),
                    col_type: sql_type_to_verieql(&col.data_type),
                })
                .collect(),
        })
        .collect()
}

fn sql_type_to_verieql(ty: &str) -> ColumnType {
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

// ── SQL file parsing ─────────────────────────────────────────────────────

/// Read a file to a string with encoding detection.
///
/// Tries UTF-8 first, then GBK (common in Chinese SQL DDL files), and
/// falls back to lossy UTF-8 replacement so the file content is always
/// recoverable regardless of encoding.
fn read_sql_file(path: &Path) -> String {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error: cannot read '{}': {}", path.display(), e);
            std::process::exit(1);
        }
    };

    // Fast path: try UTF-8
    if let Ok(s) = std::str::from_utf8(&bytes) {
        return s.to_string();
    }

    // Try GBK (common encoding for Chinese SQL files)
    let (cow, _encoding_used, _had_errors) = encoding_rs::GBK.decode(&bytes);
    if !cow.is_empty() {
        return cow.into_owned();
    }

    // Final fallback: lossy UTF-8 replacement
    tracing::warn!(
        "file '{}' is neither valid UTF-8 nor GBK; replacing invalid sequences",
        path.display()
    );
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Read a file to a string, exiting with an error on failure.
///
/// Uses [`read_sql_file`] for encoding-aware reading.
fn read_file(path: &Path) -> String {
    read_sql_file(path)
}

/// Parse a SQL string and extract exactly one statement.
///
/// Exits with an error if the file contains 0 or 2+ statements.
fn parse_single_query(sql: &str, file: &Path) -> Statement {
    let parse_output = Parser::parse_sql_with_options(
        sql,
        ParseOptions {
            preserve_comments: false,
            mybatis_params: false,
        },
    );

    if !parse_output.errors.is_empty() {
        for err in &parse_output.errors {
            eprintln!("Parse warning: {:?}", err);
        }
    }

    let stmts: Vec<Statement> = parse_output
        .statements
        .into_iter()
        .map(|si| si.statement)
        .collect();

    if stmts.is_empty() {
        eprintln!(
            "Error: '{}' contains 0 SQL statements (expected exactly 1)",
            file.display()
        );
        std::process::exit(1);
    }
    if stmts.len() > 1 {
        eprintln!(
            "Error: '{}' contains {} SQL statements (expected exactly 1)",
            file.display(),
            stmts.len()
        );
        std::process::exit(1);
    }

    stmts.into_iter().next().expect("just verified non-empty")
}

// ── Output formatting (QED) ─────────────────────────────────────────────

fn print_qed_text(result: &VerificationResult) {
    match &result.proof {
        metamorphosis_qed::prover::ProofResult::Equivalent => {
            println!("✓ Equivalent (proven in {}ms)", result.elapsed_ms);
        }
        metamorphosis_qed::prover::ProofResult::NotEquivalent { counterexample } => {
            println!("✗ Not Equivalent");

            match (&result.original_columns, &result.rewritten_columns) {
                (Some(orig), Some(rew)) => {
                    if orig.len() != rew.len() {
                        println!(
                            "  Column count: {} (original) vs {} (rewritten)",
                            orig.len(),
                            rew.len()
                        );
                        if orig.len() > rew.len() {
                            let missing: Vec<&str> = orig
                                .iter()
                                .filter(|c| !rew.contains(c))
                                .map(|s| s.as_str())
                                .collect();
                            if !missing.is_empty() {
                                println!("  Missing from rewrite: {}", missing.join(", "));
                            }
                        } else {
                            let extra: Vec<&str> = rew
                                .iter()
                                .filter(|c| !orig.contains(c))
                                .map(|s| s.as_str())
                                .collect();
                            if !extra.is_empty() {
                                println!("  Extra columns in rewrite: {}", extra.join(", "));
                            }
                        }
                    } else if orig != rew {
                        println!("  Column order differs:");
                        println!("    Original:  {}", orig.join(", "));
                        println!("    Rewritten: {}", rew.join(", "));
                    }
                }
                (Some(orig), None) => {
                    println!("  Original columns ({}): {}", orig.len(), orig.join(", "));
                }
                (None, Some(rew)) => {
                    println!("  Rewritten columns ({}): {}", rew.len(), rew.join(", "));
                }
                (None, None) => {}
            }

            if let Some(ce) = counterexample {
                println!("  Counterexample: {ce}");
            }
        }
        metamorphosis_qed::prover::ProofResult::Unknown { reason } => {
            println!("? Unknown: {reason}");
        }
        metamorphosis_qed::prover::ProofResult::Timeout { seconds } => {
            println!("? Timeout after {seconds}s");
        }
        _ => {
            println!("? Unexpected proof result: {:?}", result.proof);
        }
    }
    println!("  Original:  {}", result.original_sql);
    println!("  Rewritten: {}", result.rewritten_sql);
}

fn print_qed_json(result: &VerificationResult) {
    let outcome = match &result.proof {
        metamorphosis_qed::prover::ProofResult::Equivalent => "Equivalent",
        metamorphosis_qed::prover::ProofResult::NotEquivalent { .. } => "NotEquivalent",
        metamorphosis_qed::prover::ProofResult::Unknown { .. } => "Unknown",
        metamorphosis_qed::prover::ProofResult::Timeout { .. } => "Timeout",
        _ => "Unknown",
    };

    let mut obj = serde_json::json!({
        "result": outcome,
        "original": result.original_sql,
        "rewritten": result.rewritten_sql,
        "elapsed_ms": result.elapsed_ms,
        "engine": "qed",
    });

    if let Some(orig) = &result.original_columns {
        obj["original_columns"] = serde_json::json!(orig);
    }
    if let Some(rew) = &result.rewritten_columns {
        obj["rewritten_columns"] = serde_json::json!(rew);
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&obj).expect("JSON serialization failed")
    );
}

// ── Output formatting (VeriEQL) ──────────────────────────────────────────

fn print_verieql_text(
    report: &metamorphosis_verieql::types::ProofReport,
    original_sql: &str,
    rewritten_sql: &str,
) {
    use metamorphosis_verieql::types::ProofResult;
    match &report.result {
        ProofResult::Equivalent => {
            println!(
                "✓ Equivalent (VeriEQL, bound={}, translate={}ms, solve={}ms)",
                report.bound.0, report.translate_ms, report.solve_ms
            );
        }
        ProofResult::NotEquivalent { counterexample } => {
            println!("✗ Not Equivalent (VeriEQL, bound={})", report.bound.0);
            if !counterexample.tables.is_empty() {
                println!("  Counterexample:");
                for table in &counterexample.tables {
                    println!("    {}:", table.name);
                    for row in &table.rows {
                        println!("      [{}]", row.join(", "));
                    }
                }
            }
        }
        ProofResult::Unknown { reason } => {
            println!("? Unknown (VeriEQL): {reason}");
        }
    }
    println!("  Original:  {original_sql}");
    println!("  Rewritten: {rewritten_sql}");
}

fn print_verieql_json(
    report: &metamorphosis_verieql::types::ProofReport,
    original_sql: &str,
    rewritten_sql: &str,
) {
    use metamorphosis_verieql::types::ProofResult;
    let outcome = match &report.result {
        ProofResult::Equivalent => "Equivalent",
        ProofResult::NotEquivalent { .. } => "NotEquivalent",
        ProofResult::Unknown { .. } => "Unknown",
    };

    let obj = serde_json::json!({
        "result": outcome,
        "original": original_sql,
        "rewritten": rewritten_sql,
        "engine": "verieql",
        "bound": report.bound.0,
        "translate_ms": report.translate_ms,
        "solve_ms": report.solve_ms,
    });

    println!(
        "{}",
        serde_json::to_string_pretty(&obj).expect("JSON serialization failed")
    );
}
