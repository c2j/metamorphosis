# ExtractCandidateValues Rule Implementation Plan

> **For Claude:** Sub-agent driven development.

**Goal:** Implement a Manual-level rule `extract-candidate-values` that generates GROUP BY probe SQL to show all existing values of parameterized WHERE equality columns, enabling users to find valid input values for query parameters.

**Architecture:** New rule in `crates/rules/src/extract_candidate_values.rs`, sharing predicate analysis logic with `DetectDuplicateEqKeys` via a new shared `eq_analyzer` module. The rule generates a `RewriteAction::Generate` probe SQL with pattern: `SELECT param_cols, count(1) AS cnt FROM tables WHERE non_param_conditions GROUP BY param_cols ORDER BY cnt DESC`.

**Tech Stack:** Rust, ogsql-parser AST, existing Metamorphosis engine infrastructure.

**Design decisions:**
- V1 scope: WHERE-clause only (not JOIN ON). See test plan for full coverage.
- Multiple parameterized columns → GROUP BY all of them (composite grouping).
- OR-wrapped parameter patterns (`p IS NULL OR col = p`) → V1 known limitation, skip.

---

### Task 1: Extract shared `eq_analyzer` module

**Files:**
- Create: `crates/rules/src/eq_analyzer.rs`
- Modify: `crates/rules/src/detect_duplicate_eq_keys.rs` (use shared module)

**Step 1: Create shared module**

Extract from `detect_duplicate_eq_keys.rs` into `eq_analyzer.rs`:
- `EqPredicateCollector` struct + impl (all methods)
- `collect_eq_predicates` function
- `collect_from` function
- `extract_eq_from_non_and` function
- `make_binary_eq` function
- `collect_table_aliases_recursive` function
- `resolve_query` function (on DetectDuplicateEqKeys)

All become `pub` with `pub(crate)` visibility.

**Step 2: Refactor DetectDuplicateEqKeys**

Replace inline definitions with `use metamorphosis_rules::eq_analyzer::*`.
Add `use super::eq_analyzer;` path.

Keep only rule-specific code in `detect_duplicate_eq_keys.rs`:
- `DetectDuplicateEqKeys` struct + `RewriteRule` impl
- `build_probe_statement` function (unique to this rule)

**Step 3: Build and test**

Run: `cargo test -p metamorphosis-rules --test detect_duplicate_eq_keys_test`
Expected: All existing tests pass (no behavior change).

---

### Task 2: Implement `ExtractCandidateValues` rule

**Files:**
- Create: `crates/rules/src/extract_candidate_values.rs`

**Step 1: Implement rule struct and RewriteRule trait**

```rust
pub struct ExtractCandidateValues;

impl RewriteRule for ExtractCandidateValues {
    fn id(&self) -> &'static str { "extract-candidate-values" }
    fn description(&self) -> &'static str { "Generate probe SQL showing existing values of parameterized WHERE equality columns" }
    fn category(&self) -> RuleCategory { RuleCategory::DataQuality }
    fn safety_level(&self) -> SafetyLevel { SafetyLevel::Manual }
    fn matches(&self, ctx: &RewriteContext, stmt: &Statement) -> bool { ... }
    fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Option<RewriteAction> { ... }
}
```

**Step 2: `matches` logic**
- Resolve subquery wrapper via `resolve_query`
- Collect equality predicates via `collect_eq_predicates`
- Return `collector.tier1.len() >= 1` (at least one parameterized equality)
- No schema needed (unlike EliminateSelectStar)

**Step 3: `apply` logic**
- Same resolution + collection
- Build probe SQL with:
  - SELECT tier1 columns + `count(1) AS cnt`
  - FROM from resolved query (preserved as-is)
  - WHERE = merge_exprs(keep_exprs, non_eq) — same as DetectDuplicateEqKeys
  - GROUP BY tier1 columns
  - No HAVING
  - ORDER BY cnt DESC
  - LIMIT = config.probe_default_limit

**Step 4: Build and test**

Run: `cargo build -p metamorphosis-rules`
Expected: Compiles without error.

---

### Task 3: Register the new rule

**Files:**
- Modify: `crates/rules/src/lib.rs`

**Step 1: Add module declaration and register**

```rust
pub mod extract_candidate_values;

pub fn builtin_rules() -> Vec<Box<dyn RewriteRule>> {
    vec![
        Box::new(eliminate_select_star::EliminateSelectStar),
        Box::new(detect_duplicate_eq_keys::DetectDuplicateEqKeys),
        Box::new(subquery_to_join::SubqueryToJoin),
        Box::new(extract_candidate_values::ExtractCandidateValues),
    ]
}
```

---

### Task 4: Write tests

**Files:**
- Create: `crates/rules/tests/extract_candidate_values_test.rs`

**Test cases (from analysis):**

| # | Test | Input | Key Assertion |
|---|------|-------|--------------|
| 1 | simple_literal_and_param | `SELECT * FROM t WHERE t.clear_type = '4' AND t.task_status = p_status` | GROUP BY t.task_status, WHERE has t.clear_type = '4' |
| 2 | param_only_no_literal | `SELECT * FROM users WHERE users.id = v_id` | GROUP BY users.id, no WHERE |
| 3 | mybatis_param | `SELECT name FROM users WHERE users.status = #{status}` | Same as #1, MyBatisParam variant |
| 4 | multiple_params | `SELECT * FROM t WHERE t.col1 = v_a AND t.col2 = v_b` | GROUP BY t.col1, t.col2 (composite) |
| 5 | param_with_is_null | `SELECT * FROM t WHERE t.flag IS NULL AND t.s = p_s` | WHERE preserves IS NULL |
| 6 | param_with_or | `SELECT * FROM t WHERE (t.a = '4' OR t.a = '5') AND t.b = v_b` | WHERE preserves OR, GROUP BY t.b |
| 7 | subquery_wrapper | pagination wrapper pattern | resolve_query unwraps, correct inner FROM |
| 8 | join_with_param | `SELECT ... FROM t1 JOIN t2 ON ... WHERE t1.s = v_s AND t1.x > 10` | FROM preserved, GROUP BY t1.s |
| 9 | no_param_no_match | `SELECT * FROM users WHERE id = 1` | suggestions empty |
| 10 | no_where_no_match | `SELECT * FROM users` | suggestions empty |
| 11 | single_eq_colref_colref_no_match | `SELECT * FROM t1 JOIN t2 ON t1.id = t2.id` | suggestions empty |

---

### Task 5: Build, test, verify

**Step 1: Build workspace**
```bash
cargo build --workspace 2>&1
```
Expected: Clean build, no errors or warnings.

**Step 2: Run all tests**
```bash
cargo test --workspace 2>&1
```
Expected: All new tests pass, all existing tests still pass.

**Step 3: CLI integration test**
```bash
# Create test SQL file
echo "SELECT t.special_sql FROM dat_dataclear_config t WHERE t.clear_type = '4' AND t.task_status = p_i_taskstatus" > /tmp/test_candidates.sql

# Run suggest
cargo run -- suggest /tmp/test_candidates.sql 2>&1
```
Expected: Probe SQL output with GROUP BY task_status.
