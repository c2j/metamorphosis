# Plan: `inline` Command

Replace SQL parameters/placeholders with literal values to produce directly executable SQL. Supports CLI and MCP.

## Three Parameter Styles

| Source | AST Node | Has Name? | Example |
|--------|----------|-----------|---------|
| Stored proc variables | `Expr::ColumnRef(Vec<String>)` | Yes (needs `known_variables`) | `WHERE t.col = in_accnt_date` |
| MyBatis XML | `Expr::MyBatisParam(String)` / `Expr::MyBatisRawExpr(String)` | Yes | `WHERE col = #{status}` |
| JDBC PreparedStatement | `Expr::JdbcParam` / `Expr::Parameter(i32)` | No (positional) | `WHERE col = ?` |

## Architecture

```
crates/core/src/inline.rs        ← Core logic (CLI + MCP shared)
crates/cli/src/inline_cmd.rs     ← CLI handler
crates/mcp-server/src/types.rs   ← MCP types (+)
crates/mcp-server/src/tools.rs   ← MCP tool impl (+)
crates/mcp-server/src/server.rs  ← MCP tool registration (+)
```

---

## Phase 1: Core Logic (`crates/core/src/inline.rs`)

### 1.1 Types

```rust
/// Parameter value — maps to SQL literal types
#[derive(Debug, Clone)]
pub enum InlineValue {
    String(String),
    Integer(i64),
    Float(String),   // matches Literal::Float(String)
    Boolean(bool),
    Null,
}

impl InlineValue {
    pub fn to_sql_literal(&self) -> String { ... }
    pub fn to_expr(&self) -> Expr {
        match self {
            Self::Null => Expr::Literal(Literal::Null),
            Self::Boolean(b) => Expr::Literal(Literal::Boolean(*b)),
            Self::Integer(n) => Expr::Literal(Literal::Integer(*n)),
            Self::Float(s) => Expr::Literal(Literal::Float(s.clone())),
            Self::String(s) => Expr::Literal(Literal::String(s.clone())),
        }
    }
}

/// Parameter collection
#[derive(Debug, Clone, Default)]
pub struct InlineParams {
    pub named: HashMap<String, InlineValue>,
    pub positional: Vec<InlineValue>,
}

/// Tracking stats during substitution
#[derive(Debug, Default)]
struct InlineStats {
    replaced_named: usize,
    replaced_positional: usize,
    remaining: Vec<RemainingPlaceholder>,
}

#[derive(Debug, Clone)]
pub struct RemainingPlaceholder {
    pub kind: &'static str,   // "jdbc" | "mybatis" | "parameter" | "variable"
    pub name: Option<String>,
    pub position: Option<usize>,
}

/// Result of inlining one statement
#[derive(Debug)]
pub struct InlineResult {
    pub statement: Statement,
    pub replaced_named: usize,
    pub replaced_positional: usize,
    pub remaining: Vec<RemainingPlaceholder>,
}
```

### 1.2 Entry Function

```rust
pub fn inline_statement(
    stmt: &Statement,
    params: &InlineParams,
    known_variables: Option<&HashSet<String>>,
) -> InlineResult
```

Dispatches by Statement variant:
- `Statement::Select` → `inline_select`
- `Statement::Update` → `inline_update`
- `Statement::Delete` → `inline_delete`
- `Statement::Insert` → `inline_insert`
- Other → return as-is (no params expected)

### 1.3 Statement-Level Walkers

Each walker traverses fields in **source order** to guarantee `?` positional accuracy.

**`inline_select`** — order: `targets → from → where_clause → connect_by → group_by → having → order_by → limit → offset`, then recurse into `with` (CTEs) and `set_operation` (UNION/INTERSECT/EXCEPT).

**`inline_update`** — order: `assignments(value) → from → where_clause → order_by → limit → returning`, plus `with`.

**`inline_delete`** — order: `using → where_clause → order_by → limit`, plus `with`.

**`inline_insert`** — order: values/SELECT subquery, plus `with`.

### 1.4 Expression-Level Walker

```rust
fn substitute_expr(
    expr: &Expr,
    params: &InlineParams,
    known_vars: Option<&HashSet<String>>,
    pos_counter: &mut usize,
    stats: &mut InlineStats,
) -> Expr
```

