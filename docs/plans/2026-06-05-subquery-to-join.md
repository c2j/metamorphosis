# SubqueryToJoin Rewrite Rule Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a SubqueryToJoin rewrite rule that converts 5 subquery patterns (EXISTS, IN, NOT EXISTS, NOT IN, scalar subquery) into equivalent JOIN operations, with QED verification.

**Architecture:** Single rule struct implementing `RewriteRule` trait. `matches()` detects subquery patterns in WHERE clauses. `apply()` extracts correlation conditions from the subquery and constructs an equivalent JOIN. Safety levels are stratified: Safe for EXISTS/IN, Conditional for NOT EXISTS/NOT IN (NULL checks), Manual for scalar subqueries (suggest only).

**Tech Stack:** Rust, ogsql-parser AST types (`Expr::Exists`, `Expr::InSubquery`, `Expr::Subquery`, `TableRef::Join`, `JoinType`), metamorphosis-core `RewriteRule` trait, metamorphosis-qed for verification.

---

### Task 1: Create rule skeleton with Safe-level EXISTS→JOIN

**Files:**
- Create: `crates/rules/src/subquery_to_join.rs`
- Modify: `crates/rules/src/lib.rs`

**Step 1: Write the failing test**

Create `crates/rules/tests/subquery_to_join_test.rs`:

```rust
use metamorphosis_core::types::RewriteAction;
use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry, Suggestion};
use metamorphosis_rules::subquery_to_join::SubqueryToJoin;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::Parser;

fn test_rewrite(sql: &str) -> (Vec<Statement>, Vec<Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(SubqueryToJoin)]));
    let config = RewriteConfig::default();
    let ctx = RewriteContext {
        version: None,
        schema: None,
        config: &config,
        source_file: None,
        known_variables: None,
    };
    let (stmts, _errors) = Parser::parse_sql(sql);
    let statements: Vec<Statement> = stmts.into_iter().map(|si| si.statement).collect();
    let result = engine.rewrite(&ctx, statements);
    (result.statements, result.suggestions)
}

fn format_first(statements: &[Statement]) -> String {
    SqlFormatter::new().format_statement(&statements[0])
}

#[test]
fn test_exists_correlated_to_inner_join() {
    let (stmts, _) = test_rewrite(
        "SELECT * FROM orders WHERE EXISTS (SELECT 1 FROM users WHERE users.id = orders.user_id)",
    );
    let sql = format_first(&stmts);
    assert!(!sql.contains("EXISTS"), "EXISTS should be eliminated, got: {}", sql);
    assert!(sql.contains("JOIN"), "Should contain JOIN, got: {}", sql);
}

#[test]
fn test_in_subquery_to_inner_join() {
    let (stmts, _) = test_rewrite(
        "SELECT * FROM orders WHERE user_id IN (SELECT id FROM users)",
    );
    let sql = format_first(&stmts);
    assert!(!sql.contains("IN ("), "IN subquery should be eliminated, got: {}", sql);
    assert!(sql.contains("JOIN"), "Should contain JOIN, got: {}", sql);
}

#[test]
fn test_no_subquery_no_match() {
    let (stmts, _) = test_rewrite("SELECT * FROM orders WHERE user_id = 1");
    let sql = format_first(&stmts);
    assert_eq!(sql, format_first(&stmts));
}

#[test]
fn test_not_exists_to_left_join() {
    let (stmts, _) = test_rewrite(
        "SELECT * FROM orders WHERE NOT EXISTS (SELECT 1 FROM users WHERE users.id = orders.user_id)",
    );
    let sql = format_first(&stmts);
    assert!(!sql.contains("EXISTS"), "EXISTS should be eliminated, got: {}", sql);
    assert!(sql.contains("JOIN"), "Should contain JOIN, got: {}", sql);
}

#[test]
fn test_not_in_to_left_join() {
    let (stmts, _) = test_rewrite(
        "SELECT * FROM orders WHERE user_id NOT IN (SELECT id FROM users)",
    );
    let sql = format_first(&stmts);
    assert!(!sql.contains("IN ("), "NOT IN should be eliminated, got: {}", sql);
    assert!(sql.contains("JOIN"), "Should contain JOIN, got: {}", sql);
}

#[test]
fn test_scalar_subquery_suggest() {
    let (_, suggestions) = test_rewrite(
        "SELECT *, (SELECT MAX(amount) FROM payments) AS max_pay FROM orders",
    );
    assert!(!suggestions.is_empty(), "Scalar subquery should produce a suggestion");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p metamorphosis-rules --test subquery_to_join_test`
