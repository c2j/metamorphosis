# Plan: extract-candidate-values Rule Correctness Fixes

**Status**: Draft, pending review
**Author**: Sisyphus
**Created**: 2026-06-17
**Affects**: `crates/rules/src/eq_analyzer.rs`, `crates/rules/src/extract_candidate_values.rs`
**Out of scope**: `(+)` outer-join marker preservation (tracked separately in ogsql-parser)

## 1. Context & Motivation

The `extract-candidate-values` rule generates a probe SQL to show existing values of parameterized WHERE equality columns. Rule's stated purpose (rule docstring lines 19-23):

> When a SQL query uses `WHERE col = :param` and the input parameter value does not exist in the data, the query returns nothing. This rule generates a probe to show what values *do* exist (filtered by non-parameterized conditions), enabling the user to find a valid input value.

Analysis of four test cases (`testcases/case6.sql`, `case6-1.sql`, `case6-2.sql`, `case6-3.sql`) revealed **three correctness defects** that prevent the probe from fulfilling its purpose.

## 2. Scope

### In Scope
- P0: Parameterized `(p IS NULL OR col = p)` OR pattern not filtered from probe WHERE → defeats probe purpose
- P1: Duplicate predicate emitted for `(col = expr OR expr IS NULL)` pattern
- P2: EXISTS subquery probe with correlated reference gets `High` confidence despite being non-runnable standalone

### Out of Scope
- `(+)` outer-join marker loss (filed as separate issue in ogsql-parser)
- LIKE-based parameter handling (`temp.bank_name LIKE '%' || p || '%'`) — by design, only equality is probed
- Refactoring `eq_analyzer.rs` beyond minimal fixes
- Any API surface changes

## 3. Root Causes (Code-Level)

### RC-1 (P0): Inconsistent parameter recognition

**`classify_column_pair`** (`crates/rules/src/eq_analyzer.rs:74-88`) treats an unqualified ColumnRef whose name isn't a known table alias as a parameter:
```rust
fn is_known_table_or_correlated(&self, parts: &[Ident]) -> bool {
    self.is_known_table(parts) || parts.len() > 1   // single-part + unknown alias → false (i.e., parameter)
}
```
→ `col = p` (where `p` is a stored-proc variable) correctly classifies `col` into `tier1`.

**`contains_param`** (`eq_analyzer.rs:556-599`) only recognizes explicit Parameter AST nodes:
```rust
Expr::Parameter(_) | Expr::MyBatisParam(_) | Expr::MyBatisRawExpr(_) | Expr::JdbcParam => true,
```
→ Returns `false` for stored-proc variables (which parse as plain `ColumnRef`).

**Impact**: `non_param_exprs()` (`eq_analyzer.rs:145-151`) fails to filter expressions containing stored-proc variables. The `(p IS NULL OR col = p)` clause stays in probe WHERE while `col` is in GROUP BY → probe collapses to a single row.

### RC-2 (P1): Contract violation in `extract_eq_from_non_and`

Docstring (`eq_analyzer.rs:229-231`) says:
> Extract equality sub-expressions from non-AND, non-= operators (like OR) for tier1 analysis, **without modifying the non_eq/keep_exprs collections**.

But the function calls `col.handle_equality(left, right)` (L238), whose catch-all branch (L137-138) pushes to `non_eq`:
```rust
(Expr::ColumnRef(_), _) | (_, Expr::ColumnRef(_)) => {
    self.non_eq.push(make_binary_eq(left, right));
}
```

**Impact**: For `(col = decode(...) OR decode(...) IS NULL)`, both the bare equality `col = decode(...)` AND the full OR are pushed to `non_eq`. Both pass `contains_param` (no params) → both emitted → duplicate predicate in probe.

### RC-3 (P2): Confidence doesn't account for correlated refs

`has_subquery` (`eq_analyzer.rs:25`, set at L209-211) only tracks subqueries in current scope's WHERE. It doesn't track correlated references (`v.account_code = temp.asset_acnt_id` where `temp` is outer-scope).

**Impact**: The EXISTS subquery probe keeps the correlation predicate (correct preservation), but the probe is not runnable standalone (`temp` not in probe FROM). Confidence is `High` despite this.

## 4. Design Decisions (User-Confirmed)

