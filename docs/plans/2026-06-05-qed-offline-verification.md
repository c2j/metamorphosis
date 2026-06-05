# QED Offline Verification — Implementation Plan (方案A)

> **For Claude:** REQUIRED SUB-SKILL: Use `executing-plans` to implement this plan task-by-task.

**Goal:** Build an offline verification pipeline that uses the QED prover to prove semantic equivalence of SQL rewrites produced by Metamorphosis rules. This is Phase A of the QED integration roadmap (offline → embedded SMT → full integration).

**Why:** Every `SafetyLevel::Safe` rule in Metamorphosis claims semantic equivalence. Today `validate_statement()` only checks syntax (format → re-parse). QED gives us mathematical proof that `original SQL ≡ rewritten SQL` under bag semantics with NULL, constraints, and aggregates.

**Architecture:** Side-car pipeline — no changes to the runtime `RewriteEngine`. New crate `crates/qed/` owns the verification pipeline. Consumes `(original_stmt, rewritten_stmt, schema_ddl)` tuples, translates to QED JSON, invokes the QED prover binary, parses proof results.

**Key Constraints from `docs/CONTRIBUTING.md`:**
- Core crate: zero IO dependencies — QED integration goes in `crates/qed/`, NOT in `crates/core/`
- All `pub` items need doc comments, `#[non_exhaustive]` on exported structs/enums
- Max 600 lines per `.rs` (entry files ≤200), rustfmt enforced, no bare `as` casts
- `thiserror` for library errors, `tracing` for logging
- No `unwrap()` in library code

**QED Prover Reference:** https://github.com/qed-solver/prover (Rust 81.6%, MIT license)
- CLI: `qed-prover input.json` or `qed-prover tests/calcite/`
- Input: JSON with `schemas` (table definitions + constraints), `queries` (two Relation trees), `help` (description)

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│  Metamorphosis Test / CI Pipeline                           │
│                                                             │
│  For each (rule, test_case):                                │
│    1. Parse original SQL  → ogsql-parser AST                │
│    2. Apply rule          → rewritten AST                   │
│    3. Extract constraints from DDL → RichSchema             │
│    4. Translate (original, rewritten) + RichSchema          │
│       → QED JSON input                                      │
│    5. Invoke qed-prover binary → proof result               │
│    6. Assert Equivalent                                     │
└─────────────────────────────────────────────────────────────┘

Component Map:
  crates/core/src/extractor/     → extend with ConstraintExtractor
  crates/qed/                    → NEW crate
    src/lib.rs                   → crate root
    src/schema.rs                → RichSchema + ConstraintExtractor
    src/ir.rs                    → QED IR types (Rust → QED JSON)
    src/translator.rs            → ogsql-parser AST → QED Relation
    src/prover.rs                → QED prover harness (process invocation)
    src/verify.rs                → end-to-end verification pipeline
```

**Data Flow:**

```
DDL Files (*.sql)
     │
     ▼
ConstraintExtractor (ogsql-parser AST → RichSchema)
     │
     ├── table_name → TableConstraints { pk, unique, not_null, check, fk }
     │
     ▼
AST Translator (ogsql-parser Statement → QED Relation)
     │
     ├── maps column names → column indices (0-based)
     ├── maps SQL operators → QED Relation variants
     ├── maps GaussDB functions → QOp (uninterpreted)
     │
     ▼
QED IR → serde_json → input.json
     │
     ▼
qed-prover input.json → Equivalent | NotEquivalent | Unknown | Timeout
```

---

## Component 1: RichSchema + ConstraintExtractor

**Status:** NEW — extends existing `crates/core/src/extractor/mod.rs`
**File:** `crates/qed/src/schema.rs` (placed in `crates/qed/` because core must stay IO-free; extractor stays in core for basic name+type extraction, constraint extraction moves to qed crate)

### Problem

Current `SchemaMap = HashMap<String, HashMap<String, String>>` only stores `table → column → type`. It knows nothing about:

- Primary keys (`TableConstraint::PrimaryKey { columns, .. }`)
- NOT NULL constraints (`ColumnConstraint::NotNull`)
- UNIQUE constraints (`TableConstraint::Unique { columns, .. }` / `ColumnConstraint::Unique`)
- CHECK constraints (`TableConstraint::Check(Expr)` / `ColumnConstraint::Check(Expr)`)
- Foreign keys (`TableConstraint::ForeignKey { .. }` / `ColumnConstraint::References { .. }`)

But ogsql-parser's AST already parses all of these:

```rust
// From ogsql-parser ast/mod.rs:
pub struct CreateTableStatement {
    pub columns: Vec<ColumnDef>,        // each has .constraints: Vec<ColumnConstraint>
    pub constraints: Vec<TableConstraint>, // table-level PK, FK, UNIQUE, CHECK
    ...
}

pub enum ColumnConstraint {
    NotNull, Null, Default(Expr), Unique, PrimaryKey,
    Check(Expr),
    References { ref_table, ref_columns, on_delete, on_update },
}

