//! DDL-driven schema extraction.
//!
//! Scans a directory of SQL files, parses each with `ogsql-parser`,
//! collects `CREATE TABLE` and `ALTER TABLE ... ADD COLUMN` statements,
//! and builds a [`SchemaMap`] mapping table names to column definitions.
//!
//! # Example
//!
//! ```ignore
//! use metamorphosis_core::extractor::extract_schema_from_dir;
//!
//! let schema = extract_schema_from_dir(std::path::Path::new("./sql/ddl/"))
//!     .expect("schema extraction failed");
//! ```

use ogsql_parser::analyzer::schema::SchemaMap;
use ogsql_parser::ast::{AlterTableAction, DataType, ObjectName, Statement};
use ogsql_parser::{ParseOptions, Parser};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

/// Errors that can occur during schema extraction.
#[derive(Debug, Error)]
pub enum ExtractionError {
    /// The specified directory does not exist or is not a directory.
    #[error("schema directory not found: {0}")]
    DirNotFound(String),

    /// Failed to read the directory listing.
    #[error("failed to read schema directory '{0}': {1}")]
    DirReadError(String, String),

    /// No `*.sql` files were found in the given directory.
    #[error("no .sql files found in schema directory: {0}")]
    NoSqlFiles(String),

    /// All SQL files were skipped (unreadable or parse errors) and
    /// no schema could be extracted.
    #[error("could not extract any schema from '{0}': all .sql files were skipped")]
    AllFilesSkipped(String),
}

/// Extract a [`SchemaMap`] from all `*.sql` files in the given directory.
///
/// The directory is scanned (sorted for deterministic ordering), each
/// `.sql` file is parsed with `ogsql-parser`, and every
/// `CREATE TABLE` / `ALTER TABLE ... ADD COLUMN` statement is collected
/// into the resulting map.
///
/// Files that cannot be read (non-UTF8) or contain parse errors are
/// **skipped** with a warning — a single bad file does not block
/// extraction from other files. Only if *all* files are skipped or the
/// directory itself is inaccessible does this function return an error.
///
/// Table and column names are lowercased for case-insensitive lookups.
/// If the same table appears in multiple `CREATE TABLE` statements, the
/// last definition wins (supporting `DROP … CREATE` patterns).
pub fn extract_schema_from_dir(dir: &Path) -> Result<SchemaMap, ExtractionError> {
    if !dir.exists() {
        return Err(ExtractionError::DirNotFound(dir.display().to_string()));
    }
    if !dir.is_dir() {
        return Err(ExtractionError::DirNotFound(dir.display().to_string()));
    }

    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| ExtractionError::DirReadError(dir.display().to_string(), e.to_string()))?
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
        return Err(ExtractionError::NoSqlFiles(dir.display().to_string()));
    }

    let mut schema: SchemaMap = HashMap::new();
    let mut any_processed = false;

    for entry in &entries {
        let path = entry.path();
        let path_str = path.display().to_string();

        let content = match read_sql_file(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Skipping '{}': ({})", path_str, e);
                continue;
            }
        };

        let parse_output = Parser::parse_sql_with_options(
            &content,
            ParseOptions {
                preserve_comments: false,
                mybatis_params: false,
            },
        );

        // Separate warnings (non-fatal) from real errors.
        let real_errors: Vec<_> = parse_output
            .errors
            .iter()
            .filter(|e| !is_warning(e))
            .collect();
        let warnings: Vec<_> = parse_output
            .errors
            .iter()
            .filter(|e| is_warning(e))
            .collect();

        if !warnings.is_empty() {
            for w in &warnings {
                tracing::debug!(
                    "Warning in '{}' (line {}): {}",
                    path_str,
                    warning_line(w),
                    w
                );
            }
        }

        if !real_errors.is_empty() {
            let detail = real_errors
                .iter()
                .map(|e| format!("{e:?}"))
                .collect::<Vec<_>>()
                .join("; ");
            tracing::warn!("Skipping '{}': parse error(s) ({})", path_str, detail);
            continue;
        }

        any_processed = true;

        for stmt_info in &parse_output.statements {
            apply_statement(&mut schema, &stmt_info.statement);
        }
    }

    if schema.is_empty() {
        if any_processed {
            // Files were parsed but none contained DDL — not an error,
            // just an empty schema.
            tracing::info!(
                "Parsed {} file(s) but found no CREATE TABLE statements in '{}'",
                entries.len(),
                dir.display()
            );
        } else {
            return Err(ExtractionError::AllFilesSkipped(dir.display().to_string()));
        }
    }

    Ok(schema)
}