1. **Param-name tracking**: Record ColumnRef names identified as parameters in `EqPredicateCollector` during classification. Use this set in `non_param_exprs()` filtering. Rationale: reuses verified logic in `classify_column_pair`, avoids duplicating parameter-detection rules in two places.
2. **Whole `(OR)` expression removal**: When OR contains a parameter equality, the entire OR expression is removed from probe WHERE (not just the equality part). Rationale: aligns with probe's "show all candidate values" semantics.
3. **Correlated-ref confidence downgrade**: Keep correlation predicate in probe WHERE; downgrade confidence to `Medium` when correlated refs are present. Rationale: preserves semantic context while signaling non-runnable probe.

## 5. Change Sets

### Change Set A — Track classified param names (P0)

**File**: `crates/rules/src/eq_analyzer.rs`

A1. Add field to `EqPredicateCollector` (after L27):
```rust
/// Single-part ColumnRef names identified as parameters during classification.
/// Populated by `handle_equality` when the opposing side of a ColumnRef=ColumnRef
/// equality is classified as a parameter (single-part, not a known table alias).
param_names: HashSet<String>,
```

A2. Initialize in `EqPredicateCollector::new` (L49-56):
```rust
param_names: HashSet::new(),
```

A3. Populate in `handle_equality` ColumnRef=ColumnRef branch (L115-129):
```rust
(Expr::ColumnRef(l_parts), Expr::ColumnRef(r_parts)) => {
    let (l_is_table, r_is_table) = self.classify_column_pair(l_parts, r_parts);
    match (l_is_table, r_is_table) {
        (true, false) => {
            self.tier1.push(l_parts.clone());
            if let Some(name) = r_parts.last() {
                self.param_names.insert(name.as_str().to_string());
            }
        }
        (false, true) => {
            self.tier1.push(r_parts.clone());
            if let Some(name) = l_parts.last() {
                self.param_names.insert(name.as_str().to_string());
            }
        }
        _ => {
            self.keep_exprs.push(make_binary_eq(left, right));
        }
    }
}
```

A4. Add helper method `references_classified_param`:
```rust
/// True if `expr` contains any `ColumnRef` whose last identifier matches a name
/// in `self.param_names`. Used to filter non-equality expressions that
/// reference stored-proc variables not represented as `Expr::Parameter`.
fn references_classified_param(&self, expr: &Expr) -> bool {
    let names = &self.param_names;
    if names.is_empty() {
        return false;
    }
    walk_column_refs(expr, &|parts| {
        parts.last().is_some_and(|p| names.contains(p.as_str()))
    })
}
```
Where `walk_column_refs` is a new module-level recursive walker over `Expr::ColumnRef` nodes (handles BinaryOp, Parenthesized, FunctionCall args, Case, Between, IsNull, etc.).

A5. Add `contains_classified_param` method:
```rust
pub(crate) fn contains_classified_param(&self, expr: &Expr) -> bool {
    contains_param(expr) || self.references_classified_param(expr)
}
```

A6. Refactor `non_param_exprs` (L145-151):
```rust
pub(crate) fn non_param_exprs(&self) -> Vec<Expr> {
    self.non_eq
        .iter()
        .filter(|e| !self.contains_classified_param(e))
        .cloned()
        .collect()
}
```

### Change Set B — Fix `extract_eq_from_non_and` contract violation (P1)

**File**: `crates/rules/src/eq_analyzer.rs`

B1. Add new method `classify_for_tier1_only` that does tier1 + param_names classification **without** touching non_eq/keep_exprs:
```rust
/// Tier1-only classifier used by `extract_eq_from_non_and`.
/// Pushes to `tier1` and `param_names` only; never touches `non_eq`/`keep_exprs`.
/// This honors the contract documented on `extract_eq_from_non_and`.
fn classify_for_tier1_only(&mut self, left: &Expr, right: &Expr) {
    match (left, right) {
        (Expr::ColumnRef(name), Expr::Parameter(_) | Expr::MyBatisParam(_) | Expr::MyBatisRawExpr(_) | Expr::JdbcParam)
        | (Expr::Parameter(_) | Expr::MyBatisParam(_) | Expr::MyBatisRawExpr(_) | Expr::JdbcParam, Expr::ColumnRef(name)) => {
            self.tier1.push(name.clone());
        }
        (Expr::ColumnRef(l_parts), Expr::ColumnRef(r_parts)) => {
            let (l_is_table, r_is_table) = self.classify_column_pair(l_parts, r_parts);
            match (l_is_table, r_is_table) {
                (true, false) => {
                    self.tier1.push(l_parts.clone());
                    if let Some(n) = r_parts.last() { self.param_names.insert(n.as_str().to_string()); }
                }
                (false, true) => {
                    self.tier1.push(r_parts.clone());
                    if let Some(n) = l_parts.last() { self.param_names.insert(n.as_str().to_string()); }
                }
                _ => {}  // join condition: do NOT push to keep_exprs here
            }
        }
        _ => {}  // deliberately no-op for non-tier1 cases
    }
}
```
Note: this duplicates a subset of `handle_equality`. Acceptable trade-off — alternative (boolean flag on `handle_equality`) would couple two contracts in one function. The duplication is bounded and well-commented.

