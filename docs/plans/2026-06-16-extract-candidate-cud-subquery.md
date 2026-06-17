# Extract Candidate Values — CUD + Subquery Multi-Probe Support

> **For implementation:** Use subagent-driven-development to execute this plan task-by-task.

**Goal:** Extend `extract-candidate-values` and `detect-duplicate-eq-keys` rules to handle UPDATE/DELETE/INSERT/MERGE statements with subqueries, generating one probe per distinct query scope (outer + each subquery's WHERE).

**Architecture:** Three-layer change: (1) `RewriteRule::apply()` returns `Vec<RewriteAction>` instead of `Option<RewriteAction>`, enabling multi-probe output. (2) `eq_analyzer` adds `extract_query_scopes()` to find all (FROM, WHERE) pairs — outer DML + subqueries inside WHERE. (3) Both rules iterate scopes, run `collect_eq_predicates` per scope, and output one probe per scope with tier1 candidates.

**Tech Stack:** Rust, ogsql-parser AST, metamorphosis-core/types

---

## Task 1: Change `RewriteRule::apply()` return type

**Files:**
- Modify: `crates/core/src/registry.rs:35`
- Modify: `crates/core/src/engine.rs:80-137`
- Modify: `crates/rules/src/extract_candidate_values.rs:71`
- Modify: `crates/rules/src/detect_duplicate_eq_keys.rs:66`
- Modify: `crates/rules/src/subquery_to_join.rs:98`
- Modify: `crates/rules/src/eliminate_select_star.rs:58`

### Step 1: Change trait signature

In `crates/core/src/registry.rs`, change line 35:

```rust
// Before:
fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Option<RewriteAction>;

// After:
fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Vec<RewriteAction>;
```

Add a doc comment for the return type:
```
/// Execute the rewrite. Returns zero or more actions.
/// Multiple actions enable one statement to produce multiple probes
/// (e.g., subqueries in CUD WHERE clauses each generate their own probe).
```

### Step 2: Update engine dispatch

In `crates/core/src/engine.rs`:

**Auto rules block** (line 80-92): Change from `if let Some(RewriteAction::Replace(new_stmt)) = rule.apply(...)` to iterating:

```rust
for rule in &auto_rules {
    match rule.matches(ctx, &stmt) {
        MatchResult::Matched => {
            for action in rule.apply(ctx, &stmt) {
                if let RewriteAction::Replace(new_stmt) = action {
                    if validate_statement(&new_stmt) {
                        stmt = *new_stmt;
                        iteration_changed = true;
                        changed = true;
                        break;
                    }
                } else {
                    // Non-Replace actions from auto rules become suggestions
                    suggestions.push(Suggestion { ... });
                }
            }
            if iteration_changed { break; }
        }
        // ...
    }
}
```

**Manual rules block** (line 117-137): Change from `if let Some(action) = rule.apply(...)` to iterating:

```rust
for rule in &manual_rules {
    match rule.matches(ctx, &stmt) {
        MatchResult::Matched => {
            for action in rule.apply(ctx, &stmt) {
                suggestions.push(Suggestion {
                    rule_id: rule.id().to_string(),
                    rule_description: rule.description().to_string(),
                    action,
                    confidence: Confidence::High,
                    notes: Vec::new(),
                });
            }
        }
        // ...
    }
}
```

### Step 3: Update all 4 existing rule implementations

Each rule's `apply()` currently returns `Some(RewriteAction::...)`. Change to `vec![RewriteAction::...]`.

**`eliminate_select_star.rs:58`**: `Some(RewriteAction::Replace(...))` → `vec![RewriteAction::Replace(...)]`

**`subquery_to_join.rs:98`**: Four `Some(RewriteAction::Replace(...))` → `vec![RewriteAction::Replace(...)]` each

**`detect_duplicate_eq_keys.rs:66`**: `Some(RewriteAction::Generate{...})` → `vec![RewriteAction::Generate{...}]`

**`extract_candidate_values.rs:71`**: `Some(RewriteAction::Generate{...})` → `vec![RewriteAction::Generate{...}]` (temporary, will be expanded in Task 3)

### Step 4: Build and verify

```bash
cargo build --workspace
cargo test --workspace
```

All existing tests must pass with the new API.

---

## Task 2: Add `extract_query_scopes()` to `eq_analyzer`

**Files:**
- Modify: `crates/rules/src/eq_analyzer.rs`

### Step 1: Add `QueryScope` struct

At the top of `eq_analyzer.rs`, add:

```rust
/// A distinct query scope: a FROM + WHERE pair extracted from a statement
/// or its subqueries. Each scope can produce an independent probe.
#[derive(Debug, Clone)]
pub(crate) struct QueryScope {
    /// Table references for the probe's FROM clause.
    pub from: Vec<TableRef>,
    /// WHERE clause to analyze for parameterized equalities.
    pub where_clause: Option<Expr>,
    /// Optional label for debugging (e.g., "outer WHERE", "IN subquery on items").
    pub label: Option<String>,
}
```

Export from `crates/rules/src/lib.rs` if needed by consumers (it's `pub(crate)` for now).

### Step 2: Add `extract_query_scopes()` function

```rust
/// Extract all query scopes from a WHERE clause by walking the expression tree.
/// Returns one scope for the outer context plus one scope for each subquery
/// (IN/EXISTS/scalar subquery) found in the WHERE clause.
///
/// Subqueries that reference CTE names in their FROM are skipped —
/// the CTE definition itself will be probed as a separate scope.
pub(crate) fn extract_query_scopes(
    from: &[TableRef],
    where_clause: &Option<Expr>,
) -> Vec<QueryScope> {
    let mut scopes = vec![QueryScope {
        from: from.to_vec(),
        where_clause: where_clause.clone(),
        label: None,
    }];
    if let Some(ref expr) = where_clause {
        let cte_names = HashSet::new(); // No CTEs in direct WHERE walk
        walk_subquery_scopes(expr, &mut scopes, &cte_names);
    }
    scopes
}

/// Walk an expression tree, pushing QueryScope for each subquery variant found.
fn walk_subquery_scopes(expr: &Expr, scopes: &mut Vec<QueryScope>, cte_names: &HashSet<String>) {
    match expr {
        Expr::BinaryOp { left, right, .. } => {
            walk_subquery_scopes(left, scopes, cte_names);
            walk_subquery_scopes(right, scopes, cte_names);
        }
        Expr::UnaryOp { expr: inner, .. } => {
            walk_subquery_scopes(inner, scopes, cte_names);
        }
        Expr::Parenthesized(inner) => {
            walk_subquery_scopes(inner, scopes, cte_names);
        }
        Expr::Exists(inner) | Expr::Subquery(inner) => {
            push_subquery_scope(inner, scopes, cte_names);
        }
        Expr::InSubquery { subquery, .. } => {
            push_subquery_scope(subquery, scopes, cte_names);
        }
        Expr::ScalarSublink { subquery, .. } => {
            push_subquery_scope(subquery, scopes, cte_names);
        }
        _ => {}
    }
}

/// Push a subquery's scope if its FROM doesn't reference CTE names.
/// Then recurse into the subquery's own WHERE for nested subqueries.
fn push_subquery_scope(
    select: &SelectStatement,
    scopes: &mut Vec<QueryScope>,
    cte_names: &HashSet<String>,
) {
    // Only add as scope if FROM has real tables (no CTE references)
    let has_cte_ref = select.from.iter().any(|tr| references_cte(tr, cte_names));
    if !has_cte_ref {
        scopes.push(QueryScope {
            from: select.from.clone(),
            where_clause: select.where_clause.clone(),
            label: None,
        });
    }
    // Recurse into subquery's own WHERE for nested subqueries
    if let Some(ref wc) = select.where_clause {
        walk_subquery_scopes(wc, scopes, cte_names);
    }
}

/// Check if a TableRef references a CTE name.
fn references_cte(tr: &TableRef, cte_names: &HashSet<String>) -> bool {
    match tr {
        TableRef::Table { name, .. } => {
            name.last().is_some_and(|n| cte_names.contains(n.as_str()))
        }
        TableRef::Join { left, right, .. } => {
            references_cte(left, cte_names) || references_cte(right, cte_names)
        }
        _ => false,
    }
}
```

Requirements:
- Use `use ogsql_parser::ast::{Expr, SelectStatement, TableRef};` (already imported)
- Use `use std::collections::HashSet;` (already imported)

### Step 3: Build and verify

```bash
cargo build -p metamorphosis-rules
```

Must compile. No test changes yet — that's Task 5.

---

## Task 3: Enhance `extract_candidate_values` for DML + multi-probe

**Files:**
- Modify: `crates/rules/src/extract_candidate_values.rs`

### Step 1: Add `extract_statement_scopes()` helper

Add a function that extracts `(from, where_clause, cte_names)` from any statement type:

```rust
use ogsql_parser::ast::{
    Expr, GroupByItem, InsertSource, Literal, ObjectName, OrderByItem, 
    SelectStatement, SelectTarget, Spanned, Statement, TableRef,
};

/// Extract all query scopes from any DML statement.
/// Returns scopes for: outer WHERE + each subquery in WHERE + each CTE definition.
fn extract_statement_scopes(stmt: &Statement) -> Vec<eq_analyzer::QueryScope> {
    match stmt {
        Statement::Select(s) => {
            let (wc, from) = eq_analyzer::resolve_query(&s.node);
            let mut scopes = eq_analyzer::extract_query_scopes(from, wc);
            // Add CTE scopes
            if let Some(ref with_clause) = s.node.with {
                for cte in &with_clause.ctes {
                    let cte_scope = eq_analyzer::QueryScope {
                        from: cte.query.from.clone(),
                        where_clause: cte.query.where_clause.clone(),
                        label: Some(format!("CTE {}", cte.name)),
                    };
                    scopes.push(cte_scope);
                    // Recurse into CTE's WHERE for subqueries
                    if let Some(ref wc) = cte.query.where_clause {
                        let mut cte_scopes = eq_analyzer::extract_query_scopes(
                            &cte.query.from, wc
                        );
                        // Don't duplicate the CTE's own outer scope (already added)
                        if !cte_scopes.is_empty() {
                            cte_scopes.remove(0);
                        }
                        scopes.extend(cte_scopes);
                    }
                }
            }
            scopes
        }
        Statement::Update(s) => {
            let mut from = s.tables.clone();
            from.extend(s.from.clone());
            let wc = &s.where_clause;
            let mut scopes = eq_analyzer::extract_query_scopes(&from, wc);
            if let Some(ref with_clause) = s.with {
                for cte in &with_clause.ctes {
                    scopes.push(eq_analyzer::QueryScope {
                        from: cte.query.from.clone(),
                        where_clause: cte.query.where_clause.clone(),
                        label: Some(format!("CTE {}", cte.name)),
                    });
                }
            }
            scopes
        }
        Statement::Delete(s) => {
            let mut from = s.tables.clone();
            from.extend(s.using.clone());
            let wc = &s.where_clause;
            let mut scopes = eq_analyzer::extract_query_scopes(&from, wc);
            if let Some(ref with_clause) = s.with {
                for cte in &with_clause.ctes {
                    scopes.push(eq_analyzer::QueryScope {
                        from: cte.query.from.clone(),
                        where_clause: cte.query.where_clause.clone(),
                        label: Some(format!("CTE {}", cte.name)),
                    });
                }
            }
            scopes
        }
        Statement::Insert(s) => {
            let mut scopes = Vec::new();
            // INSERT ... SELECT → analyze the SELECT
            if let InsertSource::Select(ref sel) = s.source {
                let (wc, from) = eq_analyzer::resolve_query(sel);
                scopes = eq_analyzer::extract_query_scopes(from, wc);
            }
            if let Some(ref with_clause) = s.with {
                for cte in &with_clause.ctes {
                    scopes.push(eq_analyzer::QueryScope {
                        from: cte.query.from.clone(),
                        where_clause: cte.query.where_clause.clone(),
                        label: Some(format!("CTE {}", cte.name)),
                    });
                }
            }
            scopes
        }
        Statement::Merge(s) => {
            // Merge: use target table + ON condition + WHEN branch WHEREs
            // For simplicity: treat ON condition as WHERE for target table
            let mut scopes = vec![eq_analyzer::QueryScope {
                from: vec![s.target.clone()],
                where_clause: Some(s.on_condition.clone()),
                label: Some("MERGE target".to_string()),
            }];
            // Also check each WHEN clause's WHERE for subqueries
            for when in &s.when_clauses {
                if let Some(ref wc) = when.where_clause {
                    let mut when_scopes = eq_analyzer::extract_query_scopes(&[], wc);
                    scopes.extend(when_scopes);
                }
            }
            scopes
        }
        _ => vec![],
    }
}
```

### Step 2: Rewrite `matches()` — accept any DML

```rust
fn matches(&self, ctx: &RewriteContext, stmt: &Statement) -> MatchResult {
    let scopes = extract_statement_scopes(stmt);
    if scopes.is_empty() {
        return MatchResult::NotMatched {
            reason: "Statement has no analyzable query scopes".to_string(),
        };
    }

    let mut has_tier1 = false;
    for scope in &scopes {
        let collector = eq_analyzer::collect_eq_predicates(
            &scope.where_clause,
            &scope.from,
            ctx.known_variables,
        );
        if !collector.tier1.is_empty() {
            has_tier1 = true;
            break;
        }
    }

    if has_tier1 {
        MatchResult::Matched
    } else {
        MatchResult::NotMatched {
            reason: "No parameterized equality conditions (col = :param) found in any query scope"
                .to_string(),
        }
    }
}
```

### Step 3: Rewrite `apply()` — generate one probe per scope

```rust
fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Vec<RewriteAction> {
    let scopes = extract_statement_scopes(stmt);
    let mut actions = Vec::new();

    for scope in &scopes {
        let collector = eq_analyzer::collect_eq_predicates(
            &scope.where_clause,
            &scope.from,
            ctx.known_variables,
        );

        let mut seen = HashSet::new();
        let mut group_cols: Vec<ObjectName> = Vec::new();
        for col_name in collector.tier1.iter() {
            let key = col_name.last().map(|i| i.as_str().to_string()).unwrap_or_default();
            if seen.insert(key) {
                group_cols.push(col_name.clone());
            }
        }

        if group_cols.is_empty() {
            continue;
        }

        let limit = ctx.config.probe_default_limit;
        let non_param = collector.non_param_exprs();
        let probe = build_candidate_probe_statement(
            &scope.from,
            &collector.keep_exprs,
            &non_param,
            &group_cols,
            limit,
        );

        let purpose = if group_cols.len() == 1 {
            let display = group_cols[0].join(".");
            format!("Candidate value extraction: show existing values for column '{}'", display)
        } else {
            let displays: Vec<String> = group_cols.iter().map(|c| c.join(".")).collect();
            format!(
                "Candidate value extraction: show existing value combinations for columns [{}]",
                displays.join(", ")
            )
        };

        actions.push(RewriteAction::Generate {
            stmt: Box::new(Statement::Select(probe)),
            purpose,
            confidence: if collector.has_subquery {
                Confidence::Medium
            } else {
                Confidence::High
            },
        });
    }

    actions
}
```

### Step 4: Build and verify

```bash
cargo build -p metamorphosis-rules
```

Must compile.

---

## Task 4: Enhance `detect_duplicate_eq_keys` for DML + multi-probe

**Files:**
- Modify: `crates/rules/src/detect_duplicate_eq_keys.rs`

### Step 1: Add `extract_statement_scopes()` helper (same pattern as Task 3)

Reuse the exact same function structure but adapted for `detect_duplicate_eq_keys`. The key difference: this rule requires ≥2 tier1 columns per scope (not ≥1).

### Step 2: Rewrite `matches()` 

Same pattern as extract_candidate_values, but check `collector.tier1.len() >= 2` per scope.

### Step 3: Rewrite `apply()` 

Same pattern — iterate scopes, build probe per scope with ≥2 tier1 columns.

### Step 4: Build and verify

```bash
cargo build --workspace
cargo test --workspace
```

All existing tests must pass.

---

## Task 5: Add comprehensive tests

**Files:**
- Modify: `crates/rules/tests/extract_candidate_values_test.rs`
- Modify: `crates/rules/tests/detect_duplicate_eq_keys_test.rs`

### Step 1: Add test utility for checking multi-probe output

```rust
/// Assert that N suggestions are generated from the given SQL.
fn assert_suggestion_count(sql: &str, expected_count: usize) -> Vec<Suggestion> {
    let (_statements, suggestions) = test_suggest(sql);
    assert_eq!(
        suggestions.len(),
        expected_count,
        "Expected {} suggestion(s) for SQL: {}",
        expected_count,
        sql
    );
    suggestions
}
```

### Step 2: New test cases for `extract_candidate_values_test.rs`

```rust
// ── CUD + subquery tests ──

#[test]
fn test_update_with_in_subquery() {
    let suggestions = assert_suggestion_count(
        "UPDATE orders SET status = 'done' WHERE order_id IN (SELECT order_id FROM items WHERE category = v_cat)",
        2, // outer: order_id (param via ColumnRef=ColumnRef), inner: category
    );
    // Verify inner subquery probe references items table and category column
    let probes: Vec<String> = suggestions.iter()
        .filter_map(format_probe)
        .collect();
    let has_items_probe = probes.iter().any(|p| p.contains("items") && p.contains("category"));
    assert!(has_items_probe, "Expected a probe from items subquery, got: {:?}", probes);
}

#[test]
fn test_delete_with_exists_subquery() {
    let (_statements, suggestions) = test_suggest(
        "DELETE FROM orders WHERE EXISTS (SELECT 1 FROM items WHERE category = v_cat AND region = 'EAST')",
    );
    assert!(!suggestions.is_empty(), "Should match EXISTS subquery");
    let probe = format_probe(&suggestions).expect("Expected probe");
    assert!(probe.contains("category"), "Probe must reference category: {}", probe);
    assert!(probe.contains("region"), "Probe must retain region filter: {}", probe);
}

#[test]
fn test_insert_select() {
    let (_statements, suggestions) = test_suggest(
        "INSERT INTO archive (id, name) SELECT id, name FROM orders WHERE status = v_status",
    );
    assert!(!suggestions.is_empty(), "Should match INSERT...SELECT");
    let probe = format_probe(&suggestions).expect("Expected probe");
    assert!(probe.contains("status"), "Probe must reference status: {}", probe);
    assert!(probe.contains("orders"), "Probe must reference orders: {}", probe);
}

#[test]
fn test_update_no_subquery_single_probe() {
    let suggestions = assert_suggestion_count(
        "UPDATE t SET x = 1 WHERE col = v_col AND region = 'EAST'",
        1,
    );
    let probe = format_probe(&suggestions).expect("Expected 1 probe");
    assert!(probe.contains("col"), "Probe must reference col: {}", probe);
    assert!(probe.contains("region"), "Probe must retain region: {}", probe);
}

#[test]
fn test_two_in_subqueries_three_probes() {
    let suggestions = assert_suggestion_count(
        "SELECT * FROM t WHERE a IN (SELECT x FROM t1 WHERE y = v_y) AND b IN (SELECT z FROM t2 WHERE w = v_w)",
        3, // outer (a,b), inner1 (y), inner2 (w)
    );
    let probes: Vec<String> = suggestions.iter()
        .filter_map(format_probe)
        .collect();
    assert!(probes.iter().any(|p| p.contains("t1") && p.contains("y")), "Missing t1 probe");
    assert!(probes.iter().any(|p| p.contains("t2") && p.contains("w")), "Missing t2 probe");
}

#[test]
fn test_update_plain_no_subquery() {
    let suggestions = assert_suggestion_count(
        "UPDATE orders SET status = v_new WHERE status = v_old AND region = 'EAST'",
        1,
    );
    let probe = format_probe(&suggestions).expect("Expected 1 probe");
    assert!(probe.contains("status"), "Probe must reference status: {}", probe);
}

#[test]
fn test_insert_values_no_probe() {
    let (_statements, suggestions) = test_suggest(
        "INSERT INTO t (a, b) VALUES (1, 2)",
    );
    assert!(suggestions.is_empty(), "INSERT...VALUES should not match");
}

#[test]
fn test_merge_on_condition() {
    let (_statements, suggestions) = test_suggest(
        "MERGE INTO target t USING source s ON t.id = s.id AND t.status = v_status WHEN MATCHED THEN UPDATE SET t.x = 1",
    );
    assert!(!suggestions.is_empty(), "Should match MERGE ON condition");
}

// Nested subquery: 2 levels deep
#[test]
fn test_nested_subquery() {
    let (_statements, suggestions) = test_suggest(
        "SELECT * FROM t WHERE col IN (SELECT a FROM t1 WHERE b IN (SELECT c FROM t2 WHERE d = v_d))",
    );
    // Should produce probes for: outer col, inner1 b, inner2 d
    assert!(suggestions.len() >= 2, "Expected at least 2 probes from nested subqueries, got {}", suggestions.len());
    let probes: Vec<String> = suggestions.iter()
        .filter_map(format_probe)
        .collect();
    assert!(probes.iter().any(|p| p.contains("t2") && p.contains("d")), "Missing innermost probe");
}
```

### Step 3: Add corresponding tests for `detect_duplicate_eq_keys_test.rs`

Similar structure but checking for ≥2 tier1 columns.

### Step 4: Run tests

```bash
cargo test -p metamorphosis-rules
```

All new tests must pass. All existing tests must still pass.

---

## Task 6: Final verification — build, test, clippy

**Files:** None (verification only)

### Step 1: Full build

```bash
cargo build --workspace
```

### Step 2: Full test suite

```bash
cargo test --workspace
```

### Step 3: Clippy

```bash
cargo clippy --workspace -- -D warnings
```

### Step 4: Fix any warnings or clippy issues

### Step 5: Commit

```bash
git add -A
git commit -m "feat: multi-probe CUD + subquery support for extract-candidate-values and detect-duplicate-eq-keys"
```
