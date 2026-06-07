
# meteamorphosis-verieql Standalone Port Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Port VeriEQL's bounded equivalence verification for SQL queries from Python to Rust as a standalone crate `metamorphosis-verieql` with zero dependencies on other metamorphosis crates.

**Architecture:** The crate takes two SQL strings + schema + constraints as input. Uses `ogsql-parser` for parsing (replacing `mo-sql-parsing`), builds an internal IR, encodes IR as Z3 SMT constraints via the `z3` Rust crate, and proves/disproves equivalence within a configurable tuple bound. Returns `ProofResult` with optional counterexample.

**Tech Stack:** `ogsql-parser` (git), `z3` v0.20 (vendored), `serde`/`serde_json`, `thiserror`, `tracing`

---

## Crate Structure

```
crates/verieql/
├── Cargo.toml                 # Only: ogsql-parser, z3, serde, thiserror, tracing
├── src/
│   ├── lib.rs                 # Public API: VeriEql::verify(), ProofReport
│   ├── types.rs               # Core types: Bound, ProofResult, ProofReport, ColumnType, TableSchema
│   ├── ir.rs                  # Internal IR: Relation, Expr, Aggregate, ExprSort (the Formula tree)
│   ├── environment.rs         # Symbolic database: tuples, attributes, Z3 context manager
│   ├── translator.rs          # ogsql-parser AST → VeriEql IR (replaces encoder.py + mo-sql-parsing)
│   ├── encoder.rs             # VeriEql IR → Z3 constraints (replaces visitor.py core)
│   ├── verifier.rs            # Bag/List semantics equivalence checker (replaces verifiers/)
│   ├── constraints.rs         # Integrity constraint modeling: PK, FK, NOT NULL, range
│   └── counterexample.rs      # Z3 model → human-readable counterexample
└── tests/
    ├── integration_test.rs    # End-to-end: verify() on known-equivalent/inequivalent pairs
    └── fixtures/              # JSON test fixtures (schemas, constraints)
        ├── simple_eq.json
        └── simple_neq.json
```

**Dependency rule:** `Cargo.toml` references ONLY `ogsql-parser`, `z3`, `serde`, `serde_json`, `thiserror`, `tracing`. No `metamorphosis-core`, no `metamorphosis-rules`, no `metamorphosis-qed`.

---

## Task 1: Scaffold the Crate

**Files:**
- Create: `crates/verieql/Cargo.toml`
- Create: `crates/verieql/src/lib.rs`
- Create: `crates/verieql/src/types.rs`
- Modify: `Cargo.toml` (workspace root — add member)

**Step 1: Create Cargo.toml**

```toml
[package]
name = "metamorphosis-verieql"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
ogsql-parser = { git = "https://github.com/c2j/ogsql-parser" }
z3 = { version = "0.20", features = ["vendored"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tracing = "0.1"
```

**Step 2: Add to workspace members**

In root `Cargo.toml`, change:
```toml
members = ["crates/core", "crates/rules", "crates/cli", "crates/qed"]
```
to:
```toml
members = ["crates/core", "crates/rules", "crates/cli", "crates/qed", "crates/verieql"]
```

**Step 3: Create lib.rs skeleton**

```rust
//! VeriEQL: Bounded equivalence verification for SQL queries.
//!
//! Port of the VeriEQL OOPSLA 2024 distinguished paper algorithm.
//! Uses ogsql-parser for SQL parsing and Z3 for SMT-based bounded model checking.

pub mod types;
pub mod ir;
pub mod environment;
pub mod translator;
pub mod encoder;
pub mod verifier;
pub mod constraints;
pub mod counterexample;

use types::{Bound, ProofResult, ProofReport, TableSchema};
use constraints::IntegrityConstraint;

/// Main entry point for bounded SQL equivalence verification.
pub struct VeriEql { /* fields */ }
```

**Step 4: Verify compilation**

Run: `cargo build -p metamorphosis-verieql`
Expected: Compiles (with Z3 vendored build, ~5 min first time).

**Step 5: Commit**

```
git add crates/verieql/ Cargo.toml
git commit -m "feat(verieql): scaffold standalone crate structure"
```

---

## Task 2: Core Types (types.rs)

**Files:**
- Write: `crates/verieql/src/types.rs`
- Modify: `crates/verieql/src/lib.rs` (re-export)

**What this does:** Define all public-facing types — the input schema, bound configuration, proof results, and error types. Mirrors VeriEQL's `constants.py` `STATE` enum + `context.py`.

```rust
use serde::{Deserialize, Serialize};

/// Tuple bound for bounded model checking.
/// A bound of N means each table has N symbolic tuples.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bound(pub usize);

impl Default for Bound {
    fn default() -> Self { Bound(2) }
}

/// Column type for schema definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ColumnType {
    Integer,
    Varchar,
    Boolean,
    Date,
    Float,
}

/// Schema for a single table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<ColumnDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnDef {
    pub name: String,
    pub col_type: ColumnType,
}

/// Result of bounded equivalence check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProofResult {
    /// Queries are equivalent up to the given bound.
    Equivalent,
    /// Queries are NOT equivalent; counterexample provided.
    NotEquivalent { counterexample: Counterexample },
    /// Z3 returned Unknown (insufficient bound or complexity).
    Unknown { reason: String },
}

/// Human-readable counterexample database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Counterexample {
    pub tables: Vec<CounterexampleTable>,
    pub sql1_result: Vec<Vec<String>>,
    pub sql2_result: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CounterexampleTable {
    pub name: String,
    pub rows: Vec<Vec<String>>,
}

/// Full verification report with timing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofReport {
    pub result: ProofResult,
    pub translate_ms: u64,
    pub solve_ms: u64,
    pub bound: Bound,
}

/// Verification semantics.
#[derive(Debug, Clone, PartialEq)]
pub enum Semantics {
    /// Bag semantics (default): duplicates matter, multiple identical rows are distinct.
    Bag,
    /// List semantics: ORDER BY position matters, rows compared positionally.
    List,
}
```

**Step 1: Add `pub use types::*;` to lib.rs**

**Step 2: Verify compilation**

Run: `cargo build -p metamorphosis-verieql`

**Step 3: Commit**

```
git add crates/verieql/src/types.rs crates/verieql/src/lib.rs
git commit -m "feat(verieql): define core types — Bound, ProofResult, TableSchema"
```

---

## Task 3: Internal IR Types (ir.rs)

**Files:**
- Write: `crates/verieql/src/ir.rs`
- Modify: `crates/verieql/src/lib.rs`

**What this does:** Define the VeriEql Intermediate Representation — the Formula tree that sits between ogsql-parser AST and Z3 constraints. This is the Rust equivalent of VeriEQL's `formulas/` module (~60 Python classes). Mirrors `QedRelation`/`QedExpr` from `metamorphosis-qed` but WITHOUT depending on it.