B2. Update `extract_eq_from_non_and` (L232-251) to call new method:
```rust
pub(crate) fn extract_eq_from_non_and(expr: &Expr, col: &mut EqPredicateCollector) {
    match expr {
        Expr::BinaryOp { left, op, right } => {
            match op.to_uppercase().as_str() {
                "=" => col.classify_for_tier1_only(left, right),  // ← was: col.handle_equality(left, right)
                _ => {
                    extract_eq_from_non_and(left, col);
                    extract_eq_from_non_and(right, col);
                }
            }
        }
        Expr::Parenthesized(inner) => extract_eq_from_non_and(inner, col),
        _ => {}
    }
}
```

### Change Set C — Correlated-ref confidence downgrade (P2)

**File**: `crates/rules/src/eq_analyzer.rs`

C1. Add field to `EqPredicateCollector`:
```rust
/// True if any classified ColumnRef=ColumnRef equality referenced a column
/// whose qualifier is multi-part but unknown in the current scope's FROM
/// (i.e., a correlated reference to an outer query).
has_correlated_ref: bool,
```

C2. Initialize in `new`: `has_correlated_ref: false,`

C3. Set in `handle_equality` ColumnRef=ColumnRef else branch (L125-128, where both sides are "known or correlated"):
```rust
_ => {
    self.keep_exprs.push(make_binary_eq(left, right));
    // Detect correlated refs: either side has multi-part name whose prefix
    // is NOT in current scope's table_aliases.
    let l_correlated = l_parts.len() > 1 && !self.is_known_table(l_parts);
    let r_correlated = r_parts.len() > 1 && !self.is_known_table(r_parts);
    if l_correlated || r_correlated {
        self.has_correlated_ref = true;
    }
}
```

**File**: `crates/rules/src/extract_candidate_values.rs`

C4. Update confidence logic (L138-142):
```rust
confidence: if collector.has_subquery || collector.has_correlated_ref {
    Confidence::Medium
} else {
    Confidence::High
},
```

## 6. Test Plan

**File**: `crates/rules/tests/extract_candidate_values_test.rs`

### New Test Cases (TDD — write first, watch fail, implement, watch pass)

**T1: `test_or_is_null_pattern_filtered_from_probe_whole`** (P0)
- Input: `SELECT * FROM t WHERE t.a = '1' AND (p_x IS NULL OR t.b = p_x)`
- Expected: probe WHERE contains only `t.a = '1'`; GROUP BY `t.b`; no reference to `p_x` anywhere in probe.
- Failure mode before fix: probe WHERE still contains `(p_x IS NULL OR t.b = p_x)`.

**T2: `test_or_eq_is_null_no_duplicate_predicate`** (P1)
- Input: `SELECT * FROM t WHERE t.a = '1' AND (t.b = decode(t.a, '1', '0', t.b) OR decode(t.a, '1', '0', t.b) IS NULL)`
- Expected: probe WHERE contains `t.a = '1'` exactly once and the OR expression exactly once; the bare `t.b = decode(...)` must NOT appear standalone.
- Failure mode before fix: bare equality appears as duplicate predicate.

**T3: `test_correlated_ref_in_exists_probe_downgrades_confidence`** (P2)
- Input: `SELECT * FROM a WHERE EXISTS (SELECT 1 FROM b v WHERE v.code = a.code AND v.user = p_u)`
- Expected: main probe = `Medium` (has_subquery); EXISTS subquery probe = `Medium` (correlated ref `a.code`); EXISTS probe WHERE retains `v.code = a.code`.
- Failure mode before fix: EXISTS probe = `High`.

**T4: `test_same_scope_join_keeps_high_confidence`** (P2 regression guard)
- Input: `SELECT * FROM a, b WHERE a.id = b.id AND a.status = p_s`
- Expected: confidence = `High` (both `a.id` and `b.id` are in scope aliases → not correlated).

**T5: `test_explicit_parameter_in_or_pattern_still_filtered`** (P0 regression guard)
- Input: `SELECT * FROM t WHERE (:p IS NULL OR t.b = :p)` (JDBC `?` style)
- Expected: same as T1 — OR filtered from probe WHERE; GROUP BY `t.b`.

