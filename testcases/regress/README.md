# Regression Test Cases

Data-driven regression tests for Metamorphosis built-in rules.

Each rule has a dedicated subdirectory (named after the rule's Rust module,
e.g. `nvl_to_case`). The Rust harness at `crates/rules/tests/regression_test.rs`
auto-discovers all cases and runs them through the real engine — no Rust code
changes are needed when adding new cases.

## Directory Layout

```
testcases/regress/
├── README.md                          (this file)
├── nvl_to_case/
│   ├── README.md                      rule-level description
│   ├── pos-001-basic.input.sql        positive case — input
│   ├── pos-001-basic.expected.sql     positive case — fragment assertions
│   ├── pos-001-basic.full.sql         positive case — complete expected output
│   ├── neg-001-no-nvl.input.sql       negative case — input
│   ├── neg-001-no-nvl.full.sql        negative case — unchanged output
│   └── ...
├── between_to_eq/
│   └── ...
└── probe_null_ratio/
    └── ...
```

## File Naming Convention

| Suffix | Required | Purpose |
|--------|----------|---------|
| `.input.sql` | Always | SQL fed into the engine |
| `.expected.sql` | Positive cases | Fragment assertions (must-contain / must-not-contain) |
| `.full.sql` | All cases (auto-generated) | Complete expected output for exact normalised comparison |

Case-name prefix:

- `pos-` — **positive**: the rule should match and produce output
- `neg-` — **negative**: the rule should NOT match (statement unchanged / no suggestion generated)

Negative cases do not need `.expected.sql`. If one is provided, its fragments
are still checked against the (unchanged) output — useful for asserting
forbidden keywords, e.g. `!CASE`.

## Expected-SQL Format

`.expected.sql` supports two assertion modes: **fragment mode** (default)
and **exact mode**. Both share the same conventions for comments and
forbidden fragments.

### Common Conventions

- Lines starting with `--` are comments (ignored).
- Lines starting with `!` are **forbidden fragments** — they must NOT appear
  in the output. Works in both modes.
- Matching is **case-insensitive** and **whitespace-normalised**
  (consecutive whitespace collapses to a single space).

### Fragment Mode (default)

Each non-comment, non-`!` line is a **fragment** that MUST appear somewhere
in the engine output. Use this for resilient, semantically-focused checks.

```sql
-- Probe must include COUNT and per-column aliases
COUNT
total
col1_non_null
!NVL
```

### Exact Mode

Add `-- @exact` as a marker line. All non-comment, non-`!` lines are then
**joined and compared as one normalised string** against the full engine
output. Use this when you need to verify the complete SQL, not just
fragments.

```sql
-- @exact
SELECT CASE WHEN col IS NULL THEN 0 ELSE col END FROM t
```

`!` forbidden-fragment checks still apply in exact mode — useful for
catching leftover keywords alongside a full comparison:

```sql
-- @exact
SELECT CASE WHEN col IS NULL THEN 0 ELSE col END FROM t
!NVL
```

## Rule-Level Context Files

Some rules require additional context (schema, known variables) to match.
Place these files in the rule directory — the harness auto-loads them:

| File | Purpose | Used by |
|------|---------|---------|
| `_schema.json` | Table → column → type map | `eliminate_select_star` |
| `_variables.txt` | Stored-proc variable names (one per line, `#` = comment) | `detect_duplicate_eq_keys`, `extract_candidate_values` |

Example `_schema.json`:
```json
{"users": {"id": "integer", "name": "varchar"}}
```

Example `_variables.txt`:
```
# Stored-proc variables recognised as parameters by eq_analyzer
v_user_id
v_status
```

Directories without these files work unchanged (context = `None`).

## Rule-Type Auto-Detection

The harness reads `rule.safety_level()` to determine assertion semantics:

| Safety Level | Positive Case | Negative Case |
|--------------|---------------|---------------|
| Safe / Conditional | checks `result.statements` (rewritten SQL) | asserts `result.changed == false` |
| Manual | checks `result.suggestions` (probe SQL) | asserts `suggestions.is_empty()` |

## Full-Match Verification (`.full.sql`)

Every case has a `.full.sql` file containing the **complete expected engine
output**. The harness normalises whitespace + case and compares the entire
output string — catching errors that fragment checks might miss (e.g. a rule
accidentally dropping a clause).

These files are auto-generated, not hand-written:

```bash
REGEN_FULL=1 cargo test -p metamorphosis-rules --test regression_test
```

Re-run this whenever a rule's output format changes. Review the git diff
before committing.

Negative probe-rule cases (Manual + `neg-`) have no `.full.sql` — they
produce no output to compare.

## Running Tests

```bash
# All regression cases (all rules)
cargo test -p metamorphosis-rules --test regression_test -- --nocapture

# Single rule via REGRESS_RULE env var (accepts dir name or rule ID)
REGRESS_RULE=nvl_to_case    cargo test -p metamorphosis-rules --test regression_test -- --nocapture
REGRESS_RULE=between-to-eq  cargo test -p metamorphosis-rules --test regression_test -- --nocapture

# Re-generate all .full.sql files (after rule output format changes)
REGEN_FULL=1 cargo test -p metamorphosis-rules --test regression_test
```

`REGRESS_RULE` accepts both forms:

- **Directory name** (snake_case): `nvl_to_case`, `probe_null_ratio`
- **Rule ID** (kebab-case): `nvl-to-case`, `probe-null-ratio`

If the value matches no directory, the test fails with a clear message.

## Adding New Cases

1. Create or open `<rule_module_name>/` under this directory.
2. For a positive case, add `pos-NNN-description.input.sql` **and**
   `pos-NNN-description.expected.sql`.
3. For a negative case, add `neg-NNN-description.input.sql` (expected file
   optional).
4. Run `REGRESS_FULL=1 cargo test -p metamorphosis-rules --test regression_test`
   to auto-generate `.full.sql` files for the new cases.
5. Review the generated files, then run without `REGEN_FULL` to verify.

**Zero Rust code changes required.** The harness resolves rule directories
to rules via `metamorphosis_rules::builtin_rules()` and ID matching
(`nvl_to_case` directory → `nvl-to-case` rule ID).