Skeleton (mirrors `eq_analyzer.rs:294-337` `contains_param`):

```rust
match expr {
    // ── Positional: JDBC ? ──
    Expr::JdbcParam => {
        match params.positional.get(*pos_counter) {
            Some(val) => { *pos_counter += 1; stats.replaced_positional += 1; val.to_expr() }
            None => { stats.remaining.push(/* jdbc, position */); expr.clone() }
        }
    }

    // ── Positional: $1, $2 (Parameter(i32), 1-indexed) ──
    Expr::Parameter(n) => {
        // Parameter(n) is 1-indexed: $1 → positional[0]
        let idx = (*n as usize).saturating_sub(1);
        match params.positional.get(idx) {
            Some(val) => { stats.replaced_positional += 1; val.to_expr() }
            None => { stats.remaining.push(/* parameter, position=n */); expr.clone() }
        }
    }

    // ── Named: MyBatis #{name} ──
    Expr::MyBatisParam(name) | Expr::MyBatisRawExpr(name) => {
        match params.named.get(name) {
            Some(val) => { stats.replaced_named += 1; val.to_expr() }
            None => { stats.remaining.push(/* mybatis, name */); expr.clone() }
        }
    }

    // ── Named: Stored proc variable (ColumnRef, double-gated) ──
    Expr::ColumnRef(parts) => {
        if let (Some(vars), Some(name)) = (known_vars, parts.last()) {
            if vars.contains(name) {
                if let Some(val) = params.named.get(name) {
                    stats.replaced_named += 1;
                    return val.to_expr();
                }
                stats.remaining.push(/* variable, name */);
            }
        }
        expr.clone()  // Not a known variable → leave as-is
    }

    // ── Recursive variants ──
    Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
        left: Box::new(substitute_expr(left, ...)),
        op: op.clone(),
        right: Box::new(substitute_expr(right, ...)),
    },
    Expr::UnaryOp { op, expr } => Expr::UnaryOp {
        op: op.clone(),
        expr: Box::new(substitute_expr(expr, ...)),
    },
    Expr::Parenthesized(inner) => Expr::Parenthesized(Box::new(substitute_expr(inner, ...))),
    Expr::IsNull { expr, negated } => Expr::IsNull {
        expr: Box::new(substitute_expr(expr, ...)),
        negated: *negated,
    },
    Expr::IsBoolean { expr, value, negated } => { /* recurse expr */ },
    Expr::TypeCast { expr, type_name, .. } => { /* recurse expr */ },
    Expr::Treat { expr, type_name } => { /* recurse expr */ },
    Expr::FunctionCall { name, args, filter, .. } => {
        // recurse all args + filter
    },
    Expr::SpecialFunction { name, args } => { /* recurse args */ },
    Expr::Case { operand, whens, else_expr } => {
        // recurse operand + each WhenClause { condition, result } + else_expr
    },
    Expr::Between { expr, low, high, negated } => { /* recurse all three */ },
    Expr::InList { expr, list, negated } => {
        // recurse expr + each item in list
    },
    Expr::Like { expr, pattern, escape, .. } => { /* recurse expr + pattern + escape */ },
    Expr::Subscript { object, lower, upper, is_slice } => { /* recurse all */ },
    Expr::Array(exprs) => Expr::Array(exprs.iter().map(|e| substitute_expr(e, ...)).collect()),
    Expr::RowConstructor(exprs) => { /* recurse all */ },
    Expr::CollationFor { expr } => { /* recurse expr */ },
    Expr::Prior(inner) => { /* recurse inner */ },

    // ── Subquery-containing (recurse into inner SELECT) ──
    Expr::Exists(select) => { /* recurse inner select */ },
    Expr::Subquery(select) => { /* recurse inner select */ },
    Expr::InSubquery { expr, subquery, negated } => { /* recurse expr + inner select */ },
    Expr::ScalarSublink { expr, op, sublink_type, subquery } => { /* recurse expr + inner */ },

    // ── No sub-expressions ──
    Expr::Literal(_) | Expr::QualifiedStar(_) | Expr::Default
    | Expr::CurrentOf { .. } | Expr::PredictBy { .. }
    | Expr::XmlElement { .. } | Expr::XmlConcat(_) | Expr::XmlForest(_)
    | Expr::XmlParse { .. } | Expr::XmlPi { .. } | Expr::XmlRoot { .. }
    | Expr::XmlSerialize { .. } | Expr::FieldAccess { .. } => expr.clone(),
}
```