```rust
//! VeriEql Intermediate Representation.
//!
//! Represents SQL queries as a relational algebra tree.
//! Each node corresponds to a VeriEQL Formula class from the Python codebase:
//!   FBaseTable → Relation::BaseTable
//!   FFilterTable → Relation::Filter
//!   FProjectionTable → Relation::Project
//!   FJoinTable → Relation::Join
//!   FGroupByTable → Relation::GroupBy
//!   FOrderByTable → Relation::OrderBy
//!   FUnionTable → Relation::Union, etc.

/// A relational algebra expression (a "table" in VeriEQL terminology).
#[derive(Debug, Clone)]
pub enum Relation {
    /// Scan a named base table with specific columns.
    BaseTable {
        name: String,
        columns: Vec<String>,
        tuple_count: usize,       // = Bound
    },
    /// WHERE clause: filter tuples by a predicate.
    Filter {
        input: Box<Relation>,
        predicate: Expr,          // bool-typed expression
    },
    /// SELECT clause: project columns, optionally with aggregation.
    Project {
        input: Box<Relation>,
        exprs: Vec<ProjectExpr>,  // column references or aggregate calls
        distinct: bool,
    },
    /// JOIN (inner, left, right, full, cross, natural).
    Join {
        left: Box<Relation>,
        right: Box<Relation>,
        join_type: JoinType,
        condition: Option<Expr>,  // ON clause
        natural: bool,
        using_columns: Vec<String>,
    },
    /// GROUP BY with optional HAVING.
    GroupBy {
        input: Box<Relation>,
        keys: Vec<Expr>,
        aggregates: Vec<AggregateExpr>,
        having: Option<Expr>,
    },
    /// ORDER BY clause.
    OrderBy {
        input: Box<Relation>,
        items: Vec<OrderByItem>,
        limit: Option<Expr>,
        offset: Option<Expr>,
    },
    /// UNION / UNION ALL.
    Union {
        left: Box<Relation>,
        right: Box<Relation>,
        all: bool,                // UNION ALL if true
    },
    /// INTERSECT / INTERSECT ALL.
    Intersect {
        left: Box<Relation>,
        right: Box<Relation>,
        all: bool,
    },
    /// EXCEPT / EXCEPT ALL.
    Except {
        left: Box<Relation>,
        right: Box<Relation>,
        all: bool,
    },
    /// DISTINCT on existing relation.
    Distinct {
        input: Box<Relation>,
    },
    /// VALUES clause: literal rows.
    Values {
        rows: Vec<Vec<Expr>>,
    },
    /// Empty table (e.g., WHERE FALSE).
    Empty,
}

/// A projected expression in SELECT: either a column reference or an aggregate.
#[derive(Debug, Clone)]
pub enum ProjectExpr {
    Column(Expr),
    Aggregate(AggregateExpr),
}

/// An aggregate function call.
#[derive(Debug, Clone)]
pub struct AggregateExpr {
    pub func: AggFunc,
    pub arg: Option<Expr>,       // None for COUNT(*)
    pub distinct: bool,
    pub alias: Option<String>,
}

#[derive(Debug, Clone)]
pub enum AggFunc {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

/// An expression (scalar value).
#[derive(Debug, Clone)]
pub enum Expr {
    /// Column reference: table.column or just column.
    ColumnRef {
        table: Option<String>,
        column: String,
    },
    /// Literal value.
    Literal(ExprValue),
    /// Binary operation: a + b, a = b, a AND b, etc.
    BinaryOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// Unary operation: NOT x, -x.
    UnaryOp {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    /// CASE WHEN condition THEN result ... ELSE default END.
    Case {
        operand: Option<Box<Expr>>,
        whens: Vec<(Expr, Expr)>,      // (condition, result)
        else_expr: Option<Box<Expr>>,
    },
    /// IS NULL / IS NOT NULL.
    IsNull {
        expr: Box<Expr>,
        negated: bool,
    },
    /// IN (value1, value2, ...)
    InList {
        expr: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    /// IN (subquery)
    InSubquery {
        expr: Box<Expr>,
        subquery: Box<Relation>,
        negated: bool,
    },
    /// EXISTS (subquery)
    Exists(Box<Relation>),
    /// EXISTS (subquery)
    NotExists(Box<Relation>),
    /// Scalar subquery: (SELECT ...)
    ScalarSubquery(Box<Relation>),
    /// BETWEEN low AND high.
    Between {
        expr: Box<Expr>,
        low: Box<Expr>,
        high: Box<Expr>,
        negated: bool,
    },
    /// LIKE pattern.
    Like {
        expr: Box<Expr>,
        pattern: Box<Expr>,
        negated: bool,
    },
    /// Function call: COUNT(*), COALESCE(a,b), etc.
    FunctionCall {
        name: String,
        args: Vec<Expr>,
    },
    /// SQL NULL literal.
    SqlNull,
    /// Star: SELECT *
    Star,
}

#[derive(Debug, Clone)]
pub enum ExprValue {
    Integer(i64),
    Float(f64),
    String(String),
    Boolean(bool),
}

#[derive(Debug, Clone)]
pub enum BinOp {
    // Arithmetic
    Add, Sub, Mul, Div, Mod,
    // Comparison
    Eq, Neq, Lt, Gt, Lte, Gte,
    // Logical
    And, Or,
    // String
    Concat,
}

#[derive(Debug, Clone)]
pub enum UnaryOp {
    Not,
    Neg,
    IsTrue,
    IsFalse,
    IsUnknown,
}

#[derive(Debug, Clone)]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
    Natural,
}

#[derive(Debug, Clone)]
pub struct OrderByItem {
    pub expr: Expr,
    pub asc: bool,
    pub nulls_first: Option<bool>,
}
```

**Step 1: Register module**

Add `pub mod ir;` to `crates/verieql/src/lib.rs`.

**Step 2: Verify compilation**

Run: `cargo build -p metamorphosis-verieql`

**Step 3: Commit**

```
git add crates/verieql/src/ir.rs crates/verieql/src/lib.rs
git commit -m "feat(verieql): define IR types — Relation, Expr, aggregate model"
```

---

## Task 4: Symbolic Database Environment (environment.rs)

**Files:**
- Write: `crates/verieql/src/environment.rs`
- Modify: `crates/verieql/src/lib.rs`

**What this does:** Build the symbolic database — the core of VeriEQL's bounded model checking. For each table with N columns and bound size B, creates B symbolic tuples. Each tuple is a Z3 constant of uninterpreted sort. Each column becomes a Z3 uninterpreted function `(TupleSort) → IntSort`. Mirrors VeriEQL's `environment.py` `create_database()`, `declare_attribute()`, `_declare_function()`.

