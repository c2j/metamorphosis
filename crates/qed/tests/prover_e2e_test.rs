//! Real qed-prover E2E equivalence proof tests.
//!
//! These tests invoke the actual `qed-prover` binary (built from
//! <https://github.com/qed-solver/prover>) to prove SQL query equivalence.
//!
//! **Prerequisites** (all must be on PATH):
//! - `qed-prover` binary (built with `cargo +nightly build --release`)
//! - `z3` SMT solver
//! - `cvc5` SMT solver
//!
//! Run with:
//! ```sh
//! cargo test -p metamorphosis-qed --test prover_e2e_test -- --ignored
//! ```

use metamorphosis_qed::prover::ProverConfig;
use metamorphosis_qed::schema::extract_rich_schema;
use metamorphosis_qed::verify::{verify_rewrite, verify_rewrite_with_names};
use ogsql_parser::ast::Statement;
use ogsql_parser::Parser;
use std::collections::HashMap;
use std::path::PathBuf;

fn parse_ddl(sql: &str) -> Vec<Statement> {
    let (stmts, _) = Parser::parse_sql(sql);
    stmts.into_iter().map(|si| si.statement).collect()
}

fn parse_single(sql: &str) -> Statement {
    let (stmts, _) = Parser::parse_sql(sql);
    stmts.into_iter().next().expect("expected one statement").statement
}

fn test_schema_ddl() -> &'static str {
    "CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR(100) NOT NULL, email VARCHAR(200))"
}

fn prover_config() -> ProverConfig {
    ProverConfig {
        binary_path: PathBuf::from("qed-prover"),
        timeout_secs: 120,
        workdir: None,
    }
}

#[test]
#[ignore = "requires qed-prover + z3 + cvc5 on PATH"]
fn test_identity_query_is_provable() {
    let ddl = parse_ddl(test_schema_ddl());
    let schema = extract_rich_schema(&ddl);
    let original = parse_single("SELECT id, name FROM users WHERE id = 1");
    let rewritten = parse_single("SELECT id, name FROM users WHERE id = 1");

    let result = verify_rewrite("identity", &original, &rewritten, &schema, &prover_config());

    match result {
        Ok(vr) => {
            assert!(
                matches!(vr.proof, metamorphosis_qed::prover::ProofResult::Equivalent),
                "Expected Equivalent, got: {:?}",
                vr.proof
            );
        }
        Err(e) => {
            panic!("Prover invocation failed: {e}");
        }
    }
}

#[test]
#[ignore = "requires qed-prover + z3 + cvc5 on PATH"]
fn test_select_star_expansion_is_provable() {
    let ddl = parse_ddl(test_schema_ddl());
    let schema = extract_rich_schema(&ddl);
    let original = parse_single("SELECT * FROM users");
    let rewritten = parse_single("SELECT id, name, email FROM users");

    let result = verify_rewrite(
        "eliminate-select-star",
        &original,
        &rewritten,
        &schema,
        &prover_config(),
    );

    match result {
        Ok(vr) => {
            assert!(
                matches!(vr.proof, metamorphosis_qed::prover::ProofResult::Equivalent),
                "Expected Equivalent for SELECT * expansion, got: {:?}",
                vr.proof
            );
        }
        Err(e) => {
            panic!("Prover invocation failed: {e}");
        }
    }
}

#[test]
#[ignore = "requires qed-prover + z3 + cvc5 on PATH"]
fn test_different_columns_is_not_provable() {
    let ddl = parse_ddl(test_schema_ddl());
    let schema = extract_rich_schema(&ddl);
    let original = parse_single("SELECT id, name FROM users");
    let rewritten = parse_single("SELECT id, email FROM users");

    let result = verify_rewrite(
        "different-columns",
        &original,
        &rewritten,
        &schema,
        &prover_config(),
    );

    match result {
        Ok(vr) => {
            assert!(
                matches!(
                    vr.proof,
                    metamorphosis_qed::prover::ProofResult::NotEquivalent { .. }
                ),
                "Expected NotEquivalent for different column selection, got: {:?}",
                vr.proof
            );
        }
        Err(e) => {
            panic!("Prover invocation failed: {e}");
        }
    }
}

#[test]
#[ignore = "requires qed-prover + z3 + cvc5 on PATH"]
fn test_tautological_where_is_provable() {
    let ddl = parse_ddl(test_schema_ddl());
    let schema = extract_rich_schema(&ddl);
    let original = parse_single("SELECT id FROM users WHERE id = id");
    let rewritten = parse_single("SELECT id FROM users");

    let result = verify_rewrite(
        "tautological-where",
        &original,
        &rewritten,
        &schema,
        &prover_config(),
    );

    match result {
        Ok(vr) => {
            assert!(
                matches!(vr.proof, metamorphosis_qed::prover::ProofResult::Equivalent),
                "Expected Equivalent for tautological WHERE, got: {:?}",
                vr.proof
            );
        }
        Err(e) => {
            panic!("Prover invocation failed: {e}");
        }
    }
}

#[test]
#[ignore = "requires qed-prover + z3 + cvc5 on PATH"]
fn test_qualified_schema_names() {
    let ddl = parse_ddl(test_schema_ddl());
    let schema = extract_rich_schema(&ddl);
    let original = parse_single("SELECT id FROM users");
    let rewritten = parse_single("SELECT id FROM users");

    let mut name_map = HashMap::new();
    name_map.insert("users".to_string(), "PUBLIC.users".to_string());

    let result = verify_rewrite_with_names(
        "qualified-names",
        &original,
        &rewritten,
        &schema,
        &prover_config(),
        Some(&name_map),
    );

    match result {
        Ok(vr) => {
            assert!(
                matches!(vr.proof, metamorphosis_qed::prover::ProofResult::Equivalent),
                "Expected Equivalent with qualified names, got: {:?}",
                vr.proof
            );
        }
        Err(e) => {
            panic!("Prover invocation failed: {e}");
        }
    }
}