pub enum TableConstraint {
    PrimaryKey { columns, using_index },
    Unique { columns, deferrable, ... },
    Check(Expr),
    ForeignKey { columns, ref_table, ref_columns, on_delete, on_update },
}
```

### Design

```rust
/// Rich schema with full constraint information, suitable for QED translation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RichSchema {
    pub tables: HashMap<String, TableInfo>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TableInfo {
    /// Ordered column definitions (order matters for QED's 0-based column indexing).
    pub columns: Vec<ColumnInfo>,
    /// Table-level constraints.
    pub constraints: TableConstraints,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    /// Column-level constraints (NOT NULL already captured in `nullable`).
    pub is_primary_key: bool,
    pub is_unique: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TableConstraints {
    /// Primary key column names (in declaration order).
    pub primary_key: Option<Vec<String>>,
    /// Unique constraint sets (each entry is a set of column names).
    pub unique: Vec<Vec<String>>,
    /// NOT NULL columns (already in ColumnInfo.nullable, but collected here for convenience).
    pub not_null: Vec<String>,
    /// Check constraints (as ogsql-parser Expr, serialized).
    pub check: Vec<CheckConstraint>,
    /// Foreign key constraints.
    pub foreign_keys: Vec<ForeignKeyInfo>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckConstraint {
    /// The check expression, stored as formatted SQL text.
    pub expression: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ForeignKeyInfo {
    /// Columns in this table.
    pub columns: Vec<String>,
    /// Referenced table name.
    pub ref_table: String,
    /// Referenced columns in the other table.
    pub ref_columns: Vec<String>,
    pub on_delete: Option<ReferentialAction>,
    pub on_update: Option<ReferentialAction>,
}
```

### ConstraintExtractor Implementation

```rust
/// Extracts a [`RichSchema`] from parsed DDL statements.
///
/// Walks ogsql-parser AST for `CREATE TABLE` statements, extracting both
/// column-level constraints (from `ColumnDef.constraints`) and table-level
/// constraints (from `CreateTableStatement.constraints`).
pub fn extract_rich_schema(stmts: &[Statement]) -> RichSchema {
    let mut tables = HashMap::new();
    for stmt in stmts {
        if let Statement::CreateTable(spanned) = stmt {
            let table = extract_table_info(&spanned.node);
            let name = normalize_object_name(&spanned.node.name);
            tables.insert(name, table);
        }
    }
    RichSchema { tables }
}

fn extract_table_info(ct: &CreateTableStatement) -> TableInfo {
    let mut columns = Vec::new();
    let mut constraints = TableConstraints::default();

    // Pass 1: Extract column info from ColumnDef
    for col_def in &ct.columns {
        let mut col = ColumnInfo {
            name: col_def.name.to_lowercase(),
            data_type: data_type_to_string(&col_def.data_type),
            nullable: true,  // default
            is_primary_key: false,
            is_unique: false,
        };

        for cc in &col_def.constraints {
            match cc {
                ColumnConstraint::NotNull => { col.nullable = false; }
                ColumnConstraint::Null => { col.nullable = true; }
                ColumnConstraint::Unique => { col.is_unique = true; }
                ColumnConstraint::PrimaryKey => {
                    col.is_primary_key = true;
                    col.nullable = false;  // PK implies NOT NULL
                }
                ColumnConstraint::Check(expr) => {
                    constraints.check.push(CheckConstraint {
                        expression: format_expr(expr),
                    });
                }
                ColumnConstraint::References { ref_table, ref_columns, .. } => {
                    constraints.foreign_keys.push(ForeignKeyInfo {
                        columns: vec![col.name.clone()],
                        ref_table: normalize_object_name(ref_table),
                        ref_columns: ref_columns.iter().map(|c| c.to_lowercase()).collect(),
                        on_delete: None,  // TODO: map ReferentialAction
                        on_update: None,
                    });
                }
                _ => {}
            }
        }

        if !col.nullable {
            constraints.not_null.push(col.name.clone());
        }

        columns.push(col);
    }

    // Pass 2: Extract table-level constraints
    for tc in &ct.constraints {
        match tc {
            TableConstraint::PrimaryKey { columns: pk_cols, .. } => {
                constraints.primary_key = Some(
                    pk_cols.iter().map(|c| c.to_lowercase()).collect()
                );
                // PK columns are NOT NULL
                for col_name in pk_cols {
                    let name_lower = col_name.to_lowercase();
                    if !constraints.not_null.contains(&name_lower) {
                        constraints.not_null.push(name_lower);
                    }
                    // Mark column as PK
                    if let Some(col) = columns.iter_mut().find(|c| c.name == name_lower) {
                        col.is_primary_key = true;
                        col.nullable = false;
                    }
                }
            }
            TableConstraint::Unique { columns: uq_cols, .. } => {
                constraints.unique.push(
                    uq_cols.iter().map(|c| c.to_lowercase()).collect()
                );
            }
            TableConstraint::Check(expr) => {
                constraints.check.push(CheckConstraint {
                    expression: format_expr(expr),
                });
            }
            TableConstraint::ForeignKey { columns: fk_cols, ref_table, ref_columns, on_delete, on_update } => {
                constraints.foreign_keys.push(ForeignKeyInfo {
                    columns: fk_cols.iter().map(|c| c.to_lowercase()).collect(),
                    ref_table: normalize_object_name(ref_table),
                    ref_columns: ref_columns.iter().map(|c| c.to_lowercase()).collect(),
                    on_delete: map_referential_action(on_delete),
                    on_update: map_referential_action(on_update),
                });
            }
        }
    }

    TableInfo { columns, constraints }
}
```

**Column name → index mapping** (critical for QED which uses 0-based indices):

```rust
impl TableInfo {
    /// Returns the 0-based column index for a column name.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name.to_lowercase())
    }
}
```

---

## Component 2: QED IR Types

**Status:** NEW
**File:** `crates/qed/src/ir.rs`

These Rust types map directly to QED prover's JSON input format. Serialized via `serde_json` and passed to `qed-prover`.

### QED JSON Input Format (from prover source)

```json
{
  "schemas": [
    {
      "name": "R",
      "types": ["integer", "integer"],
      "key": [0],
      "nullable": [false, true],
      "guaranteed": ["x > 0"],
      "fields": ["x", "y"]
    }
  ],
  "queries": [
    { "Scan": { "table": "R", "fields": [0, 1] } },
    { "Filter": { "condition": "...", "input": { "Scan": ... } } }
  ],
  "help": "Verify that SELECT x FROM R WHERE x > 0 equals ..."
}
```

### Rust IR Types

```rust
use serde::{Deserialize, Serialize};

/// Top-level QED prover input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QedInput {
    /// Table schemas with constraints.
    pub schemas: Vec<QedSchema>,
    /// Exactly two queries to compare for equivalence.
    pub queries: [QedRelation; 2],
    /// Human-readable description of the equivalence claim.
    pub help: String,
}

/// A single table's schema in QED format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QedSchema {
    /// Table name.
    pub name: String,
    /// SQL types for each column (as strings).
    pub types: Vec<String>,
    /// Column indices that form the primary key (0-based).
    /// Empty vec = no primary key declared.
    pub key: Vec<usize>,
    /// Which columns are nullable (parallel to `fields`).
    pub nullable: Vec<bool>,
    /// CHECK constraints as string expressions.
    /// QED prover interprets these as guaranteed assertions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guaranteed: Vec<String>,
    /// Column names in declaration order.
    pub fields: Vec<String>,
}

