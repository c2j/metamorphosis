//! Rich schema extraction from DDL statements.
//!
//! Extracts primary keys, foreign keys, NOT NULL, UNIQUE, and CHECK
//! constraints from `CREATE TABLE` statements produced by `ogsql-parser`,
//! producing a [`RichSchema`] suitable for QED verification.

use ogsql_parser::ast::{
    ColumnConstraint, CreateTableStatement, DataType, ObjectName,
    ReferentialAction as AstReferentialAction, Statement, TableConstraint,
};
use std::collections::HashMap;

// ── Types ───────────────────────────────────────────────────────────────

/// Rich schema containing all tables with their columns and constraints.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RichSchema {
    /// Map of normalized (lowercase) table name → table info.
    pub tables: HashMap<String, TableInfo>,
}

impl RichSchema {
    /// Look up a table by name, supporting bare-name fallback for qualified entries.
    ///
    /// Resolution order:
    /// 1. Exact (lowercased) match — O(1), returns immediately.
    /// 2. If `name` contains no `.`, scan for keys ending with `.{lowered_name}`.
    ///    Returns `None` if zero or multiple matches (safe failure on ambiguity).
    pub fn find_table(&self, name: &str) -> Option<&TableInfo> {
        let lower = name.to_lowercase();

        // Step 1: exact match
        if let Some(info) = self.tables.get(&lower) {
            return Some(info);
        }

        // Step 2: if bare name (no dot), try suffix match
        if !lower.contains('.') {
            let dot_suffix = format!(".{lower}");
            let mut matches: Vec<&TableInfo> = self
                .tables
                .iter()
                .filter(|(k, _)| k.ends_with(&dot_suffix))
                .map(|(_, v)| v)
                .collect();

            if matches.len() == 1 {
                return Some(matches.remove(0));
            }
            // 0 matches or ambiguous (2+) — return None
        }

        None
    }
}

/// Constraints attached to a single table.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TableConstraints {
    /// Column names forming the primary key (empty if none).
    pub primary_key: Vec<String>,
    /// Sets of column names with UNIQUE constraints.
    pub unique: Vec<Vec<String>>,
    /// Column names with explicit NOT NULL (beyond PK columns).
    pub not_null: Vec<String>,
    /// CHECK constraints on this table.
    pub check: Vec<CheckConstraint>,
    /// Foreign key relationships.
    pub foreign_keys: Vec<ForeignKeyInfo>,
}

/// Information about a single table's columns and constraints.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TableInfo {
    /// Ordered list of column definitions.
    pub columns: Vec<ColumnInfo>,
    /// All constraints on this table.
    pub constraints: TableConstraints,
}

impl TableInfo {
    /// Returns the 0-based index of the column with the given name, if it exists.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        let lower = name.to_lowercase();
        self.columns.iter().position(|c| c.name == lower)
    }
}

/// Metadata for a single column within a table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ColumnInfo {
    /// Normalized (lowercase) column name.
    pub name: String,
    /// SQL data type string (e.g. `"INTEGER"`, `"VARCHAR(100)"`).
    pub data_type: String,
    /// Whether this column allows `NULL` values.
    pub nullable: bool,
    /// Whether this column is part of the primary key.
    pub is_primary_key: bool,
    /// Whether this column has a UNIQUE constraint.
    pub is_unique: bool,
}

/// A CHECK constraint expression.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckConstraint {
    /// The CHECK expression as a string.
    pub expression: String,
}

/// A foreign key relationship from one table to another.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ForeignKeyInfo {
    /// Columns in this table that form the FK.
    pub columns: Vec<String>,
    /// Referenced (parent) table name.
    pub ref_table: String,
    /// Referenced columns in the parent table.
    pub ref_columns: Vec<String>,
    /// Action taken on delete of a referenced row.
    pub on_delete: Option<ReferentialAction>,
    /// Action taken on update of a referenced row.
    pub on_update: Option<ReferentialAction>,
}