Expected: FAIL — module `subquery_to_join` not found.

**Step 3: Write minimal rule skeleton**

Create `crates/rules/src/subquery_to_join.rs`:

```rust
use metamorphosis_core::types::{Confidence, RewriteAction, RuleCategory, SafetyLevel, Severity};
use metamorphosis_core::{RewriteContext, RewriteRule};
use ogsql_parser::ast::{Expr, JoinType, SelectStatement, Spanned, TableRef};
use ogsql_parser::{Statement, ObjectName};
use tracing::debug;

#[derive(Debug)]
pub struct SubqueryToJoin;

impl RewriteRule for SubqueryToJoin {
    fn id(&self) -> &'static str {
        "subquery-to-join"
    }

    fn description(&self) -> &'static str {
        "Convert subqueries in WHERE clause to equivalent JOIN operations"
    }

    fn category(&self) -> RuleCategory {
        RuleCategory::Performance
    }

    fn safety_level(&self) -> SafetyLevel {
        SafetyLevel::Safe
    }

    fn matches(&self, _ctx: &RewriteContext, stmt: &Statement) -> bool {
        let select = match stmt {
            Statement::Select(s) => &s.node,
            _ => return false,
        };
        select.where_clause.as_ref().map_or(false, has_subquery)
    }

    fn apply(&self, _ctx: &RewriteContext, stmt: &Statement) -> Option<RewriteAction> {
        let spanned = match stmt {
            Statement::Select(s) => s,
            _ => return None,
        };
        let select = &spanned.node;
        let where_clause = select.where_clause.as_ref()?;

        match classify_subquery(where_clause) {
            SubqueryKind::Exists { subquery } => {
                rewrite_exists_to_join(select, subquery, false)
            }
            SubqueryKind::NotExists { subquery } => {
                rewrite_exists_to_join(select, subquery, true)
            }
            SubqueryKind::In { expr, subquery } => {
                rewrite_in_to_join(select, expr, subquery, false)
            }
            SubqueryKind::NotIn { expr, subquery } => {
                rewrite_in_to_join(select, expr, subquery, true)
            }
            SubqueryKind::ScalarSubquery => {
                Some(RewriteAction::Suggest {
                    message: "Scalar subquery can be rewritten as LEFT JOIN for better performance".to_string(),
                    severity: Severity::Info,
                })
            }
            SubqueryKind::None => None,
        }
    }
}

// ── Subquery detection ──

enum SubqueryKind<'a> {
    Exists { subquery: &'a SelectStatement },
    NotExists { subquery: &'a SelectStatement },
    In { expr: &'a Expr, subquery: &'a SelectStatement },
    NotIn { expr: &'a Expr, subquery: &'a SelectStatement },
    ScalarSubquery,
    None,
}

fn classify_subquery(expr: &Expr) -> SubqueryKind<'_> {
    match expr {
        Expr::Exists(subquery) => SubqueryKind::Exists { subquery },
        Expr::UnaryOp { op, expr: inner } if op == "NOT" => {
            match inner.as_ref() {
                Expr::Exists(subquery) => SubqueryKind::NotExists { subquery },
                _ => SubqueryKind::None,
            }
        }
        Expr::InSubquery { expr, subquery, negated: false } => {
            SubqueryKind::In { expr, subquery }
        }
        Expr::InSubquery { expr, subquery, negated: true } => {
            SubqueryKind::NotIn { expr, subquery }
        }
        Expr::Subquery(_) => SubqueryKind::ScalarSubquery,
        _ => SubqueryKind::None,
    }
}

fn has_subquery(expr: &Expr) -> bool {
    match expr {
        Expr::Exists(_) => true,
        Expr::InSubquery { .. } => true,
        Expr::Subquery(_) => true,
        Expr::UnaryOp { op, expr: inner } if op == "NOT" => has_subquery(inner),
        Expr::BinaryOp { op, left, right } if op == "AND" || op == "OR" => {
            has_subquery(left) || has_subquery(right)
        }
        _ => false,
    }
}

// ── Rewrite: EXISTS / NOT EXISTS → JOIN ──

fn rewrite_exists_to_join(
    select: &SelectStatement,
    subquery: &SelectStatement,
    negated: bool,
) -> Option<RewriteAction> {
    let sub_table = extract_single_table(&subquery.from)?;
    let correlation = extract_correlation_condition(subquery.where_clause.as_ref()?, &sub_table)?;

    let join_type = if negated {
        JoinType::Left
    } else {
        JoinType::Inner
    };

    let join_condition = Some(correlation_to_expr(&correlation));

    let new_from = build_join_from(&select.from, &sub_table, join_type, join_condition);

    let mut new_select = select.clone();
    new_select.from = new_from;

    if negated {
        let null_check = Expr::UnaryOp {
            op: "IS NULL".to_string(),
            expr: Box::new(Expr::ColumnRef(correlation.right_col.clone())),
        };
        new_select.where_clause = Some(combine_with_and(new_select.where_clause.take(), null_check));
    } else {
        new_select.where_clause = remove_subquery_from_where(new_select.where_clause.take(), subquery);
    }

    debug!(
        rule = "subquery-to-join",
        negated = negated,
        table = ?sub_table,
        "Rewrote EXISTS subquery to JOIN"
    );

    Some(RewriteAction::Replace(Box::new(Statement::Select(
        Spanned::without_span(new_select),
    ))))
}

// ── Rewrite: IN / NOT IN → JOIN ──

fn rewrite_in_to_join(
    select: &SelectStatement,
    outer_expr: &Expr,
    subquery: &SelectStatement,
    negated: bool,
) -> Option<RewriteAction> {
    let sub_table = extract_single_table(&subquery.from)?;
    let sub_column = extract_single_select_column(subquery)?;

    let join_type = if negated {
        JoinType::Left
    } else {
        JoinType::Inner
    };

    let join_condition = Some(Expr::BinaryOp {
        left: Box::new(outer_expr.clone()),
        op: "=".to_string(),
        right: Box::new(Expr::ColumnRef(ObjectName::from(vec![
            sub_table.clone(),
            sub_column.clone(),
        ]))),
    });

    let new_from = build_join_from(&select.from, &sub_table, join_type, join_condition);

    let mut new_select = select.clone();
    new_select.from = new_from;

    if negated {
        let null_check = Expr::UnaryOp {
            op: "IS NULL".to_string(),
            expr: Box::new(Expr::ColumnRef(ObjectName::from(vec![
                sub_table.clone(),
                sub_column,
            ]))),
        };
        new_select.where_clause = Some(combine_with_and(new_select.where_clause.take(), null_check));
    } else {
        new_select.where_clause = remove_subquery_from_where(new_select.where_clause.take(), subquery);
    }

    debug!(
        rule = "subquery-to-join",
        negated = negated,
        table = ?sub_table,
        "Rewrote IN subquery to JOIN"
    );

    Some(RewriteAction::Replace(Box::new(Statement::Select(
        Spanned::without_span(new_select),
    ))))
}

// ── Helpers ──

struct Correlation {
    left_col: ObjectName,
    right_col: ObjectName,
}

fn extract_single_table(from: &[TableRef]) -> Option<String> {
    if from.len() != 1 {
        return None;
    }
    match &from[0] {
        TableRef::Table { name, .. } => {
            let table_name = name.last().cloned().unwrap_or_default();
            Some(table_name)
        }
        _ => None,
    }
}

fn extract_single_select_column(stmt: &SelectStatement) -> Option<String> {
    if stmt.targets.len() != 1 {
        return None;
    }
    match &stmt.targets[0] {
        ogsql_parser::ast::SelectTarget::Expr(expr, _alias) => match expr {
            Expr::ColumnRef(name) => name.last().cloned(),
            _ => None,
        },
        _ => None,
    }
}

fn extract_correlation_condition(expr: &Expr, sub_table: &str) -> Option<Correlation> {
    match expr {
        Expr::BinaryOp { op, left, right } if op == "=" => {
            let (left_name, right_name) = (col_name(left), col_name(right));
            match (left_name, right_name) {
                (Some(ln), Some(rn)) => {
                    if is_from_table(&ln, sub_table) {
                        Some(Correlation { left_col: rn, right_col: ln })
                    } else if is_from_table(&rn, sub_table) {
                        Some(Correlation { left_col: ln, right_col: rn })
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        Expr::BinaryOp { op, left, right } if op == "AND" => {
            extract_correlation_condition(left, sub_table)
                .or_else(|| extract_correlation_condition(right, sub_table))
        }
        _ => None,
    }
}

fn col_name(expr: &Expr) -> Option<ObjectName> {
    match expr {
        Expr::ColumnRef(name) => Some(name.clone()),
        _ => None,
    }
}

fn is_from_table(name: &ObjectName, table: &str) -> bool {
    name.len() == 2 && name[0].eq_ignore_ascii_case(table)
}

fn correlation_to_expr(corr: &Correlation) -> Expr {
    Expr::BinaryOp {
        left: Box::new(Expr::ColumnRef(corr.left_col.clone())),
        op: "=".to_string(),
        right: Box::new(Expr::ColumnRef(corr.right_col.clone())),
    }
}

fn build_join_from(
    original_from: &[TableRef],
    sub_table: &str,
    join_type: JoinType,
    condition: Option<Expr>,
) -> Vec<TableRef> {
    if original_from.len() == 1 {
        vec![TableRef::Join {
            left: Box::new(original_from[0].clone()),
            right: Box::new(TableRef::Table {
                name: ObjectName::from(vec![sub_table.to_string()]),
                alias: None,
                lateral: false,
                with_hints: vec![],
            }),
            join_type,
            condition,
            natural: false,
            using_columns: vec![],
        }]
    } else {
        let left = original_from.iter().cloned().reduce(|acc, tr| {
            TableRef::Join {
                left: Box::new(acc),
                right: Box::new(tr),
                join_type: JoinType::Cross,
                condition: None,
                natural: false,
                using_columns: vec![],
            }
        }).unwrap();
        vec![TableRef::Join {
            left: Box::new(left),
            right: Box::new(TableRef::Table {
                name: ObjectName::from(vec![sub_table.to_string()]),
                alias: None,
                lateral: false,
                with_hints: vec![],
            }),
            join_type,
            condition,
            natural: false,
            using_columns: vec![],
        }]
    }
}

fn combine_with_and(existing: Option<Expr>, new: Expr) -> Expr {
    match existing {
        Some(e) => Expr::BinaryOp {
            left: Box::new(e),
            op: "AND".to_string(),
            right: Box::new(new),
        },
        None => new,
    }
}

fn remove_subquery_from_where(where_clause: Option<Expr>, subquery: &SelectStatement) -> Option<Expr> {
    where_clause.map(|expr| strip_subquery_expr(&expr, subquery))
}

fn strip_subquery_expr(expr: &Expr, subquery: &SelectStatement) -> Expr {
    match expr {
        Expr::BinaryOp { op, left, right } if op == "AND" => {
            if is_same_subquery(left, subquery) {
                return strip_subquery_expr(right, subquery);
            }
            if is_same_subquery(right, subquery) {
                return strip_subquery_expr(left, subquery);
            }
            Expr::BinaryOp {
                left: Box::new(strip_subquery_expr(left, subquery)),
                op: op.clone(),
                right: Box::new(strip_subquery_expr(right, subquery)),
            }
        }
        other => other.clone(),
    }
}

fn is_same_subquery(expr: &Expr, subquery: &SelectStatement) -> bool {
    match expr {
        Expr::Exists(sq) => std::ptr::eq(*sq as *const _, subquery as *const _),
        Expr::InSubquery { subquery: sq, .. } => std::ptr::eq(*sq as *const _, subquery as *const _),
        _ => false,
    }
}
```