/// Recursive Relation type matching QED prover's Relation enum.
///
/// Covers: Scan, Filter, Project, Join, Union, Intersect, Except,
/// Distinct, Values, Aggregate, Group, Sort, Correlate, Singleton.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum QedRelation {
    Scan {
        table: String,
        /// Column indices to project (0-based). Empty = all columns.
        #[serde(default)]
        fields: Vec<usize>,
    },
    Filter {
        condition: QedExpr,
        input: Box<QedRelation>,
    },
    Project {
        /// Output column expressions.
        exprs: Vec<QedExpr>,
        input: Box<QedRelation>,
    },
    Join {
        left: Box<QedRelation>,
        right: Box<QedRelation>,
        /// Join condition. None = cross join.
        #[serde(skip_serializing_if = "Option::is_none")]
        condition: Option<QedExpr>,
    },
    Union {
        left: Box<QedRelation>,
        right: Box<QedRelation>,
    },
    Intersect {
        left: Box<QedRelation>,
        right: Box<QedRelation>,
    },
    Except {
        left: Box<QedRelation>,
        right: Box<QedRelation>,
    },
    Distinct {
        input: Box<QedRelation>,
    },
    Values {
        rows: Vec<Vec<QedExpr>>,
    },
    Aggregate {
        /// Group key column indices.
        keys: Vec<usize>,
        /// Aggregate function calls.
        aggs: Vec<QedAggCall>,
        input: Box<QedRelation>,
    },
    /// Uninterpreted operator (LIMIT, OFFSET, GaussDB-specific functions).
    QOp {
        name: String,
        args: Vec<QedExpr>,
        input: Box<QedRelation>,
    },
}

/// Expression language for QED relations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum QedExpr {
    /// Column reference by 0-based index.
    ColumnRef { index: usize },
    /// Literal value.
    Literal { value: QedValue },
    /// Binary operation.
    BinOp { op: String, left: Box<QedExpr>, right: Box<QedExpr> },
    /// Unary operation.
    UnOp { op: String, expr: Box<QedExpr> },
    /// Function call (interpreted or uninterpreted).
    Function { name: String, args: Vec<QedExpr> },
    /// Null literal.
    Null,
    /// Comparison with quantifier (SOME, ALL, EXISTS).
    Quantified { cmp: String, quantifier: String, subquery: Box<QedRelation> },
}

/// Aggregate function call in QED.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QedAggCall {
    pub func: String,        // "sum", "count", "max", "min", "avg"
    pub arg: QedAggArg,
    pub distinct: bool,
}

/// Argument to an aggregate function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QedAggArg {
    /// All rows (COUNT(*)).
    Star,
    /// Specific column or expression.
    Expr(QedExpr),
}