**Architecture:**
```
Table "EMP" (id INT, name VARCHAR), Bound=2

TupleSort = Z3 Uninterpreted Sort
t1 = Const("t1", TupleSort)
t2 = Const("t2", TupleSort)

EMP_id = Function("EMP.id", TupleSort, IntSort)
EMP_name = Function("EMP.name", TupleSort, IntSort)
NULL = Function("NULL", TupleSort, StringSort, BoolSort)
DELETED = Function("DELETED", TupleSort, BoolSort)

DBMS facts:
  Not(DELETED(t1))
  Not(DELETED(t2))
  EMP_id(t1) == x1        (symbolic variable)
  EMP_name(t1) == x2
  EMP_id(t2) == x3
  EMP_name(t2) == x4
  Not(NULL(t1, "EMP.id"))   (NOT NULL constraint)
  INT_LOWER <= EMP_id(t1) <= INT_UPPER  (range constraint)
```

```rust
use std::collections::HashMap;
use z3::{
    ast::{Ast, Bool, Int},
    FuncDecl, Sort, Context, Solver,
};
use crate::types::{TableSchema, ColumnDef, ColumnType, Bound, Semantics};
use crate::ir::Relation;

/// Symbolic database environment.
pub struct Environment<'ctx> {
    /// Z3 context — one per verification session.
    pub ctx: &'ctx Context,
    /// Uninterpreted sort for tuples.
    pub tuple_sort: Sort<'ctx>,
    /// Uninterpreted sort for string labels (column names, aggregate IDs).
    pub string_sort: Sort<'ctx>,
    /// Bool sort shorthand.
    pub bool_sort: Sort<'ctx>,
    /// Int sort shorthand.
    pub int_sort: Sort<'ctx>,
    /// DELETED(tuple) → Bool: marks a tuple as inactive (used for outer joins, empty tables).
    pub deleted_func: FuncDecl<'ctx>,
    /// NULL(tuple, col_label) → Bool: whether column's value in tuple is NULL.
    pub null_func: FuncDecl<'ctx>,
    /// COUNT / SUM / AVG / MIN / MAX are declared lazily per aggregate expression.
    pub agg_funcs: HashMap<String, FuncDecl<'ctx>>,
    /// Per-column attribute functions: attr_func(tuple) → Int.
    pub attr_funcs: HashMap<String, FuncDecl<'ctx>>,
    pub name: Vec<String>,
    /// Active tuples in the database.
    pub tuples: Vec<Int<'ctx>>,
    /// Solver for constraint accumulation.
    pub solver: Solver<'ctx>,
    /// DBMS facts: accumulated constraints (PK, FK, NOT NULL, row values, type bounds).
    pub dbms_facts: Vec<Bool<'ctx>>,
    /// Number of symbolic tuples per table (= Bound).
    pub bound_size: usize,
    /// Verification semantics.
    pub semantics: Semantics,
}

impl<'ctx> Environment<'ctx> {
    pub fn new(ctx: &'ctx Context, bound: Bound, semantics: Semantics) -> Self { todo!() }
    /// Create uninterpreted functions.
    pub fn init_functions(&mut self) { todo!() }
    /// Create a symbolic table with B tuples.
    pub fn create_database(&mut self, schema: &TableSchema, bound: usize) -> Result<(), VeriEqlError> { todo!() }
    /// Register per-column attribute function.
    pub fn declare_attribute(&mut self, table: &str, col: &str, col_type: &ColumnType) -> FuncDecl<'ctx> { todo!() }
    /// Create a symbolic tuple (Z3 const).
    pub fn declare_tuple(&mut self, name: &str) -> Int<'ctx> { todo!() }
    /// Add a constraint (PK, FK, NOT NULL, range) to DBMS facts.
    pub fn add_fact(&mut self, fact: Bool<'ctx>) { todo!() }
}
```

**Key functions to implement:**
- `Environment::new()` — create Z3 sorts (`DeclareSort`), `DELETED`/`NULL` functions
- `Environment::create_database()` — for each row in [0..bound), create symbolic tuple + attribute assignments
- `Environment::declare_attribute()` — create `Function(table.col, TupleSort, IntSort)`
- `Environment::declare_tuple()` — create `Const(tN, TupleSort)`, add `Not(DELETED(tN))` to facts

**Step 1: Write the module**

**Step 2: Verify compilation**

Run: `cargo build -p metamorphosis-verieql`

**Step 3: Commit**

```
git add crates/verieql/src/environment.rs
git commit -m "feat(verieql): implement symbolic database environment"
```

---

## Task 5: ogsql-parser AST → VeriEql IR Translator (translator.rs)

**Files:**
- Write: `crates/verieql/src/translator.rs`
- Modify: `crates/verieql/src/lib.rs`

**What this does:** Walk `ogsql-parser` AST and build `VeriEql IR`. This is the Rust equivalent of VeriEQL's `encoder.py` (SQL → Formula tree) + `parsers/sql_parser.py` (mo-sql-parsing → dict). Because ogsql-parser provides a typed AST (not dict), the translator is cleaner and more concise.

**Mapping: ogsql-parser AST → VeriEql IR**

| ogsql-parser type | VeriEql IR |
|---|---|
| `Statement::Select(s)` | `Relation::...` (returned) |
| `SelectStatement.from` | `Relation::BaseTable` / `Join` / `Subquery` |
| `SelectStatement.where_clause` | `Relation::Filter` |
| `SelectStatement.targets` | `Relation::Project` |
| `SelectStatement.group_by` | `Relation::GroupBy` |
| `SelectStatement.order_by` | `Relation::OrderBy` |
| `SelectStatement.set_operation` | `Relation::Union/Intersect/Except` |
| `TableRef::Table` | `Relation::BaseTable` |
| `TableRef::Subquery` | recurse into `SelectStatement` |
| `TableRef::Join` | `Relation::Join` |
| `TableRef::Values` | `Relation::Values` |
| `Expr::BinaryOp` | `Expr::BinaryOp` |
| `Expr::Case` | `Expr::Case` |
| `Expr::InList` | `Expr::InList` |
| `Expr::InSubquery` | `Expr::InSubquery` |
| `Expr::Exists` | `Expr::Exists` |
| `Expr::IsNull` | `Expr::IsNull` |
| `Expr::Between` | `Expr::Between` |
| `Expr::Like` | `Expr::Like` |
| `Expr::FunctionCall` | `Expr::FunctionCall` / `ProjectExpr::Aggregate` |
| `Expr::Literal(Literal::Integer)` | `Expr::Literal(ExprValue::Integer)` |
| `Expr::Literal(Literal::String)` | `Expr::Literal(ExprValue::String)` |