### 1.5 Value Inference (for CLI string inputs)

```rust
/// Infer InlineValue from a CLI string argument
pub fn infer_value(s: &str) -> InlineValue {
    let upper = s.to_uppercase();
    match upper.as_str() {
        "NULL" => return InlineValue::Null,
        "TRUE" => return InlineValue::Boolean(true),
        "FALSE" => return InlineValue::Boolean(false),
        _ => {}
    }
    if let Ok(n) = s.parse::<i64>() {
        return InlineValue::Integer(n);
    }
    if s.parse::<f64>().is_ok() {
        return InlineValue::Float(s.to_string());
    }
    InlineValue::String(s.to_string())
}
```

### 1.6 JSON Value Conversion

```rust
/// Convert serde_json::Value to InlineValue (for --params file and MCP)
pub fn json_to_inline_value(v: &serde_json::Value) -> InlineValue {
    match v {
        Value::Null => InlineValue::Null,
        Value::Bool(b) => InlineValue::Boolean(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() { InlineValue::Integer(i) }
            else { InlineValue::Float(n.to_string()) }
        }
        Value::String(s) => InlineValue::String(s.clone()),
        _ => InlineValue::String(v.to_string()),
    }
}
```

### 1.7 Unit Tests

Cover:
- Value formatting: all InlineValue variants → correct SQL literal
- String escaping: `O'Brien` → `'O''Brien'`
- JDBC `?`: single, multiple, nested in CASE/function/BETWEEN
- MyBatis `#{name}`: matched, unmatched
- Stored proc variable: with/without `known_variables`
- `$1` numbered parameters
- `Expr::Parameter` positional mapping (1-indexed)
- Missing positional values (remaining placeholder reported)
- Extra params (not an error, just unused)
- Multiple statements (positional counter resets per statement)

---

## Phase 2: CLI Integration

### 2.1 `crates/cli/src/main.rs` — Command enum + dispatch

Add to `Command` enum:
```rust
/// Replace parameters/placeholders with literal values to produce executable SQL
Inline {
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long = "param")]
    params_named: Vec<String>,
    #[arg(long = "val")]
    params_positional: Vec<String>,
    #[arg(long)]
    params_file: Option<PathBuf>,
    #[arg(long)]
    mybatis: bool,
    #[arg(long)]
    procedure: Option<PathBuf>,
    #[arg(long)]
    from_procedure: bool,
    #[arg(short = 'o', long = "output", default_value_t = OutputFormat::SqlOnly)]
    output: OutputFormat,
},
```

Add to `match cli.command` dispatch:
```rust
Command::Inline { file, params_named, params_positional, params_file, mybatis, procedure, from_procedure, output } => {
    inline_cmd::run_inline(file, params_named, params_positional, params_file, mybatis, procedure, from_procedure, output);
}
```

### 2.2 `crates/cli/src/inline_cmd.rs`

```rust
pub fn run_inline(
    file: Option<PathBuf>,
    params_named: Vec<String>,
    params_positional: Vec<String>,
    params_file: Option<PathBuf>,
    mybatis: bool,
    procedure: Option<PathBuf>,
    from_procedure: bool,
    output: OutputFormat,
) {
    // 1. Build InlineParams from all sources
    let mut params = InlineParams::default();
    if let Some(path) = params_file { params.merge_json_file(&path); }
    for kv in &params_named { params.add_named_str(kv); }     // "status=active" → infer_value
    for v in &params_positional { params.add_positional_str(v); }

    // 2. Resolve known_variables
    let known_vars = if from_procedure {
        // Extract SQL + variables from procedure file
        analyze_procedure_and_inline(file, &params, mybatis, &output);
        return;
    } else {
        load_procedure_variables(procedure)
    };

    // 3. Parse SQL
    let (sql, source_label) = resolve_input(&file);
    let stmts = parse_sql(&sql, mybatis);

    // 4. Inline each statement
    let results: Vec<InlineResult> = stmts.iter()
        .map(|stmt| inline_statement(stmt, &params, known_vars.as_ref()))
        .collect();

    // 5. Output
    match output {
        OutputFormat::SqlOnly => print_sql_only(&results),
        OutputFormat::Text => print_text(&results, &source_label),
        OutputFormat::Json => print_json(&results),
        OutputFormat::Tsv | OutputFormat::Csv => { /* optional */ }
    }
}
```