/// Literal value types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum QedValue {
    Integer { value: i64 },
    Float { value: f64 },
    String { value: String },
    Boolean { value: bool },
}
```

---

## Component 3: AST → QED Relation Translator

**Status:** NEW — the core translation engine
**File:** `crates/qed/src/translator.rs`

This is the most complex component. It walks an `ogsql_parser::ast::Statement` and produces a `QedRelation` tree.

### Translation Strategy

```
ogsql-parser AST              QED Relation
─────────────────             ──────────────
Statement::Select      →      QedRelation (root)
  SelectStatement.targets →    Project { exprs, input }
  SelectStatement.from   →      Scan / Join (recursive)
  SelectStatement.where  →      Filter { condition, input }
  SelectStatement.group_by →    Aggregate { keys, aggs, input }
  SelectStatement.having →      Filter { condition, Aggregate }
  SelectStatement.distinct →    Distinct { input }
  SelectStatement.order_by →    QOp { "sort", ... }
  SelectStatement.limit   →    QOp { "limit", ... }

Subquery (FROM)        →      recursive translation
Subquery (WHERE)       →      Quantified or correlated subquery
Expr::ColumnRef        →      QedExpr::ColumnRef { index }
Expr::BinaryOp         →      QedExpr::BinOp
Expr::FunctionCall     →      QedExpr::Function (or QOp if GaussDB-specific)
```

### Column Name → Index Resolution

The translator maintains a **column scope** during translation:

```rust
/// Tracks column name → QED index mapping during translation.
struct ColumnScope {
    /// Ordered list of (table_name, column_name) pairs.
    /// Index in this vec = QED column index.
    columns: Vec<(Option<String>, String)>,
}

impl ColumnScope {
    /// Build scope from a single table scan.
    fn from_table(table: &str, schema: &RichSchema) -> Self { ... }

    /// Build scope from a JOIN (merge left + right scopes).
    fn join(left: Self, right: Self) -> Self { ... }

    /// Resolve a column reference to a 0-based QED index.
    fn resolve(&self, table_alias: Option<&str>, col_name: &str) -> Option<usize> { ... }
}
```

### Key Translation Rules

| ogsql-parser AST | QED Relation | Notes |
|-----------------|--------------|-------|
| `FROM t` | `Scan { table: "t", fields: [] }` | empty fields = all columns |
| `FROM t1 JOIN t2 ON ...` | `Join { left: Scan(t1), right: Scan(t2), condition }` | condition translated recursively |
| `WHERE a = 1` | `Filter { condition: BinOp(Eq, ColumnRef(0), Literal(Int(1))), input }` | column index resolved from scope |
| `SELECT a, b` | `Project { exprs: [ColumnRef(0), ColumnRef(1)], input }` | maps SELECT targets to Project |
| `GROUP BY a` | `Aggregate { keys: [0], aggs: [...], input }` | keys are column indices |
| `DISTINCT` | `Distinct { input }` | wraps inner relation |
| `LIMIT n` | `QOp { name: "Limit", args: [Literal(Int(n))], input }` | uninterpreted |
| `ORDER BY a DESC` | `QOp { name: "Sort", args: [...], input }` | uninterpreted (order doesn't affect bag semantics) |
| `NVL(a, b)` | `QOp { name: "NVL", args: [a, b], input }` | GaussDB function = uninterpreted |
| `DECODE(...)` | `QOp { name: "DECODE", args: [...], input }` | GaussDB function = uninterpreted |

### Translator Skeleton

```rust
/// Translates an ogsql-parser `Statement` into a QED `QedRelation`.
///
/// Requires a `RichSchema` for column name → index resolution.
/// Returns `Err` for SQL constructs that cannot be represented in QED.
pub struct AstTranslator<'a> {
    schema: &'a RichSchema,
}