```rust
use ogsql_parser::ast::{Statement, SelectStatement, Expr as OExpr, TableRef, SetOperation, JoinType as OJoinType};
use crate::ir::{Relation, Expr, ProjectExpr, AggregateExpr, AggFunc, ExprValue, BinOp, UnaryOp, JoinType, OrderByItem};

pub struct Translator;

impl Translator {
    /// Translate a parsed ogsql-parser Statement into VeriEql IR.
    pub fn translate(stmt: &Statement) -> Result<Relation, TranslateError> {
        match stmt {
            Statement::Select(s) => Self::translate_select(s),
            _ => Err(TranslateError::UnsupportedStatement),
        }
    }

    fn translate_select(s: &SelectStatement) -> Result<Relation, TranslateError> {
        // 1. Build FROM clause → base relation
        let mut rel = Self::translate_from(&s.from)?;
        // 2. Apply WHERE → Filter
        if let Some(ref cond) = s.where_clause {
            rel = Relation::Filter { input: Box::new(rel), predicate: Self::translate_expr(cond)? };
        }
        // 3. Apply GROUP BY / HAVING
        if !s.group_by.is_empty() || s.having.is_some() {
            rel = Self::translate_group_by(&s.group_by, &s.targets, s.having.as_ref())?;
        }
        // 4. Apply SELECT → Project
        rel = Self::translate_targets(&s.targets, rel, s.distinct)?;
        // 5. Apply ORDER BY
        if !s.order_by.is_empty() {
            rel = Relation::OrderBy { /* ... */ };
        }
        // 6. Apply set operations (UNION/INTERSECT/EXCEPT)
        if let Some(ref set_op) = s.set_operation {
            rel = Self::translate_set_op(set_op, rel)?;
        }
        Ok(rel)
    }

    fn translate_from(from: &[TableRef]) -> Result<Relation, TranslateError> { todo!() }
    fn translate_expr(expr: &OExpr) -> Result<Expr, TranslateError> { todo!() }
    fn translate_targets(targets: &[SelectTarget], input: Relation, distinct: bool) -> Result<Relation, TranslateError> { todo!() }
    fn translate_group_by(/* ... */) -> Result<Relation, TranslateError> { todo!() }
    fn translate_set_op(op: &SetOperation, left: Relation) -> Result<Relation, TranslateError> { todo!() }
}

#[derive(Debug, thiserror::Error)]
pub enum TranslateError {
    #[error("Unsupported statement type (only SELECT is supported)")]
    UnsupportedStatement,
    #[error("Unsupported expression: {0}")]
    UnsupportedExpr(String),
    #[error("Unsupported join type")]
    UnsupportedJoin,
}
```

**Key translation rules:**

1. **FROM**: A single `TableRef::Table` → `Relation::BaseTable`. Multiple `TableRef` → cross join. `TableRef::Join` → `Relation::Join` with correct `JoinType`. `TableRef::Subquery` → recurse.

2. **SELECT targets**: `SelectTarget::Expr(e, alias)` → if `e` is an aggregate function (`FunctionCall` with name in `{count,sum,avg,min,max}`), produce `ProjectExpr::Aggregate`. Otherwise `ProjectExpr::Column(translate_expr(e))`.

3. **Aggregate detection**: Walk `targets` and `group_by` to identify which `FunctionCall` nodes are aggregate functions (in SELECT or HAVING but not in WHERE).

4. **ORDER BY**: Convert `OrderByItem { expr, asc, nulls_first }` to IR `OrderByItem`.

5. **Set operations**: `SetOperation::Union { all, right }` → `Relation::Union { left, right: translate(right), all }`.

**Step 1: Write the full translator module**

Handle all `Expr` variants from ogsql-parser to IR mapping.

**Step 2: Verify compilation**

Run: `cargo build -p metamorphosis-verieql`

**Step 3: Commit**

```
git add crates/verieql/src/translator.rs
git commit -m "feat(verieql): implement ogsql-parser AST → VeriEql IR translator"
```

---

## Task 6: Z3 Constraint Encoder (encoder.rs)

**Files:**
- Write: `crates/verieql/src/encoder.rs`
- Modify: `crates/verieql/src/lib.rs`

**What this does:** Walk the VeriEql IR tree and encode it as Z3 SMT constraints. This is the Rust equivalent of VeriEQL's `visitors/visitor.py` (2368 lines) — the core encoding logic. Each `Relation` variant produces Z3 `Bool` formulas describing which output tuples satisfy it.

**Core encoding pattern (VeriEQL approach):**

```
Input: Relation tree + Environment (with tuple SORTs, attribute functions)
Output: Z3 Bool formula = "these output tuples satisfy this relation"

For a BaseTable "EMP" (bound=2):
    t1, t2 are the symbolic tuples for this table
    output_t1, output_t2 are the output tuple variables
    
    Formula = (output_t1 == t1 AND Not(DELETED(t1)))
           OR (output_t2 == t2 AND Not(DELETED(t2)))
```

```rust
use z3::ast::{Ast, Bool, Int};
use crate::ir::{Relation, Expr, ProjectExpr, AggregateExpr, BinOp, UnaryOp, ExprValue};
use crate::environment::Environment;

/// Encode a Relation tree as a Z3 Bool formula.
///
/// Given output tuple variables `out_tuples` (one per tuple in the symbolic database),
/// the formula is satisfied iff those tuples represent the relation's content.
pub fn encode_relation(
    rel: &Relation,
    out_tuples: &[Int<'ctx>],
    env: &Environment<'ctx>,
) -> Result<Bool<'ctx>, EncodeError> {
    match rel {
        Relation::BaseTable { name, columns, tuple_count } => {
            encode_base_table(name, columns, *tuple_count, out_tuples, env)
        }
        Relation::Filter { input, predicate } => {
            let inner = encode_relation(input, out_tuples, env)?;
            let cond = encode_expr_bool(predicate, out_tuples, env)?;
            Ok(Bool::and(&[&inner, &cond]))
        }
        Relation::Project { input, exprs, distinct: _ } => {
            encode_project(input, exprs, out_tuples, env)
        }
        Relation::Join { left, right, join_type, condition, natural, using_columns } => {
            encode_join(left, right, join_type, condition, natural, using_columns, out_tuples, env)
        }
        Relation::Union { left, right, all } => {
            let l = encode_relation(left, out_tuples, env)?;
            let r = encode_relation(right, out_tuples, env)?;
            if *all { Ok(Bool::or(&[&l, &r])) } else { /* bag union: distinct */ Ok(Bool::or(&[&l, &r])) }
        }
        Relation::GroupBy { input, keys, aggregates, having } => {
            encode_groupby(input, keys, aggregates, having.as_ref(), out_tuples, env)
        }
        Relation::OrderBy { input, items, limit, offset } => {
            encode_orderby(input, items, limit.as_ref(), offset.as_ref(), out_tuples, env)
        }
        Relation::Empty => Ok(Bool::from_bool(false)),
        // ...
        _ => Err(EncodeError::UnsupportedRelation),
    }
}

/// Encode an expression as a Z3 Int.
pub fn encode_expr_int(
    expr: &Expr,
    tuple: &Int<'ctx>,
    env: &Environment<'ctx>,
) -> Result<Int<'ctx>, EncodeError> {
    match expr {
        Expr::ColumnRef { table, column } => {
            let key = format!("{}.{}", table.as_deref().unwrap_or(""), column);
            let func = env.attr_funcs.get(&key)
                .ok_or_else(|| EncodeError::UnknownColumn(key.clone()))?;
            let args: Vec<&dyn Ast> = vec![tuple];
            func.apply(&args).as_int().ok_or(EncodeError::TypeMismatch)
        }
        Expr::Literal(ExprValue::Integer(v)) => Ok(Int::from_i64(*v)),
        Expr::BinaryOp { op, left, right } => {
            let l = encode_expr_int(left, tuple, env)?;
            let r = encode_expr_int(right, tuple, env)?;
            encode_binop_int(op, &l, &r)
        }
        Expr::Case { operand, whens, else_expr } => {
            encode_case(operand.as_deref(), whens, else_expr.as_deref(), tuple, env)
        }
        Expr::SqlNull => Ok(Int::fresh_const("SQL_NULL")),
        // ...
        _ => Err(EncodeError::UnsupportedExpr),
    }
}

/// Encode an expression as a Z3 Bool (for WHERE/HAVING/JOIN ON).
pub fn encode_expr_bool(
    expr: &Expr,
    tuple: &Int<'ctx>,
    env: &Environment<'ctx>,
) -> Result<Bool<'ctx>, EncodeError> { todo!() }

fn encode_base_table(/* ... */) -> Result<Bool<'ctx>, EncodeError> { todo!() }
fn encode_project(/* ... */) -> Result<Bool<'ctx>, EncodeError> { todo!() }
fn encode_join(/* ... */) -> Result<Bool<'ctx>, EncodeError> { todo!() }
fn encode_groupby(/* ... */) -> Result<Bool<'ctx>, EncodeError> { todo!() }
fn encode_orderby(/* ... */) -> Result<Bool<'ctx>, EncodeError> { todo!() }
fn encode_binop_int(op: &BinOp, l: &Int, r: &Int) -> Result<Int, EncodeError> { todo!() }
fn encode_case(/* ... */) -> Result<Int<'ctx>, EncodeError> { todo!() }

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("Unknown column: {0}")]
    UnknownColumn(String),
    #[error("Type mismatch in Z3 encoding")]
    TypeMismatch,
    #[error("Unsupported relation type")]
    UnsupportedRelation,
    #[error("Unsupported expression")]
    UnsupportedExpr,
}
```