/// Read a SQL file to string, delegating encoding detection to
/// `ogsql-parser`'s `token::decode_sql_file`.
///
/// Supports UTF-8, GB18030/GBK (Chinese), EUC-JP (Japanese),
/// EUC-KR (Korean), BIG5 (Traditional Chinese), UTF-16 LE/BE,
/// and falls back to lossy UTF-8 if all else fails.
///
/// Byte-order marks (UTF-8 BOM `\xEF\xBB\xBF`, UTF-16LE BOM `\xFF\xFE`,
/// UTF-16BE BOM `\xFE\xFF`) are stripped before decoding, since they
/// are not valid SQL syntax and would cause parse errors.
fn read_sql_file(path: &Path) -> Result<String, String> {
    let mut bytes = std::fs::read(path).map_err(|e| format!("cannot read: {e}"))?;

    // Strip BOM so the parser doesn't choke on \uFEFF as SQL syntax.
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        // UTF-8 BOM
        bytes.drain(..3);
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        // UTF-16LE BOM (decode_sql_file handles the actual encoding)
        bytes.drain(..2);
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        // UTF-16BE BOM
        bytes.drain(..2);
    }

    ogsql_parser::token::decode_sql_file(&bytes)
        .map(|(s, _)| s)
        .map_err(|e| format!("decode failed: {e}"))
}

fn apply_statement(schema: &mut SchemaMap, stmt: &Statement) {
    match stmt {
        Statement::CreateTable(spanned) => {
            let table_name = normalize_object_name(&spanned.node.name);
            let mut columns: HashMap<String, String> = HashMap::new();
            for col in &spanned.node.columns {
                columns.insert(col.name.to_lowercase(), data_type_to_string(&col.data_type));
            }
            schema.insert(table_name, columns);
        }
        Statement::CreateTableAs(spanned) => {
            let table_name = normalize_object_name(&spanned.node.name);
            let mut columns: HashMap<String, String> = HashMap::new();
            for col_name in &spanned.node.column_names {
                columns.insert(col_name.to_lowercase(), "unknown".to_string());
            }
            schema.insert(table_name, columns);
        }
        Statement::AlterTable(spanned) => {
            let table_name = normalize_object_name(&spanned.node.name);
            let table_entry = schema.entry(table_name).or_default();
            for action in &spanned.node.actions {
                apply_alter_action(table_entry, action);
            }
        }
        _ => {}
    }
}

fn apply_alter_action(columns: &mut HashMap<String, String>, action: &AlterTableAction) {
    match action {
        AlterTableAction::AddColumn(col_def) => {
            columns.insert(
                col_def.name.to_lowercase(),
                data_type_to_string(&col_def.data_type),
            );
        }
        AlterTableAction::DropColumn { name, .. } => {
            columns.remove(&name.to_lowercase());
        }
        AlterTableAction::RenameColumn { old, new } => {
            if let Some(typ) = columns.remove(&old.to_lowercase()) {
                columns.insert(new.to_lowercase(), typ);
            }
        }
        AlterTableAction::AlterColumn { .. } => {}
        _ => {}
    }
}

fn normalize_object_name(name: &ObjectName) -> String {
    name.iter()
        .map(|s| s.to_lowercase())
        .collect::<Vec<_>>()
        .join(".")
}

