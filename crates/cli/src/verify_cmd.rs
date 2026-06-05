//! `verify` subcommand — uses embedded Z3 SMT solver (QED) to prove
//! semantic equivalence between two SQL queries.
//!
//! Requires DDL schema (via `--sql-dir` or `--schema` JSON) to build the
//! relational schema needed by the prover.

use std::path::{Path, PathBuf};

use metamorphosis_qed::prover::ProverConfig;
use metamorphosis_qed::schema::{extract_rich_schema, RichSchema};
use metamorphosis_qed::verify::{verify_rewrite, VerificationResult};
use ogsql_parser::ast::Statement;
use ogsql_parser::{ParseOptions, Parser};

// ── Public entrypoint ────────────────────────────────────────────────────

/// Run the `verify` subcommand.
///
/// Reads both SQL files, loads the DDL schema, invokes the QED/Z3 prover,
/// and prints the result in the requested format.
pub fn run_verify(
    original: PathBuf,
    rewritten: PathBuf,
    schema_path: Option<PathBuf>,
    sql_dir: Option<PathBuf>,
    output: &str,
) {
    let schema = load_verify_schema(schema_path, sql_dir);

    let original_sql = read_file(&original);
    let original_stmt = parse_single_query(&original_sql, &original);

    let rewritten_sql = read_file(&rewritten);
    let rewritten_stmt = parse_single_query(&rewritten_sql, &rewritten);

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
        "json" => print_json(&result),
        _ => print_text(&result),
    }
}

// ── Schema loading ───────────────────────────────────────────────────────

/// Load a [`RichSchema`] from either a JSON schema file or a DDL directory.
///
/// Exactly one of `schema_path` or `sql_dir` must be `Some`.
fn load_verify_schema(schema_path: Option<PathBuf>, sql_dir: Option<PathBuf>) -> RichSchema {
    match (schema_path, sql_dir) {
        (Some(p), None) => load_schema_from_json(p),
        (None, Some(dir)) => load_schema_from_dir(dir),
        (None, None) => {
            eprintln!("Error: --schema or --sql-dir is required for verify");
            std::process::exit(1);
        }
        (Some(_), Some(_)) => {
            // clap's conflicts_with prevents this at arg-parsing time
            eprintln!("Error: --schema and --sql-dir are mutually exclusive");
            std::process::exit(1);
        }
    }
}

/// Build a [`RichSchema`] from a JSON schema file.
///
/// The JSON is expected to be a map of table names to column maps
/// (same format as `--schema` for `rewrite`/`suggest`). We synthesize
/// DDL from the map, parse it, and extract the rich schema.
fn load_schema_from_json(path: PathBuf) -> RichSchema {
    let content = read_file(&path);

    let schema_map: std::collections::HashMap<String, std::collections::HashMap<String, String>> =
        serde_json::from_str(&content).unwrap_or_else(|e| {
            eprintln!("Error: invalid schema JSON '{}': {}", path.display(), e);
            std::process::exit(1);
        });

    if schema_map.is_empty() {
        eprintln!("Error: schema JSON '{}' is empty", path.display());
        std::process::exit(1);
    }

    // Build DDL statements from the schema map
    let mut ddl = String::new();
    for (table_name, columns) in &schema_map {
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

    parse_and_extract(&ddl)
}

/// Build a [`RichSchema`] from a directory of `.sql` DDL files.
///
/// Reads all `.sql` files (sorted), concatenates them, parses the
/// combined DDL, and extracts the rich schema.
fn load_schema_from_dir(dir: PathBuf) -> RichSchema {
    if !dir.exists() || !dir.is_dir() {
        eprintln!(
            "Error: schema directory '{}' not found or not a directory",
            dir.display()
        );
        std::process::exit(1);
    }

    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| {
            eprintln!("Error: cannot read directory '{}': {}", dir.display(), e);
            std::process::exit(1);
        })
        .filter_map(|r| r.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case("sql"))
                .unwrap_or(false)
        })
        .collect();

    entries.sort_by_key(|e| e.file_name());

    if entries.is_empty() {
        eprintln!(
            "Error: no .sql files found in schema directory '{}'",
            dir.display()
        );
        std::process::exit(1);
    }

    let mut all_ddl = String::new();
    for entry in &entries {
        let path = entry.path();
        let content = read_sql_file(&path);
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
///
/// Non-DDL statements are silently ignored by `extract_rich_schema`.
fn parse_and_extract(ddl: &str) -> RichSchema {
    let (stmt_infos, _errors) = Parser::parse_sql(ddl);
    let stmts: Vec<Statement> = stmt_infos.into_iter().map(|si| si.statement).collect();
    extract_rich_schema(&stmts)
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

// ── Output formatting ────────────────────────────────────────────────────

/// Print result in human-readable text format.
fn print_text(result: &VerificationResult) {
    match &result.proof {
        metamorphosis_qed::prover::ProofResult::Equivalent => {
            println!("✓ Equivalent (proven in {}ms)", result.elapsed_ms);
        }
        metamorphosis_qed::prover::ProofResult::NotEquivalent { counterexample } => {
            println!("✗ Not Equivalent");

            // Show column info when available
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

/// Print result as JSON.
fn print_json(result: &VerificationResult) {
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
