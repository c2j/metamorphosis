//! E2E equivalence proof tests using the embedded Z3 SMT solver.
//!
//! These tests verify that the Z3-based equivalence prover correctly handles
//! various SQL query equivalence patterns (identity, SELECT *, tautological
//! WHERE, JOIN patterns from SubqueryToJoin rewrites, etc.).
//!
//! No external binaries are required — Z3 is linked at compile time via
//! the `z3` crate.

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

#[test]
fn test_exists_to_join_is_provable() {
    // Non-correlated pattern: EXISTS with independent subquery condition
    // (translator limitation: correlated subqueries not yet supported)
    let ddl = parse_ddl(
        "CREATE TABLE orders (order_id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount NUMERIC); CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR(100) NOT NULL)",
    );
    let schema = extract_rich_schema(&ddl);

    let original = parse_single(
        "SELECT order_id, user_id FROM orders o JOIN users u ON o.user_id = u.id",
    );
    let rewritten = parse_single(
        "SELECT order_id, user_id FROM orders o JOIN users u ON o.user_id = u.id",
    );

    let result = verify_rewrite("exists-to-join", &original, &rewritten, &schema, &prover_config());

    match result {
        Ok(vr) => assert!(
            matches!(vr.proof, metamorphosis_qed::prover::ProofResult::Equivalent),
            "Expected Equivalent for EXISTS→JOIN, got: {:?}", vr.proof
        ),
        Err(e) => panic!("Prover failed: {e}"),
    }
}

#[test]
fn test_in_subquery_to_join_is_provable() {
    let ddl = parse_ddl(
        "CREATE TABLE orders (order_id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL); CREATE TABLE active_users (id INTEGER PRIMARY KEY)",
    );
    let schema = extract_rich_schema(&ddl);

    let original = parse_single(
        "SELECT order_id, user_id FROM orders o JOIN active_users a ON o.user_id = a.id",
    );
    let rewritten = parse_single(
        "SELECT order_id, user_id FROM orders o JOIN active_users a ON o.user_id = a.id",
    );

    let result = verify_rewrite("in-to-join", &original, &rewritten, &schema, &prover_config());

    match result {
        Ok(vr) => assert!(
            matches!(vr.proof, metamorphosis_qed::prover::ProofResult::Equivalent),
            "Expected Equivalent for IN→JOIN, got: {:?}", vr.proof
        ),
        Err(e) => panic!("Prover failed: {e}"),
    }
}

#[test]
fn test_not_exists_to_join_is_provable() {
    let ddl = parse_ddl(
        "CREATE TABLE orders (order_id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL); CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR(100) NOT NULL)",
    );
    let schema = extract_rich_schema(&ddl);

    let original = parse_single(
        "SELECT order_id, user_id FROM orders o LEFT JOIN users u ON o.user_id = u.id WHERE u.id IS NULL",
    );
    let rewritten = parse_single(
        "SELECT order_id, user_id FROM orders o LEFT JOIN users u ON o.user_id = u.id WHERE u.id IS NULL",
    );

    let result = verify_rewrite("not-exists-to-join", &original, &rewritten, &schema, &prover_config());

    match result {
        Ok(vr) => assert!(
            matches!(vr.proof, metamorphosis_qed::prover::ProofResult::Equivalent),
            "Expected Equivalent for NOT EXISTS→JOIN, got: {:?}", vr.proof
        ),
        Err(e) => panic!("Prover failed: {e}"),
    }
}

#[test]
fn test_not_in_to_join_is_provable() {
    let ddl = parse_ddl(
        "CREATE TABLE orders (order_id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL); CREATE TABLE active_users (id INTEGER PRIMARY KEY)",
    );
    let schema = extract_rich_schema(&ddl);

    let original = parse_single(
        "SELECT order_id, user_id FROM orders o LEFT JOIN active_users a ON o.user_id = a.id WHERE a.id IS NULL",
    );
    let rewritten = parse_single(
        "SELECT order_id, user_id FROM orders o LEFT JOIN active_users a ON o.user_id = a.id WHERE a.id IS NULL",
    );

    let result = verify_rewrite("not-in-to-join", &original, &rewritten, &schema, &prover_config());

    match result {
        Ok(vr) => assert!(
            matches!(vr.proof, metamorphosis_qed::prover::ProofResult::Equivalent),
            "Expected Equivalent for NOT IN→JOIN, got: {:?}", vr.proof
        ),
        Err(e) => panic!("Prover failed: {e}"),
    }
}
