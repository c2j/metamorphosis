# Phase B: QED Prover Integration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build and run the real qed-prover binary to verify SQL query equivalence end-to-end, including an IR adapter layer to bridge our format to the prover's native JSON.

**Architecture:** Add a `prover_compat` module in `crates/qed/` that converts our `QedInput` (tagged-enum IR) into the prover's native JSON format (untagged variant-as-key). Then update `prover.rs` to parse the actual prover output format ("provable"/"not provable" + `.result` JSON files). Finally, build the prover from source and run a real equivalence proof.

**Tech Stack:** Rust, qed-prover (Rust nightly + Z3 + CVC5), serde_json, tempfile

**Prerequisites (manual, network required):**
- `brew install z3 cvc5` (SMT solvers)
- `rustup toolchain install nightly` (already available)
- `git clone https://github.com/qed-solver/prover.git` into a known location
- `cd prover && cargo +nightly build --release`

---

## Known Format Differences

Based on librarian research of `github.com/qed-solver/prover`:

### Schema

| Field | Our Format | Prover Format |
|-------|-----------|---------------|
| key | `[0]` (flat Vec<usize>) | `[[0]]` (Vec<Vec<usize>>, composite keys) |
| name | `"users"` | `"CATALOG.SALES.DEPT"` (qualified) |
| Other fields | Same | Same |

### Relations (the big difference)

| Our Format (tagged) | Prover Format (variant-as-key) |
|---------------------|-------------------------------|
| `{"type":"Scan","table":"R","fields":[0,1]}` | `{"scan": 0}` (integer index into schemas) |
| `{"type":"Filter","condition":{...},"input":{...}}` | `{"filter": {"source": {...}, "condition": {...}}}` |
| `{"type":"Project","exprs":[...],"input":{...}}` | `{"project": {"source": {...}, "target": [...]}}` |
| `{"type":"Join","left":{...},"right":{...},"condition":{...}}` | `{"join": {"left": {...}, "right": {...}, "condition": {...}}}` |
| `{"type":"Union",...}` | `{"union": {...}}` |
| `{"type":"Aggregate","keys":[0],"aggs":[...],"input":{...}}` | `{"aggregate": {"source": {...}, "keys": [...], "aggs": [...]}}` |
| `{"type":"Distinct","input":{...}}` | `{"distinct": {"source": {...}}}` |

### Help

| Our Format | Prover Format |
|-----------|---------------|
| `"single string"` | `["plan1", "plan2"]` (array) |

### Output

| Our Parsing | Prover Actual Output |
|-------------|---------------------|
| Keyword "equivalent" in stdout | stdout: `"provable"` or `"not provable"` |
| Keyword "not equivalent" | stdout: `"not provable"` |
| `.result` file not parsed | `.result` JSON: `{"provable": bool, "smt_timed_out": bool, ...}` |

---

## Task Breakdown

### Task 1: Add Prover-native IR types (`prover_compat.rs`)

**Files:**
- Create: `crates/qed/src/prover_compat.rs`
- Test: inline `#[cfg(test)]` module

**Step 1: Write failing tests for conversion**

Create `crates/qed/src/prover_compat.rs` with test skeletons:

```rust
//! Adapter layer: converts our internal QedInput IR into the qed-prover's
//! native JSON format.
//!
//! The prover uses an untagged enum representation where each relation variant
//! is keyed by its lowercase name (e.g., `{"scan": 0}`, `{"filter": {...}}`),
//! while our IR uses serde's `#[serde(tag = "type")]` tagged format.

use crate::ir::{QedInput, QedRelation, QedExpr, QedAggCall, QedAggArg, QedSchema, QedValue};
use serde::{Deserialize, Serialize};

// --- Prover-native types ---

/// Prover-native schema. Key difference: `key` is `Vec<Vec<usize>>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProverSchema {
    pub name: String,
    pub types: Vec<String>,
    pub nullable: Vec<bool>,
    pub key: Vec<Vec<usize>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub guaranteed: Vec<String>,
    pub fields: Vec<String>,
}

/// Prover-native input. Key differences: `help` is `Vec<String>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProverInput {
    pub schemas: Vec<ProverSchema>,
    pub queries: [ProverRelation; 2],
    pub help: Vec<String>,
}

