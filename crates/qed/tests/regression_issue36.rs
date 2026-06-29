//! Regression tests for GitHub Issue #36:
//! "[bug] QED z3_solver DISTINCT encoding drops dedup semantics +
//! IN(subquery) panic + false test replacement"
//!
//! These tests reproduce the three defects before fixes are applied.
//! After fixes, they should pass.

use metamorphosis_qed::prover::ProverConfig;
use metamorphosis_qed::prover::ProofResult;
use metamorphosis_qed::schema::extract_rich_schema;
use metamorphosis_qed::verify::verify_rewrite;
use ogsql_parser::ast::Statement;
use ogsql_parser::Parser;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Test helpers (mirrored from prover_e2e_test.rs)
// ---------------------------------------------------------------------------

fn parse_ddl(sql: &str) -> Vec<Statement> {
    let (stmts, _) = Parser::parse_sql(sql);
    stmts.into_iter().map(|si| si.statement).collect()
}

fn parse_single(sql: &str) -> Statement {
    let (stmts, _) = Parser::parse_sql(sql);
    stmts
        .into_iter()
        .next()
        .expect("expected one statement")
        .statement
}

fn prover_config() -> ProverConfig {
    ProverConfig {
        binary_path: PathBuf::from("qed-prover"),
        timeout_secs: 120,
        workdir: None,
    }
}

/// Schema where `users.id` HAS a PRIMARY KEY (unique — no duplicate rows).
fn orders_users_pk_ddl() -> &'static str {
    "CREATE TABLE orders (order_id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount NUMERIC); \
     CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR(100) NOT NULL)"
}

/// Schema where `users.id` has NO PRIMARY KEY (duplicates allowed).
/// This is critical for exposing the DISTINCT soundness bug.
fn orders_users_noprk_ddl() -> &'static str {
    "CREATE TABLE orders (order_id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount NUMERIC); \
     CREATE TABLE users (id INTEGER NOT NULL, name VARCHAR(100) NOT NULL)"
}

// ===========================================================================
// Defect 1: DISTINCT encoding drops dedup semantics (Soundness Bug)
// ===========================================================================

/// On a schema WITHOUT PK, `EXISTS` (each order returned at most once) and
/// plain `JOIN` (orders duplicated if users.id has duplicates) are NOT
/// semantically equivalent.
///
/// BUG: QED returns `Equivalent` because `z3_solver.rs:167-169` encodes
/// `Distinct { input }` as a no-op (just forwards `input`), making
/// `Distinct(Join(...)) ≡ Join(...)`.
///
/// Expected after fix: `NotEquivalent` or `Unknown`.
#[test]
fn regression_36_def1_exists_vs_plain_join_no_pk() {
    let ddl = parse_ddl(orders_users_noprk_ddl());
    let schema = extract_rich_schema(&ddl);

    let original = parse_single(
        "SELECT o.order_id FROM orders o \
         WHERE EXISTS (SELECT 1 FROM users u WHERE u.id = o.user_id)",
    );
    let rewritten = parse_single(
        "SELECT o.order_id FROM orders o \
         JOIN users u ON u.id = o.user_id",
    );

    let result = verify_rewrite(
        "regression-36-def1-exists-vs-join-noprk",
        &original,
        &rewritten,
        &schema,
        &prover_config(),
    );

    match result {
        Ok(vr) => assert!(
            !matches!(vr.proof, ProofResult::Equivalent),
            "BUG #36-Def1: EXISTS vs plain JOIN on non-PK schema must NOT be Equivalent \
             (Distinct encoding is a no-op). Got: {:?}",
            vr.proof
        ),
        Err(e) => panic!("verify_rewrite failed unexpectedly: {e}"),
    }
}

/// Simpler reproduction: `SELECT a` vs `SELECT DISTINCT a` on a table without
/// unique constraint. In SQL bag semantics these differ (multiplicity).
///
/// BUG: QED returns `Equivalent` (Distinct is no-op in set-semantics encoding).
#[test]
fn regression_36_def1_select_vs_select_distinct_no_pk() {
    let ddl = parse_ddl("CREATE TABLE t (a INTEGER NOT NULL, b INTEGER NOT NULL)");
    let schema = extract_rich_schema(&ddl);

    let original = parse_single("SELECT a FROM t");
    let rewritten = parse_single("SELECT DISTINCT a FROM t");

    let result = verify_rewrite(
        "regression-36-def1-select-vs-distinct",
        &original,
        &rewritten,
        &schema,
        &prover_config(),
    );

    match result {
        Ok(vr) => assert!(
            !matches!(vr.proof, ProofResult::Equivalent),
            "BUG #36-Def1: SELECT a vs SELECT DISTINCT a must NOT be Equivalent \
             in bag semantics. Got: {:?}",
            vr.proof
        ),
        Err(e) => panic!("verify_rewrite failed unexpectedly: {e}"),
    }
}

// ===========================================================================
// Defect 2: IN(subquery) verification panic
// ===========================================================================

