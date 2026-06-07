use metamorphosis_verieql::types::*;
use metamorphosis_verieql::VeriEql;

fn emp_schema() -> Vec<TableSchema> {
    vec![TableSchema {
        name: "EMP".into(),
        columns: vec![
            ColumnDef { name: "ID".into(), col_type: ColumnType::Integer },
            ColumnDef { name: "NAME".into(), col_type: ColumnType::Varchar },
            ColumnDef { name: "DEPT".into(), col_type: ColumnType::Integer },
        ],
    }]
}

fn no_constraints() -> serde_json::Value {
    serde_json::json!(null)
}

#[test]
fn identical_queries_are_equivalent() {
    let report = VeriEql::verify(
        "SELECT ID FROM EMP",
        "SELECT ID FROM EMP",
        &emp_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    ).unwrap();

    assert!(
        matches!(report.result, ProofResult::Equivalent),
        "expected Equivalent, got {:?}",
        report.result
    );
}

#[test]
fn trivially_equivalent_select_star() {
    let report = VeriEql::verify(
        "SELECT * FROM EMP",
        "SELECT * FROM EMP",
        &emp_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    ).unwrap();

    assert!(
        matches!(report.result, ProofResult::Equivalent),
        "expected Equivalent, got {:?}",
        report.result
    );
}

#[test]
fn different_where_clauses_may_not_be_equivalent() {
    let report = VeriEql::verify(
        "SELECT ID FROM EMP WHERE ID = 1",
        "SELECT ID FROM EMP WHERE ID = 2",
        &emp_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    ).unwrap();

    // These should NOT be equivalent — they produce different results
    assert!(
        matches!(report.result, ProofResult::NotEquivalent { .. }),
        "expected NotEquivalent, got {:?}",
        report.result
    );
}

#[test]
fn where_true_vs_no_where() {
    let report = VeriEql::verify(
        "SELECT ID FROM EMP WHERE 1 = 1",
        "SELECT ID FROM EMP",
        &emp_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    ).unwrap();

    assert!(
        matches!(report.result, ProofResult::Equivalent),
        "expected Equivalent (WHERE TRUE is same as no WHERE), got {:?}",
        report.result
    );
}

#[test]
fn parse_error_returns_err() {
    let result = VeriEql::verify(
        "INVALID SQL @@@",
        "SELECT ID FROM EMP",
        &emp_schema(),
        &no_constraints(),
        Bound(2),
        Semantics::Bag,
    );

    assert!(result.is_err(), "expected parse error");
}