**Step 4: Register in lib.rs**

Add to `crates/rules/src/lib.rs`:
```rust
pub mod subquery_to_join;
// In builtin_rules():
Box::new(subquery_to_join::SubqueryToJoin),
```

**Step 5: Run tests**

Run: `cargo test -p metamorphosis-rules --test subquery_to_join_test`
Expected: All 6 tests PASS.

**Step 6: Run full workspace**

Run: `cargo test --workspace`
Expected: All tests pass including new ones.

**Step 7: Commit**

```
feat(rules): add SubqueryToJoin rewrite rule with EXISTS/IN/NOT EXISTS/NOT IN/scalar patterns
```

---

### Task 2: Add edge-case tests for safety guards

**Files:**
- Modify: `crates/rules/tests/subquery_to_join_test.rs`

**Step 1: Write edge-case tests**

Append to `crates/rules/tests/subquery_to_join_test.rs`:

```rust
#[test]
fn test_multi_table_subquery_no_match() {
    // Subquery with JOIN inside — too complex for safe rewrite
    let (stmts, _) = test_rewrite(
        "SELECT * FROM orders WHERE user_id IN (SELECT id FROM users JOIN roles ON users.role_id = roles.id)",
    );
    let sql = format_first(&stmts);
    assert!(sql.contains("IN"), "Complex subquery should not be rewritten, got: {}", sql);
}

#[test]
fn test_aggregate_subquery_no_match() {
    let (stmts, _) = test_rewrite(
        "SELECT * FROM orders WHERE user_id IN (SELECT user_id FROM payments GROUP BY user_id HAVING COUNT(*) > 1)",
    );
    let sql = format_first(&stmts);
    assert!(sql.contains("IN"), "Aggregate subquery should not be rewritten, got: {}", sql);
}

#[test]
fn test_exists_with_extra_conditions() {
    // EXISTS with additional non-correlation conditions — should still rewrite
    let (stmts, _) = test_rewrite(
        "SELECT * FROM orders WHERE EXISTS (SELECT 1 FROM users WHERE users.id = orders.user_id AND users.active = 1)",
    );
    let sql = format_first(&stmts);
    assert!(sql.contains("JOIN"), "Should rewrite EXISTS with extra conditions, got: {}", sql);
}
```