**Key encoding rules:**

1. **NULL handling (three-valued logic):** `NULL(tuple, col_label) → Bool`. An expression is NULL if any of its column operands is NULL. Comparisons with NULL evaluate to `NULL` (not True/False). Use `If(NULL(t, col), fresh_bool, actual_comparison)` for conditions.

2. **Aggregate functions:** Use uninterpreted Z3 functions: `COUNT(tuple, agg_id) → Int`, `SUM(tuple, agg_id) → Int`, etc. These are declared lazily when first encountered. The GroupBy table maps input tuples to output tuples via `GroupByMap`.

3. **ORDER BY:** Uses VeriEQL's list semantics — tuples are compared positionally with ordering constraints as additional Z3 formulas.

4. **Outer joins:** The `DELETED` function marks tuples that are "deleted" (not participating in the join). For LEFT JOIN, right-side tuples can be marked DELETED if they don't match; the left-side tuple is never DELETED.

**Step 1: Write encoder.rs**

**Step 2: Verify compilation**

Run: `cargo build -p metamorphosis-verieql`

**Step 3: Commit**

```
git add crates/verieql/src/encoder.rs
git commit -m "feat(verieql): implement IR → Z3 constraint encoder"
```

---

## Task 7: Equivalence Verifier (verifier.rs)

**Files:**
- Write: `crates/verieql/src/verifier.rs`
- Modify: `crates/verieql/src/lib.rs`

**What this does:** Given two encoded Z3 formulas (one per query), check equivalence using bag or list semantics. Mirrors VeriEQL's `verifiers/bag_semantics_verifier.py` + `verifiers/list_semantics_verifier.py`.

**Bag semantics equivalence (VeriEQL core algorithm):**

For two tables L and R (each with B tuples):
1. Cardinality: `|L| = |R|` — same number of non-deleted tuples
2. For each tuple t in L: `count_L(t) = count_R(t)` — same multiplicity
3. For each tuple t in R: `count_L(t) = count_R(t)` — symmetric check

Where `count_X(t)` = `SUM(If(equals(t, t'), 1, 0) for t' in X)` using tuple equality formula:
```
tuple_equals(t1, t2) = Or(
    And(DELETED(t1), DELETED(t2)),
    And(Not(DELETED(t1)), Not(DELETED(t2)),
        col1_eq, col2_eq, ...)
)
```