impl<'a> AstTranslator<'a> {
    pub fn new(schema: &'a RichSchema) -> Self { Self { schema } }

    /// Translate a full SELECT statement into a QED Relation tree.
    pub fn translate_select(&self, select: &SelectStatement) -> Result<QedRelation, TranslateError> {
        // 1. Build FROM clause → base relation + column scope
        let (base_rel, scope) = self.translate_from(&select.from)?;

        // 2. Apply WHERE → Filter
        let filtered = match &select.where_clause {
            Some(expr) => QedRelation::Filter {
                condition: self.translate_expr(expr, &scope)?,
                input: Box::new(base_rel),
            },
            None => base_rel,
        };

        // 3. Apply GROUP BY → Aggregate
        let aggregated = if !select.group_by.is_empty() {
            let keys = self.translate_group_keys(&select.group_by, &scope)?;
            let aggs = self.extract_aggregates(&select.targets, &scope)?;
            let agg_rel = QedRelation::Aggregate {
                keys,
                aggs,
                input: Box::new(filtered),
            };
            // Apply HAVING as a Filter on top of Aggregate
            match &select.having {
                Some(having) => QedRelation::Filter {
                    condition: self.translate_expr(having, &scope)?,
                    input: Box::new(agg_rel),
                },
                None => agg_rel,
            }
        } else {
            filtered
        };

        // 4. Apply SELECT targets → Project
        let projected = self.translate_projection(&select.targets, &aggregated, &scope)?;

        // 5. Apply DISTINCT
        let distincted = if select.distinct {
            QedRelation::Distinct { input: Box::new(projected) }
        } else {
            projected
        };

        // 6. Wrap ORDER BY → QOp (doesn't affect bag semantics, but QED needs it)
        let ordered = match &select.order_by {
            Some(order) if !order.is_empty() => QedRelation::QOp {
                name: "Sort".to_string(),
                args: self.translate_order_by(order, &scope)?,
                input: Box::new(distincted),
            },
            _ => distincted,
        };

        // 7. Wrap LIMIT → QOp
        let limited = match &select.limit {
            Some(limit_expr) => QedRelation::QOp {
                name: "Limit".to_string(),
                args: vec![self.translate_expr(limit_expr, &scope)?],
                input: Box::new(ordered),
            },
            None => ordered,
        };

        Ok(limited)
    }

    /// Translate FROM clause → (base QedRelation, ColumnScope).
    fn translate_from(&self, from: &[TableRef]) -> Result<(QedRelation, ColumnScope), TranslateError> {
        // Handle: Table, Subquery (derived), JOIN chains
        // For JOINs, recursively translate left and right, merge scopes
        ...
    }

    /// Translate an ogsql-parser Expr → QedExpr.
    fn translate_expr(&self, expr: &Expr, scope: &ColumnScope) -> Result<QedExpr, TranslateError> {
        match expr {
            Expr::ColumnRef(name) => {
                let index = scope.resolve_column(name)?;
                Ok(QedExpr::ColumnRef { index })
            }
            Expr::Literal(lit) => self.translate_literal(lit),
            Expr::BinaryOp { left, op, right } => { ... }
            Expr::UnaryOp { expr, op } => { ... }
            Expr::FunctionCall { name, args, .. } => { ... }
            Expr::Subquery(sub) => { ... }
            Expr::Exists(sub) => { ... }
            Expr::InList { expr, list, negated } => { ... }
            Expr::Between { expr, low, high, negated } => { ... }
            Expr::IsNull(expr) => { ... }
            Expr::IsNotNull(expr) => { ... }
            Expr::Placeholder(_) => {
                // Placeholder (? or :name) → treat as uninterpreted variable
                Ok(QedExpr::Function {
                    name: "Param".to_string(),
                    args: vec![],
                })
            }
            _ => Err(TranslateError::UnsupportedExpr(format!("{expr:?}"))),
        }
    }
}
```

### Known Limitations (Phase A)

| SQL Construct | Phase A Handling | Future |
|--------------|-----------------|--------|
| UNION / INTERSECT / EXCEPT | Full support | — |
| Correlated subqueries | `Correlate` node | Phase B |
| CTEs (WITH) | Inline into main query first | Phase B |
| Window functions | `QOp` (uninterpreted) | Phase B |
| GaussDB: DECODE, NVL, MERGE INTO | `QOp` (uninterpreted) | Phase B |
| UNION ALL vs UNION | Union node with `all` flag | Phase A |
| EXISTS / NOT EXISTS | `Filter` with `Quantified` | Phase A |

---

## Component 4: QED Prover Harness

**Status:** NEW
**File:** `crates/qed/src/prover.rs`

Wraps the `qed-prover` binary as an external process. No Rust SMT dependency needed for Phase A.

### Design

```rust
/// Result of a QED equivalence proof attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofResult {
    /// QED proved the two queries are semantically equivalent.
    Equivalent,
    /// QED found a counterexample (queries are NOT equivalent).
    NotEquivalent { counterexample: Option<String> },
    /// QED could not determine equivalence within the timeout.
    Unknown { reason: String },
    /// The prover process timed out.
    Timeout { seconds: u64 },
}

/// Configuration for the QED prover invocation.
#[derive(Debug, Clone)]
pub struct ProverConfig {
    /// Path to the `qed-prover` binary.
    pub binary_path: PathBuf,
    /// Timeout in seconds for the prover process.
    pub timeout_secs: u64,
    /// Working directory for the prover (optional).
    pub workdir: Option<PathBuf>,
}

/// Run the QED prover on a pair of queries.
pub fn run_prover(
    input: &QedInput,
    config: &ProverConfig,
) -> Result<ProofResult, ProverError> {
    // 1. Serialize QedInput to JSON
    let json = serde_json::to_string_pretty(input)
        .map_err(|e| ProverError::Serialization(e.to_string()))?;

    // 2. Write to temp file
    let temp_dir = tempfile::tempdir()
        .map_err(|e| ProverError::Io(e.to_string()))?;
    let input_path = temp_dir.path().join("input.json");
    std::fs::write(&input_path, &json)
        .map_err(|e| ProverError::Io(e.to_string()))?;

    // 3. Spawn qed-prover process
    let output = Command::new(&config.binary_path)
        .arg(&input_path)
        .current_dir(config.workdir.as_deref().unwrap_or(temp_dir.path()))
        .output()
        .map_err(|e| ProverError::Process(e.to_string()))?;

    // 4. Parse prover output (stdout)
    // qed-prover outputs: "Equivalent" | "NotEquivalent" | "Unknown"
    parse_prover_output(&output)
}