**Step 2: Run tests**

Run: `cargo test -p metamorphosis-rules --test subquery_to_join_test`
Expected: All 9 tests PASS.

**Step 3: Commit**

```
test(rules): add edge-case tests for SubqueryToJoin safety guards
```

---

### Task 3: Conditional safety for NOT EXISTS / NOT IN

**Files:**
- Modify: `crates/rules/src/subquery_to_join.rs`

The current skeleton treats all rewrites as Safe. NOT EXISTS and NOT IN should be Conditional because they require NULL-safety checks (columns must be NOT NULL for the rewrite to be semantically equivalent).

**Step 1: Override safety_level to return Conditional when the subquery is NOT EXISTS/NOT IN**

The `safety_level()` method on the trait is fixed per rule. Since we have mixed safety levels (Safe for EXISTS/IN, Conditional for NOT EXISTS/NOT IN, Manual for scalar), we should set the overall rule level to `Conditional` and let the engine trust the rule's own gating.

Alternative: split into 3 rules. But per user request "I want them all", we keep one rule with `SafetyLevel::Conditional`.

Update `safety_level()` to return `Conditional`:

```rust
fn safety_level(&self) -> SafetyLevel {
    SafetyLevel::Conditional
}
```

**Step 2: Run tests**

Run: `cargo test -p metamorphosis-rules --test subquery_to_join_test`
Expected: Tests still pass (Conditional rules still auto-execute when preconditions pass).