```rust
use z3::ast::{Ast, Bool, Int};
use crate::environment::Environment;

/// Build the equivalence formula for two tables.
/// Returns: Bool formula that is True iff both tables are equivalent.
/// The caller asserts Not(formula) and checks UNSAT.
pub fn build_equivalence_formula<'ctx>(
    ltuples: &[Int<'ctx>],
    rtuples: &[Int<'ctx>],
    lattr_funcs: &[String],     // attribute function keys
    rattr_funcs: &[String],
    env: &Environment<'ctx>,
) -> Result<Bool<'ctx>, VerifierError> {
    match env.semantics {
        Semantics::Bag => build_bag_equivalence(ltuples, rtuples, lattr_funcs, rattr_funcs, env),
        Semantics::List => build_list_equivalence(ltuples, rtuples, lattr_funcs, rattr_funcs, env),
    }
}

fn build_bag_equivalence<'ctx>(
    ltuples: &[Int<'ctx>],
    rtuples: &[Int<'ctx>],
    lattr_funcs: &[String],
    rattr_funcs: &[String],
    env: &Environment<'ctx>,
) -> Result<Bool<'ctx>, VerifierError> {
    let mut formulas: Vec<Bool<'ctx>> = Vec::new();

    // 1. |L| = |R|
    let l_size = table_size(ltuples, env);
    let r_size = table_size(rtuples, env);
    formulas.push(l_size._eq(&r_size));

    // 2. For each tuple in L, count_L(t) = count_R(t)
    for (idx, lt) in ltuples.iter().enumerate() {
        if !all_deleted(lt, env)? {
            let count_in_l = count_equals(lt, ltuples, lattr_funcs, lattr_funcs, env)?;
            let count_in_r = count_equals(lt, rtuples, lattr_funcs, rattr_funcs, env)?;
            formulas.push(Bool::implies(
                &env.deleted_func.apply(&[lt]).as_bool().unwrap().not(),
                &count_in_l._eq(&count_in_r),
            ));
        }
    }

    // 3. For each tuple in R, count_L(t) = count_R(t)
    for rt in rtuples.iter() {
        if !all_deleted(rt, env)? {
            let count_in_l = count_equals(rt, ltuples, rattr_funcs, lattr_funcs, env)?;
            let count_in_r = count_equals(rt, rtuples, rattr_funcs, rattr_funcs, env)?;
            formulas.push(Bool::implies(
                &env.deleted_func.apply(&[rt]).as_bool().unwrap().not(),
                &count_in_l._eq(&count_in_r),
            ));
        }
    }

    Ok(Bool::and(&formulas.iter().collect::<Vec<_>>()))
}

/// Tuple equality: two tuples are equal if either both are DELETED,
/// or neither is DELETED and all attribute values match (or both are NULL).
fn tuple_equals<'ctx>(
    t1: &Int<'ctx>,
    t2: &Int<'ctx>,
    attr_funcs1: &[String],
    attr_funcs2: &[String],
    env: &Environment<'ctx>,
) -> Result<Bool<'ctx>, VerifierError> {
    let both_deleted = Bool::and(&[
        &env.deleted_func.apply(&[t1]).as_bool().unwrap(),
        &env.deleted_func.apply(&[t2]).as_bool().unwrap(),
    ]);

    let mut eqs = vec![
        env.deleted_func.apply(&[t1]).as_bool().unwrap().not(),
        env.deleted_func.apply(&[t2]).as_bool().unwrap().not(),
    ];

    for (a1, a2) in attr_funcs1.iter().zip(attr_funcs2.iter()) {
        let f1 = env.attr_funcs.get(a1).unwrap();
        let f2 = env.attr_funcs.get(a2).unwrap();
        let v1 = f1.apply(&[t1]).as_int().unwrap();
        let v2 = f2.apply(&[t2]).as_int().unwrap();
        eqs.push(Bool::or(&[
            &Bool::and(&[
                &env.null_func.apply(&[t1, &Int::from_i64(hash_str(a1))]).as_bool().unwrap(),
                &env.null_func.apply(&[t2, &Int::from_i64(hash_str(a2))]).as_bool().unwrap(),
            ]),
            &Bool::and(&[
                &env.null_func.apply(&[t1, &Int::from_i64(hash_str(a1))]).as_bool().unwrap().not(),
                &env.null_func.apply(&[t2, &Int::from_i64(hash_str(a2))]).as_bool().unwrap().not(),
                &v1._eq(&v2),
            ]),
        ]));
    }

    Ok(Bool::or(&[
        &both_deleted,
        &Bool::and(&eqs.iter().collect::<Vec<_>>()),
    ]))
}

fn table_size<'ctx>(tuples: &[Int<'ctx>], env: &Environment<'ctx>) -> Int<'ctx> {
    // SUM(If(Not(DELETED(t)), 1, 0))
    let terms: Vec<Int<'ctx>> = tuples.iter().map(|t| {
        z3::ast::Int::from(
            Bool::implies(
                &env.deleted_func.apply(&[t]).as_bool().unwrap().not(),
                &Int::from_i64(1),
            )
        )
    }).collect();
    // simplified: just count non-deleted
    Int::from_i64(tuples.len() as i64) // for non-deleted only — actual impl sums If expressions
}

fn count_equals<'ctx>(
    target: &Int<'ctx>,
    tuples: &[Int<'ctx>],
    target_attrs: &[String],
    tuple_attrs: &[String],
    env: &Environment<'ctx>,
) -> Result<Int<'ctx>, VerifierError> {
    let terms: Result<Vec<Int<'ctx>>, _> = tuples.iter().map(|t| {
        let eq = tuple_equals(target, t, target_attrs, tuple_attrs, env)?;
        Ok(Int::from(Bool::ite(&eq, &Int::from_i64(1), &Int::from_i64(0))))
    }).collect();
    Ok(Int::sum(&terms?))
}

fn build_list_equivalence<'ctx>(
    ltuples: &[Int<'ctx>],
    rtuples: &[Int<'ctx>],
    /* ... */
) -> Result<Bool<'ctx>, VerifierError> {
    // List semantics: positional comparison.
    // 1. |L| = |R|
    // 2. For each position i: tuple_equals(L[i], R[i])
    todo!("list semantics")
}
```

**Step 1: Write verifier.rs**

**Step 2: Verify compilation**

Run: `cargo build -p metamorphosis-verieql`

**Step 3: Commit**

```
git add crates/verieql/src/verifier.rs
git commit -m "feat(verieql): implement bag/list equivalence verifier"
```

---

## Task 8: Integrity Constraints (constraints.rs)

**Files:**
- Write: `crates/verieql/src/constraints.rs`
- Modify: `crates/verieql/src/lib.rs`

**What this does:** Parse integrity constraints (JSON format, same as VeriEQL) and convert them to Z3 DBMS facts. Mirrors VeriEQL's `environment.py` `add_constraints()` + `parsers/constraint_parser.py`.

**Constraint types:**
- `primary`: `[["EMP__ID"]]` → `Not(NULL(t, col))` for each tuple, `col(t1) ≠ col(t2)` for distinct tuples
- `foreign`: `[["EMP__DEPTNO"], ["DEPT__DEPTNO"]]` → `Not(NULL(t, col))`, each EMP tuple's DEPTNO must exist in DEPT
- `not_null`: `["EMP__NAME"]` → `Not(NULL(t, col))`
- `boolean` / `int` / `varchar` / `date`: type range constraints
- `inc`: auto-increment constraint
- `lt` / `lte` / `gt` / `gte` / `eq` / `neq`: value comparison constraints
- `between`: range constraint
- `in`: set membership constraint

```rust
use serde_json::Value;
use z3::ast::Bool;
use crate::environment::Environment;

/// Parse and apply integrity constraints from JSON.
pub fn apply_constraints<'ctx>(
    constraints_json: &Value,
    env: &mut Environment<'ctx>,
) -> Result<(), ConstraintError> {
    match constraints_json {
        Value::Array(arr) => {
            for constraint in arr {
                apply_single_constraint(constraint, env)?;
            }
            Ok(())
        }
        _ => Err(ConstraintError::InvalidFormat),
    }
}

fn apply_single_constraint<'ctx>(
    constraint: &Value,
    env: &mut Environment<'ctx>,
) -> Result<(), ConstraintError> {
    match constraint {
        Value::Object(obj) => {
            for (op, operands) in obj {
                match op.as_str() {
                    "primary" => apply_primary(operands, env),
                    "foreign" => apply_foreign(operands, env),
                    "not_null" => apply_not_null(operands, env),
                    "boolean" | "int" | "varchar" | "date" => apply_type_constraint(op, operands, env),
                    "lt" | "lte" | "gt" | "gte" | "eq" | "neq" => apply_comparison(op, operands, env),
                    "between" => apply_between(operands, env),
                    "in" => apply_in(operands, env),
                    "inc" => apply_increment(operands, env),
                    _ => return Err(ConstraintError::UnsupportedOperator(op.clone())),
                }?
            }
            Ok(())
        }
        _ => Err(ConstraintError::InvalidFormat),
    }
}

fn apply_primary<'ctx>(
    operands: &Value,
    env: &mut Environment<'ctx>,
) -> Result<(), ConstraintError> {
    // PRIMARY KEY: [["T__COL1", "T__COL2"]]
    // → Not(NULL(t, col)) for each column
    // → (col1(t1), col2(t1)) ≠ (col1(t2), col2(t2)) for distinct tuples
    todo!()
}

fn apply_foreign<'ctx>(/* ... */) -> Result<(), ConstraintError> {
    // FOREIGN KEY: [["EMP__DEPTNO"], ["DEPT__DEPTNO"]]
    // → For each EMP tuple t1, exists DEPT tuple t2 where EMP.DEPTNO(t1) = DEPT.DEPTNO(t2)
    todo!()
}
```