fn parse_prover_output(output: &std::process::Output) -> Result<ProofResult, ProverError> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if stdout.contains("Equivalent") {
        Ok(ProofResult::Equivalent)
    } else if stdout.contains("NotEquivalent") {
        Ok(ProofResult::NotEquivalent {
            counterexample: extract_counterexample(&stdout),
        })
    } else if stdout.contains("Unknown") {
        Ok(ProofResult::Unknown {
            reason: extract_reason(&stdout, &stderr),
        })
    } else {
        Err(ProverError::UnexpectedOutput {
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        })
    }
}
```

### Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum ProverError {
    #[error("serialization failed: {0}")]
    Serialization(String),
    #[error("IO error: {0}")]
    Io(String),
    #[error("prover process error: {0}")]
    Process(String),
    #[error("prover timed out after {0}s")]
    Timeout(u64),
    #[error("unexpected prover output: stdout={stdout}, stderr={stderr}")]
    UnexpectedOutput { stdout: String, stderr: String },
}
```

---

## Component 5: End-to-End Verification Pipeline

**Status:** NEW
**File:** `crates/qed/src/verify.rs`

High-level API that ties everything together.

```rust
/// Verification result for a single rewrite rule test case.
#[derive(Debug)]
pub struct VerificationResult {
    /// The rule that produced the rewrite.
    pub rule_id: String,
    /// The original SQL (formatted).
    pub original_sql: String,
    /// The rewritten SQL (formatted).
    pub rewritten_sql: String,
    /// The QED proof result.
    pub proof: ProofResult,
    /// Time taken for the proof attempt (milliseconds).
    pub elapsed_ms: u64,
}

/// Verify that a rewrite preserves semantic equivalence using QED.
///
/// Takes the original and rewritten statements, a rich schema, and prover
/// configuration. Translates both statements to QED IR, invokes the prover,
/// and returns the proof result.
pub fn verify_rewrite(
    rule_id: &str,
    original: &Statement,
    rewritten: &Statement,
    schema: &RichSchema,
    prover_config: &ProverConfig,
) -> Result<VerificationResult, VerifyError> {
    let translator = AstTranslator::new(schema);

    let start = std::time::Instant::now();

    // Translate both statements to QED Relations
    let query1 = translate_statement(&translator, original)?;
    let query2 = translate_statement(&translator, rewritten)?;

    // Build QED input
    let qed_schemas = build_qed_schemas(schema);
    let input = QedInput {
        schemas: qed_schemas,
        queries: [query1, query2],
        help: format!("Verify semantic equivalence for rule '{rule_id}'"),
    };

    // Run prover
    let proof = run_prover(&input, prover_config)?;
    let elapsed = start.elapsed().as_millis() as u64;

    Ok(VerificationResult {
        rule_id: rule_id.to_string(),
        original_sql: SqlFormatter::new().format_statement(original),
        rewritten_sql: SqlFormatter::new().format_statement(rewritten),
        proof,
        elapsed_ms: elapsed,
    })
}

/// Verify all test cases for a rewrite rule against QED.
pub fn verify_rule_tests(
    rule: &dyn RewriteRule,
    test_cases: &[RuleTestCase],
    schema: &RichSchema,
    prover_config: &ProverConfig,
) -> Vec<Result<VerificationResult, VerifyError>> {
    test_cases.iter().map(|tc| {
        let ctx = build_test_context(schema);
        let original = parse_single_statement(&tc.input_sql);
        if rule.matches(&ctx, &original) {
            if let Some(RewriteAction::Replace(rewritten)) = rule.apply(&ctx, &original) {
                return verify_rewrite(rule.id(), &original, &rewritten, schema, prover_config);
            }
        }
        Err(VerifyError::RuleNotApplicable)
    }).collect()
}

/// A single test case for rule verification.
pub struct RuleTestCase {
    pub input_sql: String,
    pub schema_ddl: Option<String>,
}
```

---

## Component 6: Crate Structure

**Status:** NEW
**File:** `crates/qed/`

### Cargo.toml

```toml
[package]
name = "metamorphosis-qed"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
metamorphosis-core = { path = "../core" }
ogsql-parser = { path = "../../ogsql-parser" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tracing = "0.1"

[dev-dependencies]
tempfile = "3"
metamorphosis-rules = { path = "../rules" }
```

### lib.rs (≤200 lines)