### Regression — Existing Tests
- Run full `cargo test -p metamorphosis-rules` suite (existing 750 lines of tests).
- Specifically verify these still pass:
  - `test_comma_join_condition_excluded_from_group_by`
  - `test_correlated_ref_in_subquery_no_probe`
  - `test_update_with_in_subquery`
  - `test_two_in_subqueries`

### Integration — Manual Probe Verification
Re-run the four testcases and assert expected changes:

| Testcase | Pre-fix behavior | Post-fix expectation |
|---|---|---|
| `case6.sql` | Duplicate `t.operation_status = decode(...)` predicate | Single OR predicate, no duplicate |
| `case6-1.sql` | Same as case6 | Same fix |
| `case6-2.sql` main probe | 7 `(p_i_qry_xxx IS NULL OR ...)` predicates preserved | All 7 OR predicates removed; GROUP BY unchanged |
| `case6-2.sql` EXISTS probe | Confidence `High` | Confidence `Medium`; correlation predicate retained |
| `case6-3.sql` | Same as case6-2 | Same fixes |

## 7. Implementation Order

1. **Phase 1 — TDD tests**: Write T1-T5 (they should fail). Commit isolated.
2. **Phase 2 — Change Set A**: Implement param-name tracking. Verify T1, T5 pass.
3. **Phase 3 — Change Set B**: Implement `classify_for_tier1_only`. Verify T2 passes; verify T1 still passes (A+B together fully fix P0).
4. **Phase 4 — Change Set C**: Implement correlated-ref confidence. Verify T3, T4 pass.
5. **Phase 5 — Full regression**: `cargo test --workspace`; manual probe re-run on case6*.sql.
6. **Phase 6 — LSP diagnostics**: `lsp_diagnostics` clean on both changed files.

Each phase is independently committable. If a phase regresses, revert just that phase.

## 8. Verification Commands

```bash
# Unit tests
cargo test -p metamorphosis-rules

# Full workspace
cargo test --workspace

# Manual probe runs (compare against expected table in §6)
./target/release/metamorphosis suggest --rules extract-candidate-values --file ./testcases/case6.sql
./target/release/metamorphosis suggest --rules extract-candidate-values --file ./testcases/case6-1.sql
./target/release/metamorphosis suggest --rules extract-candidate-values --file ./testcases/case6-2.sql
./target/release/metamorphosis suggest --rules extract-candidate-values --file ./testcases/case6-3.sql

# LSP diagnostics
# (via lsp_diagnostics tool on both changed files)
```

## 9. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `walk_column_refs` walker misses an `Expr` variant → false negative in param detection | Medium | Medium (under-filtering) | Mirror the variant list from `contains_param`; add unit test for each variant |
| `classify_for_tier1_only` diverges from `handle_equality` over time | Medium | Low (drift) | Add doc comment cross-referencing the two; add unit test asserting both produce same tier1 for shared inputs |
| Correlated-ref heuristic (`len > 1 && !is_known_table`) false-positives on CTE references | Low | Low (over-downgrade to Medium) | Acceptable; safe direction. Add test for CTE case. |
| Whole-OR-removal removes a `(p IS NULL)` test that user wanted | Low | Low (semantic shift) | Document in rule description; user can fall back to inspecting original SQL |

## 10. Success Criteria

The plan is complete when ALL of the following hold:

1. ✅ T1-T5 new tests pass
2. ✅ All pre-existing tests in `extract_candidate_values_test.rs` pass unchanged
3. ✅ `cargo test --workspace` exits 0
4. ✅ Manual probe runs on `case6.sql`, `case6-1.sql`, `case6-2.sql`, `case6-3.sql` match the expected-output table in §6
5. ✅ `lsp_diagnostics` returns 0 errors and 0 warnings on both changed files
6. ✅ No `as any` / `@ts-ignore` / `unwrap()` introduced (per AGENTS.md coding standards)
7. ✅ Every new `pub` item has a doc comment (per AGENTS.md)

## 11. Open Questions for Reviewer (Momus)

1. Is the duplication between `handle_equality` and `classify_for_tier1_only` acceptable, or should we instead add a `mode: Enum { Full, Tier1Only }` parameter to `handle_equality`?
2. Should `walk_column_refs` be `pub(crate)` for reuse by other rules, or private to this module?
3. Is the correlated-ref heuristic (multi-part prefix + not in aliases) precise enough, or should we maintain an explicit `outer_scope_aliases` set?
4. For T5 (explicit `:p` parameter in OR), is the whole-OR-removal still correct, or should the `(:p IS NULL)` portion be preserved as a literal filter (it can never be true at probe time)?
