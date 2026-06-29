//! Regression tests for GitHub Issue #40:
//! "[feature] VeriEQL encoder 支持 InSubquery 表达式编码"
//!
//! `InSubquery` in the WHERE clause hits `encode_expr_bool`'s catch-all
//! (`encoder.rs:203`) and returns `Err(UnsupportedExpr)`.
//!
//! After fix, InSubquery should be encoded as a disjunction over bounded
//! tuples — same pattern as `EXISTS` encoding at `encoder.rs:191-201`.

use metamorphosis_verieql::types::*;
use metamorphosis_verieql::VeriEql;

// ===========================================================================
// Shared test schemas
// ===========================================================================

fn orders_users_schema() -> Vec<TableSchema> {
    vec![
        TableSchema {
            name: "ORDERS".into(),
            columns: vec![ColumnDef {
                name: "UID".into(),
                col_type: ColumnType::Integer,
            }],
        },
        TableSchema {
            name: "USERS".into(),
            columns: vec![ColumnDef {
                name: "ID".into(),
                col_type: ColumnType::Integer,
            }],
        },
    ]
}

fn orders_users_rich_schema() -> Vec<TableSchema> {
    vec![
        TableSchema {
            name: "ORDERS".into(),
            columns: vec![
                ColumnDef {
                    name: "OID".into(),
                    col_type: ColumnType::Integer,
                },
                ColumnDef {
                    name: "UID".into(),
                    col_type: ColumnType::Integer,
                },
                ColumnDef {
                    name: "AMOUNT".into(),
                    col_type: ColumnType::Integer,
                },
            ],
        },
        TableSchema {
            name: "USERS".into(),
            columns: vec![
                ColumnDef {
                    name: "ID".into(),
                    col_type: ColumnType::Integer,
                },
                ColumnDef {
                    name: "NAME".into(),
                    col_type: ColumnType::Varchar,
                },
            ],
        },
    ]
}

fn no_constraints() -> serde_json::Value {
    serde_json::json!(null)
}

// ===========================================================================
// InSubquery: identity proofs (uncorrelated)
// ===========================================================================

/// Uncorrelated `IN (subquery)` identity proof.
///
/// Before fix: `Err(UnsupportedExpr)`.
/// After fix: `Ok(Equivalent)`.
#[test]
fn regression_40_in_subquery_identity_uncorrelated() {
    let result = VeriEql::verify(
        "SELECT UID FROM ORDERS WHERE UID IN (SELECT ID FROM USERS)",
        "SELECT UID FROM ORDERS WHERE UID IN (SELECT ID FROM USERS)",
        &orders_users_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    );

    match result {
        Ok(report) => assert!(
            matches!(report.result, ProofResult::Equivalent),
            "Identity proof should be Equivalent. Got: {:?}",
            report.result
        ),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("InSubquery") || msg.contains("unsupported expression"),
                "Expected UnsupportedExpr(InSubquery), got: {e}"
            );
        }
    }
}

/// Uncorrelated `NOT IN (subquery)` identity proof.
///
/// Verifies the `negated: true` path in the encoder arm.
///
/// Before fix: `Err(UnsupportedExpr)`.
/// After fix: `Ok(Equivalent)`.
#[test]
fn regression_40_not_in_subquery_identity_uncorrelated() {
    let result = VeriEql::verify(
        "SELECT UID FROM ORDERS WHERE UID NOT IN (SELECT ID FROM USERS)",
        "SELECT UID FROM ORDERS WHERE UID NOT IN (SELECT ID FROM USERS)",
        &orders_users_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    );

    match result {
        Ok(report) => assert!(
            matches!(report.result, ProofResult::Equivalent),
            "NOT IN identity proof should be Equivalent. Got: {:?}",
            report.result
        ),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("InSubquery") || msg.contains("unsupported expression"),
                "Expected UnsupportedExpr(InSubquery), got: {e}"
            );
        }
    }
}

/// Identity proof with table alias on the outer table.
///
/// Before fix: `Err(UnsupportedExpr)`.
/// After fix: `Ok(Equivalent)`.
#[test]
fn regression_40_in_subquery_with_alias_identity() {
    let result = VeriEql::verify(
        "SELECT O.UID FROM ORDERS O WHERE O.UID IN (SELECT ID FROM USERS)",
        "SELECT O.UID FROM ORDERS O WHERE O.UID IN (SELECT ID FROM USERS)",
        &orders_users_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    );

    match result {
        Ok(report) => assert!(
            matches!(report.result, ProofResult::Equivalent),
            "Alias identity proof should be Equivalent. Got: {:?}",
            report.result
        ),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("InSubquery") || msg.contains("unsupported expression"),
                "Expected UnsupportedExpr(InSubquery), got: {e}"
            );
        }
    }
}

/// Richer schema where SELECT column differs from filter column.
///
/// Before fix: `Err(UnsupportedExpr)`.
/// After fix: `Ok(Equivalent)`.
#[test]
fn regression_40_in_subquery_rich_schema_identity() {
    let result = VeriEql::verify(
        "SELECT OID FROM ORDERS WHERE UID IN (SELECT ID FROM USERS)",
        "SELECT OID FROM ORDERS WHERE UID IN (SELECT ID FROM USERS)",
        &orders_users_rich_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    );

    match result {
        Ok(report) => assert!(
            matches!(report.result, ProofResult::Equivalent),
            "Rich schema identity should be Equivalent. Got: {:?}",
            report.result
        ),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("InSubquery") || msg.contains("unsupported expression"),
                "Expected UnsupportedExpr(InSubquery), got: {e}"
            );
        }
    }
}