```rust
//! QED-based offline verification for Metamorphosis rewrite rules.
//!
//! Provides tools to verify that SQL rewrites produced by Metamorphosis
//! rules are semantically equivalent to the original queries, using the
//! QED prover (https://github.com/qed-solver/prover).
//!
//! # Architecture
//!
//! 1. [`schema`] — Extract rich schema (PK, FK, NOT NULL, CHECK) from DDL
//! 2. [`ir`] — QED intermediate representation types (Rust → JSON)
//! 3. [`translator`] — ogsql-parser AST → QED Relation translator
//! 4. [`prover`] — QED prover binary harness
//! 5. [`verify`] — End-to-end verification pipeline
//!
//! # Example
//!
//! ```ignore
//! use metamorphosis_qed::verify::verify_rewrite;
//! use metamorphosis_qed::schema::extract_rich_schema;
//! use metamorphosis_qed::prover::ProverConfig;
//!
//! let schema = extract_rich_schema(&ddl_statements);
//! let config = ProverConfig::default();
//! let result = verify_rewrite("my-rule", &original, &rewritten, &schema, &config)?;
//! assert!(matches!(result.proof, ProofResult::Equivalent));
//! ```

pub mod ir;
pub mod prover;
pub mod schema;
pub mod translator;
pub mod verify;

pub use ir::{QedInput, QedRelation, QedSchema};
pub use prover::{ProofResult, ProverConfig, ProverError};
pub use schema::{RichSchema, TableInfo, TableConstraints, ForeignKeyInfo};
pub use translator::{AstTranslator, TranslateError};
pub use verify::{VerificationResult, VerifyError};
```

---

## Task Breakdown (Bite-Sized)

### Phase A.1: Foundation (crate setup + RichSchema)

| # | Task | Files | Est. Lines | Depends On |
|---|------|-------|-----------|------------|
| A.1.1 | Create `crates/qed/` crate with `Cargo.toml` + `lib.rs` | `crates/qed/Cargo.toml`, `crates/qed/src/lib.rs` | 40 | — |
| A.1.2 | Implement `RichSchema` types | `crates/qed/src/schema.rs` | ~150 | A.1.1 |
| A.1.3 | Implement `ConstraintExtractor` (walk ogsql-parser DDL AST) | `crates/qed/src/schema.rs` | ~200 | A.1.2 |
| A.1.4 | Unit tests for ConstraintExtractor | `crates/qed/src/schema.rs` (inline tests) | ~100 | A.1.3 |
| A.1.5 | Verify: `cargo test -p metamorphosis-qed` passes | — | — | A.1.4 |

### Phase A.2: QED IR + Translator

| # | Task | Files | Est. Lines | Depends On |
|---|------|-------|-----------|------------|
| A.2.1 | Implement QED IR types (`QedInput`, `QedRelation`, `QedExpr`, etc.) | `crates/qed/src/ir.rs` | ~250 | A.1.1 |
| A.2.2 | Implement `ColumnScope` (name → index resolution) | `crates/qed/src/translator.rs` | ~80 | A.2.1 |
| A.2.3 | Implement FROM clause translation (`TableRef` → `Scan`/`Join`) | `crates/qed/src/translator.rs` | ~120 | A.2.2 |
| A.2.4 | Implement WHERE → Filter translation | `crates/qed/src/translator.rs` | ~80 | A.2.3 |
| A.2.5 | Implement SELECT → Project translation | `crates/qed/src/translator.rs` | ~80 | A.2.3 |
| A.2.6 | Implement GROUP BY → Aggregate translation | `crates/qed/src/translator.rs` | ~100 | A.2.3 |
| A.2.7 | Implement expression translation (`Expr` → `QedExpr`) | `crates/qed/src/translator.rs` | ~200 | A.2.2 |
| A.2.8 | Implement DISTINCT → Distinct, ORDER BY → QOp, LIMIT → QOp | `crates/qed/src/translator.rs` | ~60 | A.2.3 |
| A.2.9 | Implement UNION/INTERSECT/EXCEPT | `crates/qed/src/translator.rs` | ~60 | A.2.3 |
| A.2.10 | Unit tests: round-trip simple SQL → QedRelation → JSON | `crates/qed/src/translator.rs` | ~150 | A.2.3-9 |

### Phase A.3: Prover Harness

| # | Task | Files | Est. Lines | Depends On |
|---|------|-------|-----------|------------|
| A.3.1 | Implement `ProverConfig` + `ProofResult` types | `crates/qed/src/prover.rs` | ~80 | A.1.1 |
| A.3.2 | Implement `run_prover()` (temp file + process spawn) | `crates/qed/src/prover.rs` | ~120 | A.3.1 |
| A.3.3 | Implement output parsing (stdout → `ProofResult`) | `crates/qed/src/prover.rs` | ~80 | A.3.2 |
| A.3.4 | Integration test: invoke qed-prover on a known equivalent pair | `crates/qed/tests/prover_test.rs` | ~80 | A.3.3, QED binary available |

### Phase A.4: End-to-End Pipeline

| # | Task | Files | Est. Lines | Depends On |
|---|------|-------|-----------|------------|
| A.4.1 | Implement `verify_rewrite()` pipeline function | `crates/qed/src/verify.rs` | ~100 | A.2, A.3 |
| A.4.2 | Implement `build_qed_schemas()` (RichSchema → Vec<QedSchema>) | `crates/qed/src/verify.rs` | ~60 | A.4.1 |
| A.4.3 | Implement `verify_rule_tests()` batch verification | `crates/qed/src/verify.rs` | ~80 | A.4.1 |
| A.4.4 | E2E test: verify `EliminateSelectStar` equivalence | `crates/qed/tests/e2e_test.rs` | ~80 | A.4.3 |

### Phase A.5: CI Integration

| # | Task | Files | Est. Lines | Depends On |
|---|------|-------|-----------|------------|
| A.5.1 | Create CI script to install `qed-prover` binary | `scripts/install-qed-prover.sh` | ~30 | — |
| A.5.2 | Create verification test runner script | `scripts/run-qed-verify.sh` | ~20 | A.5.1 |
| A.5.3 | Add CI job (GitHub Actions or equivalent) | `.github/workflows/qed-verify.yml` | ~40 | A.5.2 |
| A.5.4 | Document QED verification setup in CONTRIBUTING.md | `docs/CONTRIBUTING.md` (update) | ~30 | A.5.3 |

---

## File Manifest

| File | Action | Est. Lines | Component |
|------|--------|-----------|-----------|
| `crates/qed/Cargo.toml` | Create | 20 | Setup |
| `crates/qed/src/lib.rs` | Create | 50 | Setup |
| `crates/qed/src/schema.rs` | Create | 350 | RichSchema + ConstraintExtractor |
| `crates/qed/src/ir.rs` | Create | 250 | QED IR types |
| `crates/qed/src/translator.rs` | Create | 500 | AST → QED translator |
| `crates/qed/src/prover.rs` | Create | 280 | Prover harness |
| `crates/qed/src/verify.rs` | Create | 200 | E2E pipeline |
| `crates/qed/tests/prover_test.rs` | Create | 80 | Integration tests |
| `crates/qed/tests/e2e_test.rs` | Create | 120 | E2E tests |
| `scripts/install-qed-prover.sh` | Create | 30 | CI |
| `scripts/run-qed-verify.sh` | Create | 20 | CI |
| `.github/workflows/qed-verify.yml` | Create | 40 | CI |

**Total new code: ~1,940 lines** (spread across 12 files, all ≤600 lines)

---

## Dependency Graph

```
A.1.1 ──► A.1.2 ──► A.1.3 ──► A.1.4 ──► A.1.5
  │
  ├──► A.2.1 ──► A.2.2 ──► A.2.3 ─┬──► A.2.4
  │                                ├──► A.2.5
  │                                ├──► A.2.6
  │                                ├──► A.2.7
  │                                ├──► A.2.8
  │                                ├──► A.2.9
  │                                └──► A.2.10
  │
  └──► A.3.1 ──► A.3.2 ──► A.3.3 ──► A.3.4