// ... ProverRelation, ProverExpr etc. as untagged enums

// --- Conversion functions ---

pub fn convert_input(our: &QedInput, schema_name_map: &[String]) -> ProverInput {
    todo!()
}

pub fn convert_relation(rel: &QedRelation, schema_map: &[String]) -> ProverRelation {
    todo!()
}

pub fn convert_expr(expr: &QedExpr) -> ProverExpr {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_scan() {
        // Our Scan with table name -> prover's {"scan": schema_index}
        todo!()
    }

    #[test]
    fn test_convert_filter() {
        // Our Filter -> prover's {"filter": {"source": ..., "condition": ...}}
        todo!()
    }

    #[test]
    fn test_convert_project() {
        // Our Project -> prover's {"project": {"source": ..., "target": ...}}
        todo!()
    }

    #[test]
    fn test_convert_schema_key() {
        // Our [0] -> prover's [[0]]
        todo!()
    }

    #[test]
    fn test_convert_help() {
        // Our "string" -> prover's ["string", "string"]
        todo!()
    }

    #[test]
    fn test_roundtrip_simple_query() {
        // Build a QedInput, convert to ProverInput, serialize to JSON,
        // verify it matches expected prover format structure
        todo!()
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p metamorphosis-qed prover_compat`
Expected: compile errors (todo!() panics)

**Step 3: Implement conversion types and functions**

Define `ProverRelation` and `ProverExpr` as untagged enums matching the prover format. Implement `From`/conversion for all our IR types. Key mappings:
- `QedRelation::Scan { table, fields }` → look up table index in schemas → `ProverRelation::Scan { scan: usize }`
- `QedRelation::Filter { condition, input }` → `ProverRelation::Filter { filter: FilterBody { source, condition } }`
- `QedRelation::Project { exprs, input }` → `ProverRelation::Project { project: ProjectBody { source, target } }`
- Schema.key: `vec![0]` → `vec![vec![0]]`
- help: `"desc"` → `vec!["query1".to_string(), "query2".to_string()]`

**IMPORTANT:** The exact ProverRelation/ProverExpr types MUST be determined by reading actual test JSON files from the prover repo. The struct definitions above are approximate. The implementing agent MUST clone the prover repo and read:
- `tests/calcite/*.json` (any 3 files)
- `src/pipeline.rs` or `src/pipeline/mod.rs` (Input struct)
- `src/shared.rs` or equivalent (Schema struct)
- `src/main.rs` (CLI entry)

**Step 4: Run tests to verify they pass**

Run: `cargo test -p metamorphosis-qed prover_compat`
Expected: All 6+ tests pass

**Step 5: Commit**

```bash
git add crates/qed/src/prover_compat.rs crates/qed/src/lib.rs
git commit -m "feat(qed): add prover-native IR adapter layer"
```

---

### Task 2: Update prover output parsing

**Files:**
- Modify: `crates/qed/src/prover.rs` (update `parse_prover_output`)
- Test: existing tests in `prover.rs` + new tests

**Step 1: Write failing tests for actual prover output**

Add tests to `prover.rs` test module:

```rust
#[test]
fn test_parse_provable() {
    let output = make_output("provable\n", "", 0);
    let result = parse_prover_output(&output).unwrap();
    assert_eq!(result, ProofResult::Equivalent);
}

#[test]
fn test_parse_not_provable() {
    let output = make_output("not provable\n", "", 0);
    let result = parse_prover_output(&output).unwrap();
    assert!(matches!(result, ProofResult::NotEquivalent { .. }));
}

#[test]
fn test_parse_timed_out_prover() {
    // Prover may output "timed out" or similar
    let output = make_output("unknown\n", "smt timed out\n", 0);
    let result = parse_prover_output(&output).unwrap();
    assert!(matches!(result, ProofResult::Unknown { .. }));
}
```

**Step 2: Run tests to see failures**

Run: `cargo test -p metamorphosis-qed prover::tests`
Expected: Some existing tests still pass, new ones may fail on keywords

**Step 3: Update `parse_prover_output` to handle both formats**

Add "provable" / "not provable" keywords alongside existing "equivalent" / "not equivalent":

```rust
fn parse_prover_output(output: &std::process::Output) -> Result<ProofResult, ProverError> {
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let stdout_lower = stdout.to_lowercase();

    // Prover format: "provable" / "not provable"
    // Legacy format: "equivalent" / "not equivalent"
    let is_provable = stdout_lower.contains("provable") && !stdout_lower.contains("not provable");
    let is_not_provable = stdout_lower.contains("not provable");
    let is_equivalent = stdout_lower.contains("equivalent")
        && !stdout_lower.contains("not equivalent")
        && !stdout_lower.contains("notequivalent");
    let is_not_equivalent = stdout_lower.contains("notequivalent")
        || stdout_lower.contains("not equivalent");
    let is_unknown = stdout_lower.contains("unknown")
        || stdout_lower.contains("timed out");

    if is_provable || is_equivalent {
        Ok(ProofResult::Equivalent)
    } else if is_not_provable || is_not_equivalent {
        Ok(ProofResult::NotEquivalent {
            counterexample: extract_counterexample(&stdout),
        })
    } else if is_unknown {
        Ok(ProofResult::Unknown {
            reason: extract_reason(&stdout, &stderr),
        })
    } else if output.status.success() {
        Ok(ProofResult::Equivalent)
    } else {
        Err(ProverError::UnexpectedOutput { stdout, stderr })
    }
}
```

**Step 4: Run all prover tests**

Run: `cargo test -p metamorphosis-qed prover::tests`
Expected: All pass

**Step 5: Commit**

```bash
git add crates/qed/src/prover.rs
git commit -m "fix(qed): update prover output parsing for 'provable'/'not provable' format"
```

---

### Task 3: Update `run_prover` to use native format via adapter

**Files:**
- Modify: `crates/qed/src/prover.rs` (`run_prover` function)
- Modify: `crates/qed/src/verify.rs`
- Test: update integration tests

**Step 1: Write failing test**

The `run_prover` function should accept a flag to use native format:

```rust
#[test]
fn test_run_prover_uses_native_format() {
    // Build a QedInput, run through conversion, check the temp file
    // contains native format, not tagged format
    todo!()
}
```

**Step 2: Modify `run_prover`**

Update `run_prover` to:
1. Call `prover_compat::convert_input()` before serialization
2. Serialize `ProverInput` (not `QedInput`) to the temp file
3. Optionally also parse `.result` file if present

**Step 3: Update `verify.rs`**

Update `verify_rewrite()` to pass schema info needed for table-name→index mapping.

**Step 4: Run all tests**

Run: `cargo test -p metamorphosis-qed`
Expected: All pass (existing 87 + new tests)

**Step 5: Commit**

```bash
git add crates/qed/src/prover.rs crates/qed/src/verify.rs crates/qed/src/prover_compat.rs
git commit -m "feat(qed): wire prover_compat adapter into run_prover pipeline"
```

---

### Task 4: Build qed-prover from source (manual prerequisite)

**This task requires network access.**

**Step 1: Install SMT solvers**

```bash
brew install z3 cvc5
```

**Step 2: Clone and build prover**

```bash
git clone https://github.com/qed-solver/prover.git /tmp/qed-prover
cd /tmp/qed-prover
cargo +nightly build --release
```

The binary will be at `/tmp/qed-prover/target/release/qed-prover`.

**Step 3: Verify prover works**

```bash
# Run on one of the bundled test cases
/tmp/qed-prover/target/release/qed-prover tests/calcite/
```

Expected: Output showing "provable" / "not provable" for each test.

**Step 4: Record binary path**

Note the path for integration test configuration.

---

### Task 5: End-to-end equivalence proof

**Files:**
- Add: `crates/qed/tests/prover_e2e_test.rs` (new, `#[ignore]` by default)
- Modify: `crates/qed/src/verify.rs` (if adjustments needed)

**Step 1: Write the E2E test**

```rust
//! Real prover E2E test. Requires qed-prover binary on PATH.
//! Run with: cargo test -p metamorphosis-qed --test prover_e2e_test -- --ignored

#[test]
#[ignore = "requires qed-prover binary + Z3 + CVC5 on PATH"]
fn test_select_star_equivalence() {
    // Original: SELECT * FROM users
    // Rewritten: SELECT id, name, email FROM users
    // Schema: CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR(100) NOT NULL, email VARCHAR(200))
    // Expected: Provable (equivalent)
}

#[test]
#[ignore = "requires qed-prover binary + Z3 + CVC5 on PATH"]
fn test_non_equivalent_queries() {
    // Original: SELECT id, name FROM users
    // Rewritten: SELECT id, email FROM users
    // Expected: Not provable (NOT equivalent — different columns)
}

#[test]
#[ignore = "requires qed-prover binary + Z3 + CVC5 on PATH"]
fn test_identity_query() {
    // Original: SELECT id FROM users WHERE id > 0
    // Rewritten: SELECT id FROM users WHERE id > 0
    // Expected: Provable (trivially equivalent)
}
```

**Step 2: Generate test JSON and validate manually**

Before running through the harness, dump a `ProverInput` JSON to a temp file and run the prover on it directly:

```bash
# Run the test that writes the JSON file
cargo test -p metamorphosis-qed test_write_prover_input_json

# Run prover on it
qed-prover /tmp/test_qed_input.json

# Check output
```

**Step 3: Run full E2E test**

```bash
PATH="/tmp/qed-prover/target/release:$PATH" \
cargo test -p metamorphosis-qed --test prover_e2e_test -- --ignored
```

**Step 4: Fix any format mismatches**

If the prover rejects our JSON, read the error message, study the test files, and adjust `prover_compat.rs`.

**Step 5: Commit**

```bash
git add crates/qed/tests/prover_e2e_test.rs
git commit -m "test(qed): add real prover E2E equivalence proof tests"
```

---

### Task 6: Update `parse_prover_output` to also parse `.result` files

**Files:**
- Modify: `crates/qed/src/prover.rs`

**Step 1: Add `.result` file parsing**

After the prover finishes, if a `.result` file exists next to the input file, parse it for structured data:

```rust
#[derive(Deserialize)]
struct ProverResult {
    provable: bool,
    smt_timed_out: bool,
    complete_fragment: bool,
    panicked: bool,
}
```

**Step 2: Integrate into `run_prover`**

If `.result` file exists, use it as the authoritative answer. Fall back to stdout parsing otherwise.

**Step 3: Test**

**Step 4: Commit**

```bash
git add crates/qed/src/prover.rs
git commit -m "feat(qed): parse prover .result files for structured output"
```

---

### Task 7: Update CI scripts and documentation

**Files:**
- Modify: `scripts/install-qed-prover.sh`
- Modify: `.github/workflows/qed-verify.yml`
- Modify: `docs/QED.md` (add prover format documentation section)

**Step 1: Update install script**

Update `scripts/install-qed-prover.sh` to:
- Install Z3 and CVC5 via apt/brew
- Clone and build qed-prover from source
- Place binary in a known location

**Step 2: Update CI workflow**

Ensure the workflow installs all dependencies and runs the `#[ignore]` tests.

**Step 3: Document prover format**

Add a section to `docs/QED.md` documenting the prover's JSON format and the adapter layer.

**Step 4: Commit**

```bash
git add scripts/ .github/ docs/QED.md
git commit -m "docs(qed): update CI scripts and prover format documentation"
```

---

## Summary

| Task | Description | Dependencies |
|------|-------------|-------------|
| 1 | Prover-native IR adapter (`prover_compat.rs`) | None (code only) |
| 2 | Update output parsing ("provable"/"not provable") | None |
| 3 | Wire adapter into `run_prover` pipeline | Task 1 |
| 4 | Build qed-prover from source | **Network required** (brew, git clone) |
| 5 | Real E2E equivalence proof | Tasks 1-4 |
| 6 | Parse `.result` files | Task 5 (needs real output to validate) |
| 7 | CI + docs update | Tasks 1-6 |

**Tasks 1-3 can be done without network access** (pure code changes, tested with mocked output).
**Tasks 4-5 require network** to install Z3/CVC5 and clone the prover repo.
**Tasks 6-7** are polish after the pipeline works.

**Estimated effort:** Tasks 1-3 = ~2 hours coding. Tasks 4-5 = ~1 hour setup + debugging. Tasks 6-7 = ~30 min.