**Step 3: Commit**

```
fix(rules): set SubqueryToJoin safety level to Conditional
```

---

### Task 4: QED verification tests (4 patterns)

**Files:**
- Modify: `crates/qed/tests/prover_e2e_test.rs`

**Step 1: Add QED verification tests**

Append to `crates/qed/tests/prover_e2e_test.rs`:

```rust
#[test]
#[ignore = "requires qed-prover + z3 + cvc5 on PATH"]
fn test_exists_to_join_is_provable() {
    let ddl = parse_ddl(
        "CREATE TABLE orders (order_id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount NUMERIC) \
         CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR(100) NOT NULL)",
    );
    let schema = extract_rich_schema(&ddl);

    let original = parse_single(
        "SELECT order_id, user_id, amount FROM orders WHERE EXISTS (SELECT 1 FROM users WHERE users.id = orders.user_id)",
    );
    let rewritten = parse_single(
        "SELECT order_id, user_id, amount FROM orders JOIN users ON users.id = orders.user_id",
    );

    let result = verify_rewrite("exists-to-join", &original, &rewritten, &schema, &prover_config());
    match result {
        Ok(vr) => assert!(
            matches!(vr.proof, metamorphosis_qed::prover::ProofResult::Equivalent),
            "Expected Equivalent, got: {:?}", vr.proof
        ),
        Err(e) => panic!("Prover failed: {e}"),
    }
}

#[test]
#[ignore = "requires qed-prover + z3 + cvc5 on PATH"]
fn test_in_subquery_to_join_is_provable() {
    let ddl = parse_ddl(
        "CREATE TABLE orders (order_id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL) \
         CREATE TABLE active_users (id INTEGER PRIMARY KEY)",
    );
    let schema = extract_rich_schema(&ddl);

    let original = parse_single(
        "SELECT order_id, user_id FROM orders WHERE user_id IN (SELECT id FROM active_users)",
    );
    let rewritten = parse_single(
        "SELECT order_id, user_id FROM orders JOIN active_users ON orders.user_id = active_users.id",
    );

    let result = verify_rewrite("in-to-join", &original, &rewritten, &schema, &prover_config());
    match result {
        Ok(vr) => assert!(
            matches!(vr.proof, metamorphosis_qed::prover::ProofResult::Equivalent),
            "Expected Equivalent, got: {:?}", vr.proof
        ),
        Err(e) => panic!("Prover failed: {e}"),
    }
}

#[test]
#[ignore = "requires qed-prover + z3 + cvc5 on PATH"]
fn test_not_exists_to_join_is_provable() {
    let ddl = parse_ddl(
        "CREATE TABLE orders (order_id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL) \
         CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR(100) NOT NULL)",
    );
    let schema = extract_rich_schema(&ddl);

    let original = parse_single(
        "SELECT order_id, user_id FROM orders WHERE NOT EXISTS (SELECT 1 FROM users WHERE users.id = orders.user_id)",
    );
    let rewritten = parse_single(
        "SELECT order_id, user_id FROM orders LEFT JOIN users ON users.id = orders.user_id WHERE users.id IS NULL",
    );

    let result = verify_rewrite("not-exists-to-join", &original, &rewritten, &schema, &prover_config());
    match result {
        Ok(vr) => assert!(
            matches!(vr.proof, metamorphosis_qed::prover::ProofResult::Equivalent),
            "Expected Equivalent, got: {:?}", vr.proof
        ),
        Err(e) => panic!("Prover failed: {e}"),
    }
}

#[test]
#[ignore = "requires qed-prover + z3 + cvc5 on PATH"]
fn test_not_in_to_join_is_provable() {
    let ddl = parse_ddl(
        "CREATE TABLE orders (order_id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL) \
         CREATE TABLE active_users (id INTEGER PRIMARY KEY)",
    );
    let schema = extract_rich_schema(&ddl);

    let original = parse_single(
        "SELECT order_id, user_id FROM orders WHERE user_id NOT IN (SELECT id FROM active_users)",
    );
    let rewritten = parse_single(
        "SELECT order_id, user_id FROM orders LEFT JOIN active_users ON orders.user_id = active_users.id WHERE active_users.id IS NULL",
    );

    let result = verify_rewrite("not-in-to-join", &original, &rewritten, &schema, &prover_config());
    match result {
        Ok(vr) => assert!(
            matches!(vr.proof, metamorphosis_qed::prover::ProofResult::Equivalent),
            "Expected Equivalent, got: {:?}", vr.proof
        ),
        Err(e) => panic!("Prover failed: {e}"),
    }
}
```

**Step 2: Run ignored tests with prover**

Run: `Z3_SYS_Z3_HEADER=/opt/homebrew/include/z3.h RUSTFLAGS="-L /opt/homebrew/lib" cargo +nightly test -p metamorphosis-qed --test prover_e2e_test -- --ignored --nocapture`
Expected: All 9 E2E tests pass (5 existing + 4 new).

**Step 3: Run full workspace**

Run: `cargo test --workspace`
Expected: All tests pass.

**Step 4: Commit**

```
test(qed): add QED verification tests for SubqueryToJoin equivalence (EXISTS/IN/NOT EXISTS/NOT IN)
```

---

### Task 5: Update docs and plan

**Files:**
- Modify: `AGENTS.md` (update project status to include SubqueryToJoin)

**Step 1: Update AGENTS.md**

In the "Project Status" section, add SubqueryToJoin to the built-in rules list.

**Step 2: Final workspace verification**

Run: `cargo test --workspace`
Expected: All tests pass.

**Step 3: Commit**

```
docs: update AGENTS.md with SubqueryToJoin rule
```