fn data_type_to_string(dt: &DataType) -> String {
    match dt {
        DataType::Boolean => "BOOLEAN".to_string(),
        DataType::TinyInt(p) => format_int_type("TINYINT", *p),
        DataType::SmallInt(p) => format_int_type("SMALLINT", *p),
        DataType::Integer(p) => format_int_type("INTEGER", *p),
        DataType::BigInt(p) => format_int_type("BIGINT", *p),
        DataType::Real => "REAL".to_string(),
        DataType::Float(p) => format_int_type("FLOAT", *p),
        DataType::Double => "DOUBLE".to_string(),
        DataType::Numeric(p, s) => format_numeric_type("NUMERIC", *p, *s),
        DataType::Char(p) => format_int_type("CHAR", *p),
        DataType::Varchar(p) => format_int_type("VARCHAR", *p),
        DataType::Text => "TEXT".to_string(),
        DataType::Bytea => "BYTEA".to_string(),
        DataType::Timestamp(p, tz) => format_timestamp_type("TIMESTAMP", *p, tz),
        DataType::Timestamptz(p) => format_int_type("TIMESTAMPTZ", *p),
        DataType::Date => "DATE".to_string(),
        DataType::Time(p, tz) => format_timestamp_type("TIME", *p, tz),
        DataType::Interval(..) => "INTERVAL".to_string(),
        DataType::Json => "JSON".to_string(),
        DataType::Jsonb => "JSONB".to_string(),
        DataType::Uuid => "UUID".to_string(),
        DataType::Bit(p) => format_int_type("BIT", *p),
        DataType::Varbit(p) => format_int_type("VARBIT", *p),
        DataType::Serial => "SERIAL".to_string(),
        DataType::SmallSerial => "SMALLSERIAL".to_string(),
        DataType::BigSerial => "BIGSERIAL".to_string(),
        DataType::BinaryFloat => "BINARY_FLOAT".to_string(),
        DataType::BinaryDouble => "BINARY_DOUBLE".to_string(),
        DataType::Array(inner) => format!("{}[]", data_type_to_string(inner)),
        DataType::Custom(name, _) => name.join("."),
    }
}

fn format_int_type(base: &str, precision: Option<u32>) -> String {
    match precision {
        Some(p) => format!("{base}({p})"),
        None => base.to_string(),
    }
}

fn format_numeric_type(base: &str, precision: Option<u32>, scale: Option<u32>) -> String {
    match (precision, scale) {
        (Some(p), Some(s)) => format!("{base}({p},{s})"),
        (Some(p), None) => format!("{base}({p})"),
        (None, _) => base.to_string(),
    }
}

/// Returns `true` if a `ParserError` is a non-fatal warning.
///
/// Matches the same classification used by ogsql-parser's own CLI:
/// `Warning` and `ReservedKeywordAsIdentifier` are treated as warnings,
/// everything else is a real error.
fn is_warning(e: &ogsql_parser::ParserError) -> bool {
    matches!(
        e,
        ogsql_parser::ParserError::Warning { .. }
            | ogsql_parser::ParserError::ReservedKeywordAsIdentifier { .. }
    )
}

/// Extract the line number from a `ParserError`, defaulting to 0.
fn warning_line(e: &ogsql_parser::ParserError) -> usize {
    use ogsql_parser::ParserError;
    match e {
        ParserError::Warning { location, .. } => location.line,
        ParserError::ReservedKeywordAsIdentifier { location, .. } => location.line,
        _ => 0,
    }
}

fn format_timestamp_type(
    base: &str,
    precision: Option<u32>,
    tz: &Option<ogsql_parser::ast::TimeZoneInfo>,
) -> String {
    let base = match precision {
        Some(p) => format!("{base}({p})"),
        None => base.to_string(),
    };
    match tz {
        Some(ogsql_parser::ast::TimeZoneInfo::WithTimeZone) => format!("{base} WITH TIME ZONE"),
        Some(ogsql_parser::ast::TimeZoneInfo::WithoutTimeZone) => {
            format!("{base} WITHOUT TIME ZONE")
        }
        None => base,
    }
}

#[cfg(test)]
mod tests;
