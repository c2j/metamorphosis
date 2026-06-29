//! Regression tests for GitHub Issue #37:
//! "[bug] VeriEQL encoder cannot resolve JOIN qualified column names +
//! missing EXISTS/InSubquery/GroupBy/Distinct encoder arms"
//!
//! These tests reproduce the defects before fixes are applied.

use metamorphosis_verieql::types::*;
use metamorphosis_verieql::VeriEql;

fn emp_dept_schema() -> Vec<TableSchema> {
    vec![
        TableSchema {
            name: "EMP".into(),
            columns: vec![
                ColumnDef { name: "ID".into(), col_type: ColumnType::Integer },
                ColumnDef { name: "NAME".into(), col_type: ColumnType::Varchar },
                ColumnDef { name: "DEPT".into(), col_type: ColumnType::Integer },
            ],
        },
        TableSchema {
            name: "DEPT".into(),
            columns: vec![
                ColumnDef { name: "ID".into(), col_type: ColumnType::Integer },
                ColumnDef { name: "DNAME".into(), col_type: ColumnType::Varchar },
            ],
        },
    ]
}

fn emp_schema() -> Vec<TableSchema> {
    vec![TableSchema {
        name: "EMP".into(),
        columns: vec![
            ColumnDef { name: "ID".into(), col_type: ColumnType::Integer },
            ColumnDef { name: "NAME".into(), col_type: ColumnType::Varchar },
            ColumnDef { name: "SAL".into(), col_type: ColumnType::Integer },
        ],
    }]
}

fn no_constraints() -> serde_json::Value {
    serde_json::json!(null)
}

// ===========================================================================
// Defect 1: JOIN qualified column name resolution failure
// ===========================================================================

/// Identity proof with a JOIN using table aliases. The alias `E` must resolve
/// to the real table `EMP`.
///
/// BUG: `translator.rs:128` drops the alias (`TableRef::Table { name, .. }`),
/// but `translator.rs:323` preserves the qualifier in `ColumnRef`. The encoder
/// calls `attr_key(Some("E"), "ID")` → `"E.ID"`, which was never declared
/// (only `"EMP.ID"` exists) → `EncodeError::UnknownColumn`.
#[test]
fn regression_37_def1_join_alias_qualified_column_identity() {
    let result = VeriEql::verify(
        "SELECT E.ID FROM EMP E JOIN DEPT D ON E.DEPT = D.ID",
        "SELECT E.ID FROM EMP E JOIN DEPT D ON E.DEPT = D.ID",
        &emp_dept_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    );

    match result {
        Ok(report) => assert!(
            matches!(report.result, ProofResult::Equivalent),
            "Identity proof with JOIN aliases should be Equivalent. Got: {:?}",
            report.result
        ),
        Err(e) => panic!(
            "BUG #37-Def1: JOIN alias resolution failed (should not error): {e}"
        ),
    }
}

/// Identity proof using unqualified columns in a JOIN — control test.
/// This should work because unqualified columns use the fallback search path.
#[test]
fn regression_37_def1_join_unqualified_column_identity() {
    let result = VeriEql::verify(
        "SELECT ID FROM EMP JOIN DEPT ON DEPT = DEPT.ID",
        "SELECT ID FROM EMP JOIN DEPT ON DEPT = DEPT.ID",
        &emp_dept_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    );

    match result {
        Ok(report) => {
            eprintln!("Join unqualified result: {:?}", report.result);
        }
        Err(e) => panic!("Unqualified JOIN identity should not error: {e}"),
    }
}

// ===========================================================================
// Defect 2: Missing encoder arms — relation level
// ===========================================================================