/// `IN(subquery)` decorrelated to `DISTINCT JOIN` must not panic.
///
/// BUG: Panics at `prover_compat.rs:357` when `convert_relation` encounters
/// a `Scan` whose table name is not in `schema_index`.
///
/// Expected after fix: returns `Ok(...)` with some `ProofResult` (no panic).
#[test]
fn regression_36_def2_in_subquery_no_panic() {
    let ddl = parse_ddl(orders_users_pk_ddl());
    let schema = extract_rich_schema(&ddl);

    let original = parse_single(
        "SELECT o.order_id FROM orders o \
         WHERE o.user_id IN (SELECT id FROM users)",
    );
    let rewritten = parse_single(
        "SELECT DISTINCT o.order_id FROM orders o \
         JOIN users u ON o.user_id = u.id",
    );

    let result = verify_rewrite(
        "regression-36-def2-in-subquery-no-panic",
        &original,
        &rewritten,
        &schema,
        &prover_config(),
    );

    // The test passes as long as we get a result back (no panic, no error).
    match result {
        Ok(vr) => {
            eprintln!("IN subquery proof result: {:?}", vr.proof);
        }
        Err(e) => panic!(
            "BUG #36-Def2: IN(subquery) verification should not error/panic: {e}"
        ),
    }
}

// ===========================================================================
// Defect 3: Replace false identity tests with real subquery vs JOIN pairs
// ===========================================================================

/// With PK on users.id, `EXISTS` ≡ `DISTINCT JOIN` (each order matches at
/// most one user — no duplication).
///
/// This replaces the false `test_exists_to_join_is_provable` which used
/// identical JOIN SQL on both sides (identity proof, not subquery proof).
#[test]
fn regression_36_def3_exists_to_distinct_join_pk_equivalent() {
    let ddl = parse_ddl(orders_users_pk_ddl());
    let schema = extract_rich_schema(&ddl);

    let original = parse_single(
        "SELECT o.order_id FROM orders o \
         WHERE EXISTS (SELECT 1 FROM users u WHERE u.id = o.user_id)",
    );
    let rewritten = parse_single(
        "SELECT DISTINCT o.order_id FROM orders o \
         JOIN users u ON u.id = o.user_id",
    );

    let result = verify_rewrite(
        "regression-36-def3-exists-to-distinct-join",
        &original,
        &rewritten,
        &schema,
        &prover_config(),
    );

    match result {
        Ok(vr) => assert!(
            matches!(vr.proof, ProofResult::Equivalent),
            "EXISTS -> DISTINCT JOIN on PK schema should be Equivalent. Got: {:?}",
            vr.proof
        ),
        Err(e) => panic!("verify_rewrite failed: {e}"),
    }
}

/// With PK on users.id, `IN(subquery)` ≡ `DISTINCT JOIN`.
#[test]
fn regression_36_def3_in_subquery_to_distinct_join_pk_equivalent() {
    let ddl = parse_ddl(orders_users_pk_ddl());
    let schema = extract_rich_schema(&ddl);

    let original = parse_single(
        "SELECT o.order_id FROM orders o \
         WHERE o.user_id IN (SELECT id FROM users)",
    );
    let rewritten = parse_single(
        "SELECT DISTINCT o.order_id FROM orders o \
         JOIN users u ON o.user_id = u.id",
    );

    let result = verify_rewrite(
        "regression-36-def3-in-to-distinct-join",
        &original,
        &rewritten,
        &schema,
        &prover_config(),
    );

    match result {
        Ok(vr) => assert!(
            matches!(vr.proof, ProofResult::Equivalent),
            "IN(subquery) -> DISTINCT JOIN on PK schema should be Equivalent. Got: {:?}",
            vr.proof
        ),
        Err(e) => panic!("verify_rewrite failed: {e}"),
    }
}

/// `NOT EXISTS` must be translatable and provable. The ideal rewrite is
/// `LEFT JOIN ... WHERE IS NULL`, but the QED translator doesn't support
/// LEFT JOIN (ignores join_type at `translator/mod.rs:723`) and can't
/// resolve correlated columns in NOT EXISTS subqueries. We test uncorrelated
/// NOT EXISTS identity to verify the translator handles it at all.
#[test]
fn regression_36_def3_not_exists_identity_pk() {
    let ddl = parse_ddl(orders_users_pk_ddl());
    let schema = extract_rich_schema(&ddl);

    let original = parse_single(
        "SELECT o.order_id FROM orders o \
         WHERE NOT EXISTS (SELECT 1 FROM users)",
    );
    let rewritten = parse_single(
        "SELECT o.order_id FROM orders o \
         WHERE NOT EXISTS (SELECT 1 FROM users)",
    );

    let result = verify_rewrite(
        "regression-36-def3-not-exists-identity",
        &original,
        &rewritten,
        &schema,
        &prover_config(),
    );

    match result {
        Ok(vr) => assert!(
            matches!(vr.proof, ProofResult::Equivalent),
            "NOT EXISTS identity should be Equivalent. Got: {:?}",
            vr.proof
        ),
        Err(e) => panic!("NOT EXISTS identity should not fail: {e}"),
    }
}