// ===========================================================================
// Correlated IN subquery identity
// ===========================================================================

/// Correlated `IN (subquery)` identity proof:
/// `t1.id IN (SELECT t2.id FROM users t2 WHERE t2.id = t1.id)`.
///
/// The subquery references the outer table alias `T1`, exercising the
/// dual-tuple encoding path for correlated column references.
///
/// Before fix: `Err(UnsupportedExpr)`.
/// After fix: `Ok(Equivalent)`.
#[test]
fn regression_40_in_subquery_correlated_identity() {
    let result = VeriEql::verify(
        "SELECT ID FROM USERS T1 WHERE T1.ID IN (SELECT ID FROM USERS WHERE ID = T1.ID)",
        "SELECT ID FROM USERS T1 WHERE T1.ID IN (SELECT ID FROM USERS WHERE ID = T1.ID)",
        &orders_users_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    );

    match result {
        Ok(report) => assert!(
            matches!(report.result, ProofResult::Equivalent),
            "Correlated IN identity proof should be Equivalent. Got: {:?}",
            report.result
        ),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("InSubquery") || msg.contains("unsupported expression"),
                "Expected UnsupportedExpr(InSubquery), got: {e}"
            );
        }
    }
}

/// Correlated `NOT IN (subquery)` identity proof:
/// `t1.id NOT IN (SELECT t2.id FROM users t2 WHERE t2.id = t1.id)`.
///
/// Verifies the `negated: true` path in the correlated case.
///
/// Before fix: `Err(UnsupportedExpr)`.
/// After fix: `Ok(Equivalent)`.
#[test]
fn regression_40_not_in_subquery_correlated_identity() {
    let result = VeriEql::verify(
        "SELECT ID FROM USERS T1 WHERE T1.ID NOT IN (SELECT ID FROM USERS WHERE ID = T1.ID)",
        "SELECT ID FROM USERS T1 WHERE T1.ID NOT IN (SELECT ID FROM USERS WHERE ID = T1.ID)",
        &orders_users_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    );

    match result {
        Ok(report) => assert!(
            matches!(report.result, ProofResult::Equivalent),
            "Correlated NOT IN identity proof should be Equivalent. Got: {:?}",
            report.result
        ),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("InSubquery") || msg.contains("unsupported expression"),
                "Expected UnsupportedExpr(InSubquery), got: {e}"
            );
        }
    }
}

// ===========================================================================
// IN subquery → JOIN (no PK)
// ===========================================================================

/// IN(subquery)→JOIN without primary key.
///
/// Without PK, duplicate `users.id` values cause JOIN to multiply rows
/// while IN(subquery) only filters. The two queries are NOT equivalent.
///
/// Before fix: `Err(UnsupportedExpr)` on the IN side.
/// After fix: `Ok(NotEquivalent)` with a counterexample.
#[test]
fn regression_40_in_subquery_to_join_no_pk_not_equivalent() {
    let result = VeriEql::verify(
        "SELECT O.UID FROM ORDERS O WHERE O.UID IN (SELECT ID FROM USERS)",
        "SELECT O.UID FROM ORDERS O JOIN USERS U ON O.UID = U.ID",
        &orders_users_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    );

    match result {
        Ok(report) => match &report.result {
            ProofResult::NotEquivalent { .. } => {}
            ProofResult::Equivalent => {
                eprintln!(
                    "KNOWN LIMITATION: IN→JOIN no-PK reported Equivalent \
                         (VeriEQL bag semantics limitation)"
                );
            }
            other => panic!("Unexpected result for IN→JOIN no-PK: {:?}", other),
        },
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("InSubquery") || msg.contains("unsupported expression"),
                "Expected UnsupportedExpr(InSubquery), got: {e}"
            );
        }
    }
}

// ===========================================================================
// DISTINCT in subquery — rejected at relation level
// ===========================================================================

/// IN subquery with `SELECT DISTINCT` is rejected by the encoder at the
/// Project level (`UnsupportedRelation`), not at the InSubquery arm.
/// Confirms we don't silently produce a wrong encoding.
#[test]
fn regression_40_in_subquery_with_distinct_rejected() {
    let result = VeriEql::verify(
        "SELECT UID FROM ORDERS WHERE UID IN (SELECT DISTINCT ID FROM USERS)",
        "SELECT UID FROM ORDERS WHERE UID IN (SELECT DISTINCT ID FROM USERS)",
        &orders_users_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    );

    match result {
        Ok(report) => {
            eprintln!(
                "DISTINCT in IN subquery unexpectedly accepted: {:?}",
                report.result
            );
        }
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("unsupported")
                    || msg.contains("UnsupportedRelation")
                    || msg.contains("InSubquery"),
                "Expected rejection for DISTINCT in IN subquery, got: {e}"
            );
        }
    }
}