### 2.3 CLI Integration Tests (`crates/cli/tests/inline_test.rs`)

- `case-wenhao.sql` with `--val ACC001` → produces `WHERE b.acnt_id = 'ACC001'`
- MyBatis: `#{status}` with `--param status=active`
- Stored proc: `case1.sql` with `--from-procedure --param in_accnt_date=20240101`
- JSON params file
- Missing params (remaining placeholders reported)
- `--help` shows inline command

---

## Phase 3: MCP Integration

### 3.1 `crates/mcp-server/src/types.rs` — Add types

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct InlineSqlParams {
    /// SQL text (supports multiple statements)
    pub sql: String,
    /// Named params: {"status": "active", "count": 42, "flag": true, "note": null}
    #[serde(default)]
    pub named: HashMap<String, serde_json::Value>,
    /// Positional params (JDBC ?): ["en", 1, null, true]
    #[serde(default)]
    pub positional: Vec<serde_json::Value>,
    /// Enable MyBatis #{}/${} parsing
    #[serde(default)]
    pub mybatis: bool,
    /// Known variable names (stored proc mode, distinguishes variables from columns)
    pub known_variables: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct InlineResponse {
    pub inlined_sql: Vec<String>,
    pub total_replaced: usize,
    pub remaining_placeholders: Vec<RemainingPlaceholderInfo>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RemainingPlaceholderInfo {
    pub kind: String,
    pub name: Option<String>,
    pub position: Option<usize>,
    pub statement_index: usize,
}
```

### 3.2 `crates/mcp-server/src/tools.rs` — Add tool function

```rust
pub fn inline_sql(params: crate::types::InlineSqlParams) -> Result<InlineResponse, String> {
    // 1. Convert JSON params → InlineParams
    let inline_params = InlineParams {
        named: params.named.iter().map(|(k, v)| (k.clone(), json_to_inline_value(v))).collect(),
        positional: params.positional.iter().map(json_to_inline_value).collect(),
    };

    // 2. known_variables
    let known_vars = params.known_variables.map(|v| v.into_iter().collect::<HashSet<_>>());

    // 3. Parse SQL with optional mybatis
    let (stmt_infos, warnings) = if params.mybatis {
        parse_sql_mybatis(&params.sql)
    } else {
        parse_sql(&params.sql)
    };

    // 4. Inline each statement
    // 5. Build response
}
```

### 3.3 `crates/mcp-server/src/server.rs` — Register tool

```rust
#[rmcp::tool(
    name = "inline_sql",
    description = "Replace SQL parameters and placeholders with literal values \
        to produce directly executable SQL. Supports named params (MyBatis #{name}, \
        stored proc variables), positional params (JDBC ?), and numbered params ($1)."
)]
async fn inline_sql(&self, Parameters(params): Parameters<InlineSqlParams>) -> String {
    // same pattern as other tools
}
```

---

## Implementation Order

```
Phase 1 (core) ──┬── 1.1 Types + InlineValue impls
                 ├── 1.2 inline_statement entry
                 ├── 1.3 Statement walkers (Select first, then Update/Delete/Insert)
                 ├── 1.4 substitute_expr (full coverage)
                 ├── 1.5-1.6 infer_value + json_to_inline_value
                 └── 1.7 Unit tests

Phase 2 (CLI) ───┬── 2.1 main.rs: Command enum + dispatch
                 ├── 2.2 inline_cmd.rs: handler
                 └── 2.3 Integration tests

Phase 3 (MCP) ───┬── 3.1 types.rs
                 ├── 3.2 tools.rs
                 └── 3.3 server.rs

Final ──────────── cargo build --workspace && cargo test --workspace
```

Phase 2 and Phase 3 can run in parallel after Phase 1.

## Constraints

- No `unwrap()` in lib code (use `expect()` with justification or `?`)
- `tracing` for logging (not `println!` in lib)
- All `pub` items need doc comments
- `#[non_exhaustive]` on exported structs/enums
- Max 600 lines per `.rs` file
- `thiserror` for errors in lib code
- File names: `as_`/`to_`/`into_` by ownership semantics