(A.2 + A.3) ──► A.4.1 ──► A.4.2 ──► A.4.3 ──► A.4.4

A.4 ──► A.5.1 ──► A.5.2 ──► A.5.3 ──► A.5.4
```

**Parallelizable tracks:** A.2 and A.3 can proceed in parallel (translator and prover harness are independent).

---

## Testing Strategy

### Unit Tests (50%)

Each component tested in isolation:

1. **ConstraintExtractor**: Feed DDL statements → assert extracted PK/NOT NULL/CHECK/FK
2. **ColumnScope**: Test name → index resolution with aliases, joins, ambiguity
3. **Translator**: Feed simple SQL → assert QedRelation tree shape
4. **IR serialization**: QedInput → JSON → assert matches expected format

### Integration Tests (30%)

5. **Prover harness**: Known equivalent pairs (e.g., `SELECT * FROM t` ≡ `SELECT a, b FROM t`)
6. **Known non-equivalent pairs**: Verify prover catches them

### E2E Tests (20%)

7. **EliminateSelectStar**: `SELECT * FROM users` → `SELECT id, name FROM users` → prove equivalent
8. **Future rules**: Each new Safe rule must include QED verification test case

---

## QED Prover Known Limitations (Phase A Workarounds)

| Limitation | Impact | Phase A Workaround |
|-----------|--------|-------------------|
| FK constraints not yet implemented in prover | Cannot prove equivalence for FK-dependent rewrites | Skip FK-dependent test cases; document as Phase B |
| GaussDB-specific functions | QED doesn't know DECODE/NVL semantics | Encode as `QOp` (uninterpreted) — prover will return Unknown |
| Correlated subqueries | Complex Correlate node | Simplify test cases to avoid correlation; document |
| CTEs (WITH clause) | No direct CTE support | Inline CTEs before translation (translator pre-pass) |

---

## Phase B Preview (Not in Scope)

After Phase A is complete:

1. **Embedded SMT**: Replace `qed-prover` binary with direct Z3/CVC5 Rust binding
2. **Runtime integration**: Extend `validate_statement()` in `engine.rs` to optionally call QED
3. **FK support**: Implement FK constraint encoding when QED prover adds support
4. **Confidence::Proven(Theorem)**: Add variant to `Confidence` enum for QED-proven rewrites
5. **Incremental verification**: Cache proof results, only re-verify changed rules

---

## Entry Point: What to Build First

**Start with Task A.1.1** (crate setup), then proceed to A.1.2-A.1.3 (RichSchema). This is the foundation everything else depends on, and can be validated independently by extracting constraints from real DDL files.