/// Action taken when a referenced row is deleted or updated.
///
/// Mirrors [`ogsql_parser::ast::ReferentialAction`] but owned and
/// `#[non_exhaustive]` for forward compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum ReferentialAction {
    /// `ON DELETE CASCADE` / `ON UPDATE CASCADE`
    Cascade,
    /// `ON DELETE RESTRICT` / `ON UPDATE RESTRICT`
    Restrict,
    /// `ON DELETE SET NULL` / `ON UPDATE SET NULL`
    SetNull,
    /// `ON DELETE SET DEFAULT` / `ON UPDATE SET DEFAULT`
    SetDefault,
    /// `ON DELETE NO ACTION` / `ON UPDATE NO ACTION`
    NoAction,
}

impl From<&AstReferentialAction> for ReferentialAction {
    fn from(action: &AstReferentialAction) -> Self {
        match action {
            AstReferentialAction::Cascade => Self::Cascade,
            AstReferentialAction::Restrict => Self::Restrict,
            AstReferentialAction::SetNull => Self::SetNull,
            AstReferentialAction::SetDefault => Self::SetDefault,
            AstReferentialAction::NoAction => Self::NoAction,
        }
    }
}

// ── Extraction ──────────────────────────────────────────────────────────

/// Extracts a [`RichSchema`] from a slice of parsed SQL statements.
///
/// Walks all `CREATE TABLE` statements and populates column metadata plus
/// both column-level and table-level constraints. Non-DDL statements are
/// silently skipped.
pub fn extract_rich_schema(stmts: &[Statement]) -> RichSchema {
    let mut schema = RichSchema::default();

    for stmt in stmts {
        if let Statement::CreateTable(spanned) = stmt {
            let table_name = normalize_object_name(&spanned.node.name);
            let info = extract_table(&spanned.node);
            schema.tables.insert(table_name, info);
        }
    }

    schema
}

fn extract_table(stmt: &CreateTableStatement) -> TableInfo {
    let mut columns: Vec<ColumnInfo> = Vec::with_capacity(stmt.columns.len());
    let mut constraints = TableConstraints::default();

    for col_def in &stmt.columns {
        let mut col = ColumnInfo {
            name: col_def.name.to_lowercase(),
            data_type: data_type_to_string(&col_def.data_type),
            nullable: true,
            is_primary_key: false,
            is_unique: false,
        };

        for c in &col_def.constraints {
            apply_column_constraint(c, &mut col, &mut constraints);
        }

        columns.push(col);
    }

    for tc in &stmt.constraints {
        apply_table_constraint(tc, &mut columns, &mut constraints);
    }

    TableInfo {
        columns,
        constraints,
    }
}

fn apply_column_constraint(
    c: &ColumnConstraint,
    col: &mut ColumnInfo,
    constraints: &mut TableConstraints,
) {
    match c {
        ColumnConstraint::NotNull => {
            col.nullable = false;
            constraints.not_null.push(col.name.clone());
        }
        ColumnConstraint::Null => {
            col.nullable = true;
        }
        ColumnConstraint::Unique => {
            col.is_unique = true;
            constraints.unique.push(vec![col.name.clone()]);
        }
        ColumnConstraint::PrimaryKey => {
            col.is_primary_key = true;
            col.nullable = false;
            constraints.primary_key.push(col.name.clone());
        }
        ColumnConstraint::Check(expr) => {
            constraints.check.push(CheckConstraint {
                expression: format!("{expr:?}"),
            });
        }
        ColumnConstraint::References {
            ref_table,
            ref_columns,
            on_delete,
            on_update,
        } => {
            constraints.foreign_keys.push(ForeignKeyInfo {
                columns: vec![col.name.clone()],
                ref_table: normalize_object_name(ref_table),
                ref_columns: ref_columns.iter().map(|s| s.to_lowercase()).collect(),
                on_delete: on_delete.as_ref().map(ReferentialAction::from),
                on_update: on_update.as_ref().map(ReferentialAction::from),
            });
        }
        ColumnConstraint::Default(_) => {}
    }
}