**Step 1: Write constraints.rs**

**Step 2: Verify compilation**

Run: `cargo build -p metamorphosis-verieql`

**Step 3: Commit**

```
git add crates/verieql/src/constraints.rs
git commit -m "feat(verieql): implement integrity constraint modeling"
```

---

## Task 9: Counterexample Extraction (counterexample.rs)

**Files:**
- Write: `crates/verieql/src/counterexample.rs`
- Modify: `crates/verieql/src/lib.rs`

**What this does:** When Z3 finds a SAT model (queries are NOT equivalent), extract the model and build a human-readable counterexample. Mirrors VeriEQL's `environment.py` `compare()` counterexample extraction (lines 956-1055).

```rust
use z3::{Model, ast::Int};
use crate::types::{Counterexample, CounterexampleTable};
use crate::environment::Environment;

/// Extract a counterexample from a Z3 model.
pub fn extract_counterexample<'ctx>(
    model: &Model<'ctx>,
    env: &Environment<'ctx>,
    table_names: &[String],
    bound: usize,
) -> Counterexample {
    let mut tables = Vec::new();

    for name in table_names {
        let mut rows = Vec::new();
        for i in 0..bound {
            let tuple = &env.tuples[i]; // symbolic tuple variable
            let deleted = model.eval(
                &env.deleted_func.apply(&[tuple]).as_bool().unwrap(), true
            ).unwrap().as_bool().unwrap();

            if deleted {
                continue; // skip deleted tuples
            }

            let mut row = Vec::new();
            for (col_key, func) in &env.attr_funcs {
                if col_key.starts_with(name) {
                    let val = model.eval(
                        &func.apply(&[tuple]).as_int().unwrap(), false
                    ).unwrap().as_i64().unwrap();
                    // Check if NULL
                    let is_null = model.eval(
                        &env.null_func.apply(&[
                            tuple,
                            &Int::from_i64(hash_str(&col_key.split('.').last().unwrap())),
                        ]).as_bool().unwrap(), true
                    ).unwrap().as_bool().unwrap();
                    if is_null {
                        row.push("NULL".to_string());
                    } else {
                        row.push(val.to_string());
                    }
                }
            }
            rows.push(row);
        }
        tables.push(CounterexampleTable { name: name.clone(), rows });
    }

    Counterexample {
        tables,
        sql1_result: vec![],
        sql2_result: vec![],
    }
}

fn hash_str(s: &str) -> i64 {
    // Simple hash for Z3 string labels
    s.bytes().fold(0i64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as i64))
}
```

**Step 1: Write counterexample.rs**

**Step 2: Verify compilation**

Run: `cargo build -p metamorphosis-verieql`

**Step 3: Commit**

```
git add crates/verieql/src/counterexample.rs
git commit -m "feat(verieql): implement counterexample extraction from Z3 model"
```

---

## Task 10: Public API + Wire Everything Together (lib.rs)

**Files:**
- Modify: `crates/verieql/src/lib.rs`

**What this does:** Expose the `VeriEql` struct with a clean public API that users call. Orchestrates: parse → translate → create symbolic DB → encode → verify → extract counterexample.

```rust
use std::time::Instant;
use z3::{Config, Context};

use crate::types::*;
use crate::translator::Translator;
use crate::environment::Environment;
use crate::encoder::encode_relation;
use crate::verifier::build_equivalence_formula;
use crate::constraints::apply_constraints;
use crate::counterexample::extract_counterexample;

pub struct VeriEql;

impl VeriEql {
    /// Verify whether two SQL queries are equivalent under the given schema and constraints.
    ///
    /// # Arguments
    /// * `sql1` - First SQL query (SELECT statement)
    /// * `sql2` - Second SQL query (SELECT statement)
    /// * `schema` - Table definitions
    /// * `constraints` - Integrity constraints (JSON Value, same format as VeriEQL)
    /// * `bound` - Tuple bound for bounded model checking
    /// * `semantics` - Bag or List semantics
    ///
    /// # Returns
    /// `ProofReport` with result, timing, and optional counterexample.
    pub fn verify(
        sql1: &str,
        sql2: &str,
        schema: &[TableSchema],
        constraints: &serde_json::Value,
        bound: Bound,
        semantics: Semantics,
    ) -> Result<ProofReport, VeriEqlError> {
        let t0 = Instant::now();

        // 1. Parse SQL
        let stmt1 = Self::parse_sql(sql1)?;
        let stmt2 = Self::parse_sql(sql2)?;

        // 2. Translate to IR
        let ir1 = Translator::translate(&stmt1)?;
        let ir2 = Translator::translate(&stmt2)?;

        let translate_ms = t0.elapsed().as_millis() as u64;
        let t_solve = Instant::now();

        // 3. Create Z3 context
        let cfg = Config::new();
        let ctx = Context::new(&cfg);

        // 4. Build symbolic database environment
        let mut env = Environment::new(&ctx, bound.clone(), semantics);
        env.init_functions();
        for table_schema in schema {
            env.create_database(table_schema, bound.0)?;
        }

        // 5. Apply integrity constraints
        apply_constraints(constraints, &mut env)?;

        // 6. Encode both queries as Z3 constraints
        let formula1 = encode_relation(&ir1, &env.tuples, &env)?;
        let formula2 = encode_relation(&ir2, &env.tuples, &env)?;

        // 7. Build equivalence formula + DBMS facts
        let equiv = build_equivalence_formula(
            &env.tuples, &env.tuples,
            &vec![], &vec![], // simplified - real impl tracks attribute keys
            &env,
        )?;

        // 8. Check: Not(equivalence) → UNSAT = Equivalent
        //    Assert DBMS facts + negated equivalence
        for fact in &env.dbms_facts {
            env.solver.assert(fact);
        }
        env.solver.assert(&equiv.not());

        // 9. Solve
        let result = match env.solver.check() {
            z3::SatResult::Unsat => ProofResult::Equivalent,
            z3::SatResult::Sat => {
                let model = env.solver.get_model().unwrap();
                let table_names: Vec<String> = schema.iter().map(|s| s.name.clone()).collect();
                let ce = extract_counterexample(&model, &env, &table_names, bound.0);
                ProofResult::NotEquivalent { counterexample: ce }
            }
            z3::SatResult::Unknown => ProofResult::Unknown {
                reason: "Z3 returned Unknown".to_string(),
            },
        };

        let solve_ms = t_solve.elapsed().as_millis() as u64;

        Ok(ProofReport {
            result,
            translate_ms,
            solve_ms,
            bound,
        })
    }

    fn parse_sql(sql: &str) -> Result<ogsql_parser::ast::Statement, VeriEqlError> {
        use ogsql_parser::{Tokenizer, parser::Parser};
        let tokens = Tokenizer::new(sql).tokenize()
            .map_err(|e| VeriEqlError::ParseError(e.to_string()))?;
        let stmts = Parser::new(tokens).parse()
            .map_err(|e| VeriEqlError::ParseError(e.to_string()))?;
        stmts.into_iter().next()
            .ok_or_else(|| VeriEqlError::ParseError("Empty SQL input".to_string()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VeriEqlError {
    #[error("SQL parse error: {0}")]
    ParseError(String),
    #[error("Translation error: {0}")]
    TranslateError(#[from] crate::translator::TranslateError),
    #[error("Encoding error: {0}")]
    EncodeError(#[from] crate::encoder::EncodeError),
    #[error("Verification error: {0}")]
    VerifyError(#[from] crate::verifier::VerifierError),
    #[error("Constraint error: {0}")]
    ConstraintError(#[from] crate::constraints::ConstraintError),
    #[error("Z3 error: {0}")]
    Z3Error(String),
}
```