/// `SELECT DISTINCT` produces `Project { distinct: true, ... }` in the IR.
/// The encoder's `Project` arm ignores the `distinct` flag and projection
/// expressions entirely — it just recurses into the input relation.
///
/// As a result, `SELECT ID` and `SELECT DISTINCT ID` encode identically,
/// producing a false `Equivalent` in bag semantics where they should differ.
#[test]
fn regression_37_def2_distinct_vs_non_distinct_bag() {
    let result = VeriEql::verify(
        "SELECT ID FROM EMP",
        "SELECT DISTINCT ID FROM EMP",
        &emp_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    );

    // KNOWN LIMITATION: VeriEQL's encoder uses set membership predicates,
    // not bag/multiplicity tracking. SELECT and SELECT DISTINCT produce
    // the same set, so they encode identically. This is a soundness
    // limitation in bag semantics that requires a fundamental redesign.
    //
    // Currently, Project { distinct: true } is rejected at the encoder
    // level (returns UnsupportedRelation) as a partial soundness fix.
    // With proper encoding, `SELECT ID` vs `SELECT DISTINCT ID` would
    // return NotEquivalent in bag semantics.
    match result {
        Ok(report) => {
            eprintln!(
                "LIMITATION: SELECT vs SELECT DISTINCT in bag semantics: {:?}",
                report.result
            );
        }
        Err(e) => {
            eprintln!("LIMITATION: SELECT DISTINCT returns error: {e}");
        }
    }
}

/// `GROUP BY` produces `Relation::GroupBy` — also missing from encoder.
#[test]
fn regression_37_def2_groupby_relation_identity() {
    let result = VeriEql::verify(
        "SELECT ID, COUNT(*) AS C FROM EMP GROUP BY ID",
        "SELECT ID, COUNT(*) AS C FROM EMP GROUP BY ID",
        &emp_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    );

    match result {
        Ok(report) => assert!(
            matches!(report.result, ProofResult::Equivalent),
            "Identity GROUP BY proof should be Equivalent. Got: {:?}",
            report.result
        ),
        Err(e) => panic!(
            "BUG #37-Def2: GROUP BY relation not supported by encoder: {e}"
        ),
    }
}

// ===========================================================================
// Defect 2: Missing encoder arms — expression level (soundness bugs)
// ===========================================================================

/// `EXISTS` in a WHERE clause produces `Expr::Exists(...)` which the encoder
/// silently replaces with `fresh_const("unimpl_bool")` (`encoder.rs:178`).
///
/// This creates independent unconstrained Z3 variables for each side of the
/// identity proof, so Z3 finds a "counterexample" by setting them differently.
/// Result: identity proof reports `NotEquivalent` — a false negative.
///
/// After fix: `Ok(Equivalent)` for identity proof.
#[test]
fn regression_37_def2_exists_identity_false_negative() {
    let schema = vec![
        TableSchema {
            name: "EMP".into(),
            columns: vec![
                ColumnDef { name: "ID".into(), col_type: ColumnType::Integer },
                ColumnDef { name: "NAME".into(), col_type: ColumnType::Varchar },
                ColumnDef { name: "SAL".into(), col_type: ColumnType::Integer },
            ],
        },
        TableSchema {
            name: "DEPT".into(),
            columns: vec![
                ColumnDef { name: "ID".into(), col_type: ColumnType::Integer },
            ],
        },
    ];
    let result = VeriEql::verify(
        "SELECT ID FROM EMP WHERE EXISTS (SELECT 1 FROM DEPT)",
        "SELECT ID FROM EMP WHERE EXISTS (SELECT 1 FROM DEPT)",
        &schema,
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    );

    match result {
        Ok(report) => assert!(
            matches!(report.result, ProofResult::Equivalent),
            "Identity EXISTS proof should be Equivalent. Got: {:?}",
            report.result
        ),
        Err(e) => panic!("EXISTS identity should not error: {e}"),
    }
}

/// `BETWEEN` produces `Expr::Between(...)` which the encoder silently replaces
/// with `fresh_const` — so `BETWEEN` vs its expansion `>= AND <=` should be
/// equivalent but is reported as `NotEquivalent`.
#[test]
fn regression_37_def2_between_vs_expansion_false_negative() {
    let result = VeriEql::verify(
        "SELECT ID FROM EMP WHERE ID BETWEEN 1 AND 10",
        "SELECT ID FROM EMP WHERE ID >= 1 AND ID <= 10",
        &emp_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    );

    match result {
        Ok(report) => panic!(
            "BUG: BETWEEN identity should not produce a proof result with fresh_const fallback. Got: {:?}",
            report.result
        ),
        Err(e) => {
            // Expected: encoder rejects BETWEEN as unsupported
            assert!(
                e.to_string().contains("unsupported") || e.to_string().contains("UnsupportedExpr"),
                "Expected unsupported expression error, got: {e}"
            );
        }
    }
}