fn apply_table_constraint(
    tc: &TableConstraint,
    columns: &mut [ColumnInfo],
    constraints: &mut TableConstraints,
) {
    match tc {
        TableConstraint::PrimaryKey {
            columns: pk_cols, ..
        } => {
            constraints.primary_key = pk_cols.iter().map(|s| s.to_lowercase()).collect();
            for pk_name in &constraints.primary_key {
                if let Some(col) = columns.iter_mut().find(|c| c.name == *pk_name) {
                    col.is_primary_key = true;
                    col.nullable = false;
                }
            }
        }
        TableConstraint::Unique {
            columns: u_cols, ..
        } => {
            constraints
                .unique
                .push(u_cols.iter().map(|s| s.to_lowercase()).collect());
            for u_name in u_cols {
                let lower = u_name.to_lowercase();
                if let Some(col) = columns.iter_mut().find(|c| c.name == lower) {
                    col.is_unique = true;
                }
            }
        }
        TableConstraint::Check(expr) => {
            constraints.check.push(CheckConstraint {
                expression: format!("{expr:?}"),
            });
        }
        TableConstraint::ForeignKey {
            columns: fk_cols,
            ref_table,
            ref_columns,
            on_delete,
            on_update,
        } => {
            constraints.foreign_keys.push(ForeignKeyInfo {
                columns: fk_cols.iter().map(|s| s.to_lowercase()).collect(),
                ref_table: normalize_object_name(ref_table),
                ref_columns: ref_columns.iter().map(|s| s.to_lowercase()).collect(),
                on_delete: on_delete.as_ref().map(ReferentialAction::from),
                on_update: on_update.as_ref().map(ReferentialAction::from),
            });
        }
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

fn format_timestamp_type(
    base: &str,
    precision: Option<u32>,
    tz: &Option<ogsql_parser::ast::TimeZoneInfo>,
) -> String {
    let base_str = match precision {
        Some(p) => format!("{base}({p})"),
        None => base.to_string(),
    };
    match tz {
        Some(ogsql_parser::ast::TimeZoneInfo::WithTimeZone) => {
            format!("{base_str} WITH TIME ZONE")
        }
        Some(ogsql_parser::ast::TimeZoneInfo::WithoutTimeZone) => {
            format!("{base_str} WITHOUT TIME ZONE")
        }
        None => base_str,
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_and_extract(sql: &str) -> RichSchema {
        let (stmts, _errors) = ogsql_parser::Parser::parse_sql(sql);
        let stmts: Vec<_> = stmts.into_iter().map(|si| si.statement).collect();
        extract_rich_schema(&stmts)
    }

    #[test]
    fn test_extract_simple_pk() {
        let schema = parse_and_extract(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR(100) NOT NULL)",
        );
        assert!(schema.tables.contains_key("users"));
        let users = &schema.tables["users"];
        assert_eq!(users.column_index("id"), Some(0));
        assert_eq!(users.column_index("name"), Some(1));
        assert!(users.columns[0].is_primary_key);
        assert!(!users.columns[0].nullable);
        assert!(!users.columns[1].nullable);
    }

    #[test]
    fn test_extract_table_level_pk() {
        let schema = parse_and_extract(
            "CREATE TABLE orders (order_id INTEGER, user_id INTEGER, amount NUMERIC, \
             PRIMARY KEY (order_id, user_id))",
        );
        let orders = &schema.tables["orders"];
        assert_eq!(orders.constraints.primary_key, vec!["order_id", "user_id"]);
        assert!(orders.columns[0].is_primary_key);
        assert!(orders.columns[1].is_primary_key);
        assert!(!orders.columns[0].nullable);
        assert!(!orders.columns[1].nullable);
        assert!(orders.columns[2].nullable);
    }

    #[test]
    fn test_extract_not_null() {
        let schema = parse_and_extract("CREATE TABLE items (id INTEGER NOT NULL, label TEXT)");
        let items = &schema.tables["items"];
        assert!(!items.columns[0].nullable);
        assert!(items.columns[1].nullable);
        assert!(items.constraints.not_null.contains(&"id".to_string()));
    }

    #[test]
    fn test_extract_unique() {
        let schema = parse_and_extract(
            "CREATE TABLE accounts (id INTEGER, email VARCHAR(255) UNIQUE, \
             CONSTRAINT uq_name_email UNIQUE (id, email))",
        );
        let accounts = &schema.tables["accounts"];
        assert!(accounts.columns[1].is_unique);
        assert!(accounts.columns[0].is_unique);
        assert!(accounts
            .constraints
            .unique
            .contains(&vec!["email".to_string()]));
    }

    #[test]
    fn test_extract_check() {
        let schema = parse_and_extract("CREATE TABLE products (price NUMERIC CHECK (price > 0))");
        let products = &schema.tables["products"];
        assert_eq!(products.constraints.check.len(), 1);
        assert!(products.constraints.check[0].expression.contains("price"));
    }

    #[test]
    fn test_extract_foreign_key() {
        let schema = parse_and_extract(
            "CREATE TABLE orders (id INTEGER, user_id INTEGER REFERENCES users(id))",
        );
        let orders = &schema.tables["orders"];
        assert_eq!(orders.constraints.foreign_keys.len(), 1);
        let fk = &orders.constraints.foreign_keys[0];
        assert_eq!(fk.columns, vec!["user_id"]);
        assert_eq!(fk.ref_table, "users");
        assert_eq!(fk.ref_columns, vec!["id"]);
    }

    #[test]
    fn test_column_index_resolution() {
        let schema = parse_and_extract("CREATE TABLE t (Alpha INTEGER, Beta TEXT, Gamma BOOLEAN)");
        let t = &schema.tables["t"];
        assert_eq!(t.column_index("alpha"), Some(0));
        assert_eq!(t.column_index("Beta"), Some(1));
        assert_eq!(t.column_index("GAMMA"), Some(2));
        assert_eq!(t.column_index("delta"), None);
    }

    #[test]
    fn test_pk_implies_not_null() {
        let schema = parse_and_extract("CREATE TABLE pk_test (a INTEGER, b TEXT, PRIMARY KEY (a))");
        let t = &schema.tables["pk_test"];
        assert!(!t.columns[0].nullable, "PK column 'a' must be NOT NULL");
        assert!(
            t.columns[1].nullable,
            "non-PK column 'b' should remain nullable"
        );
    }

    // ── find_table tests ───────────────────────────────────────────────────

    #[test]
    fn find_table_exact_match() {
        let mut schema = RichSchema::default();
        let info = TableInfo {
            columns: vec![ColumnInfo {
                name: "id".to_string(),
                data_type: "INTEGER".to_string(),
                nullable: false,
                is_primary_key: true,
                is_unique: true,
            }],
            constraints: TableConstraints::default(),
        };
        schema.tables.insert("users".to_string(), info.clone());
        assert!(schema.find_table("users").is_some());
        assert!(schema.find_table("USERS").is_some());
    }

    #[test]
    fn find_table_bare_to_qualified() {
        let mut schema = RichSchema::default();
        let info = TableInfo {
            columns: vec![ColumnInfo {
                name: "id".to_string(),
                data_type: "INTEGER".to_string(),
                nullable: false,
                is_primary_key: true,
                is_unique: true,
            }],
            constraints: TableConstraints::default(),
        };
        schema
            .tables
            .insert("public.users".to_string(), info.clone());
        // Bare name finds the qualified entry
        assert!(schema.find_table("users").is_some());
        // Simple name that doesn't exist as suffix
        assert!(schema.find_table("nonexistent").is_none());
    }

    #[test]
    fn find_table_ambiguous_returns_none() {
        let mut schema = RichSchema::default();
        let info = TableInfo {
            columns: vec![],
            constraints: TableConstraints::default(),
        };
        schema
            .tables
            .insert("schema_a.users".to_string(), info.clone());
        schema
            .tables
            .insert("schema_b.users".to_string(), info);
        // Ambiguous — two schemas have the same bare name
        assert!(schema.find_table("users").is_none());
    }

    #[test]
    fn find_table_qualified_not_found_returns_none() {
        let mut schema = RichSchema::default();
        let info = TableInfo {
            columns: vec![],
            constraints: TableConstraints::default(),
        };
        schema.tables.insert("public.users".to_string(), info);
        // If the user provides a qualified name that doesn't exist exactly, no suffix fallback
        // for dot-containing names
        assert!(schema.find_table("other.users").is_none());
    }
}