**Step 1: Write the full lib.rs with all imports and error wiring**

**Step 2: Verify compilation**

Run: `cargo build -p metamorphosis-verieql`

**Step 3: Commit**

```
git add crates/verieql/src/lib.rs
git commit -m "feat(verieql): wire public API — parse → translate → encode → verify"
```

---

## Task 11: Integration Tests

**Files:**
- Create: `crates/verieql/tests/integration_test.rs`
- Create: `crates/verieql/tests/fixtures/simple_eq.json`
- Create: `crates/verieql/tests/fixtures/simple_neq.json`

**What this does:** End-to-end tests that call `VeriEql::verify()` on known-equivalent and known-inequivalent SQL pairs.

**Test fixture: simple_eq.json**
```json
{
  "description": "SELECT DISTINCT ID FROM EMP vs SELECT ID FROM EMP GROUP BY ID",
  "sql1": "SELECT DISTINCT ID FROM EMP",
  "sql2": "SELECT ID FROM EMP GROUP BY ID",
  "schema": [
    {
      "name": "EMP",
      "columns": [
        {"name": "ID", "col_type": "Integer"},
        {"name": "NAME", "col_type": "Varchar"}
      ]
    }
  ],
  "constraints": [{"primary": [["EMP__ID"]]}],
  "bound": 2,
  "semantics": "Bag",
  "expected": "Equivalent"
}
```

**Test fixture: simple_neq.json**
```json
{
  "description": "SELECT * FROM EMP vs SELECT * FROM EMP WHERE ID > 10",
  "sql1": "SELECT * FROM EMP",
  "sql2": "SELECT * FROM EMP WHERE ID > 10",
  "schema": [
    {
      "name": "EMP",
      "columns": [
        {"name": "ID", "col_type": "Integer"}
      ]
    }
  ],
  "constraints": [],
  "bound": 2,
  "semantics": "Bag",
  "expected": "NotEquivalent"
}
```

**Test code:**
```rust
use serde::Deserialize;
use metamorphosis_verieql::{VeriEql, types::*, ProofResult};

#[derive(Deserialize)]
struct TestCase {
    description: String,
    sql1: String,
    sql2: String,
    schema: Vec<TableSchema>,
    constraints: serde_json::Value,
    bound: usize,
    semantics: String,
    expected: String,
}

#[test]
fn test_simple_equivalent() {
    let data = std::fs::read_to_string("tests/fixtures/simple_eq.json").unwrap();
    let tc: TestCase = serde_json::from_str(&data).unwrap();

    let semantics = match tc.semantics.as_str() {
        "Bag" => Semantics::Bag,
        "List" => Semantics::List,
        _ => panic!("unknown semantics"),
    };

    let report = VeriEql::verify(
        &tc.sql1,
        &tc.sql2,
        &tc.schema,
        &tc.constraints,
        Bound(tc.bound),
        semantics,
    ).unwrap_or_else(|e| panic!("{tc.description}: {e}"));

    match tc.expected.as_str() {
        "Equivalent" => assert!(matches!(report.result, ProofResult::Equivalent),
            "Expected Equivalent, got {:?}", report.result),
        "NotEquivalent" => assert!(matches!(report.result, ProofResult::NotEquivalent { .. }),
            "Expected NotEquivalent, got {:?}", report.result),
        _ => {}
    }
}

#[test]
fn test_simple_not_equivalent() {
    let data = std::fs::read_to_string("tests/fixtures/simple_neq.json").unwrap();
    let tc: TestCase = serde_json::from_str(&data).unwrap();

    let semantics = match tc.semantics.as_str() {
        "Bag" => Semantics::Bag,
        "List" => Semantics::List,
        _ => panic!("unknown semantics"),
    };

    let report = VeriEql::verify(
        &tc.sql1,
        &tc.sql2,
        &tc.schema,
        &tc.constraints,
        Bound(tc.bound),
        semantics,
    ).unwrap_or_else(|e| panic!("{tc.description}: {e}"));

    match tc.expected.as_str() {
        "NotEquivalent" => assert!(matches!(report.result, ProofResult::NotEquivalent { .. }),
            "Expected NotEquivalent, got {:?}", report.result),
        _ => {}
    }
}
```

**Step 1: Create test fixtures and test file**

**Step 2: Run tests**

Run: `cargo test -p metamorphosis-verieql`
Expected: 2 tests pass (may need bound tuning).

**Step 3: Commit**

```
git add crates/verieql/tests/
git commit -m "test(verieql): add integration tests for equivalence/inequivalence"
```

---

## Dependency Graph (Final)

```
metamorphosis-verieql
├── ogsql-parser (git)     # SQL parsing, AST types
├── z3 (vendored)          # SMT solver
├── serde / serde_json     # Serialization for constraints + reports
├── thiserror              # Error types
└── tracing                # Logging
```

**No dependencies on:** `metamorphosis-core`, `metamorphosis-rules`, `metamorphosis-qed`, `metamorphosis-cli`.

---

## Effort Summary

| Task | Description | Estimated Time |
|---|---|---|
| 1 | Scaffold crate | 30 min |
| 2 | Core types | 1 hr |
| 3 | IR types | 2 hr |
| 4 | Symbolic environment | 3-4 hr |
| 5 | Translator (ogsql → IR) | 3-5 hr |
| 6 | Encoder (IR → Z3) | 5-8 hr |
| 7 | Verifier (bag/list) | 3-5 hr |
| 8 | Constraints | 2-3 hr |
| 9 | Counterexample | 1-2 hr |
| 10 | Public API wiring | 1 hr |
| 11 | Integration tests | 2 hr |
| **Total** | | **24-34 hr** (~3-4 working days) |

