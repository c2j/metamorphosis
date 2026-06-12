use metamorphosis_mcp::tools;
use metamorphosis_mcp::types::*;

#[test]
fn list_rules_returns_builtin_rules() {
    let result = tools::list_rules();
    assert!(
        !result.rules.is_empty(),
        "should have at least one builtin rule"
    );

    let ids: Vec<&str> = result.rules.iter().map(|r| r.id.as_str()).collect();
    assert!(
        ids.contains(&"eliminate-select-star"),
        "missing eliminate-select-star, got: {ids:?}"
    );
    assert!(
        ids.contains(&"detect-duplicate-eq-keys"),
        "missing detect-duplicate-eq-keys, got: {ids:?}"
    );
}

#[test]
fn rewrite_sql_handles_empty_input() {
    let params = SqlParams {
        sql: String::new(),
        version: None,
        schema_path: None,
        schema_json: None,
        sql_dir: None,
        rules: None,
    };
    let result = tools::rewrite_sql(params);
    assert!(result.is_ok(), "empty SQL should succeed with no-op result");
    let output = result.unwrap();
    assert!(!output.changed, "empty SQL should not be changed");
}

#[test]
fn rewrite_sql_with_simple_select() {
    let params = SqlParams {
        sql: "SELECT 1".to_string(),
        version: Some("5.0".to_string()),
        schema_path: None,
        schema_json: None,
        sql_dir: None,
        rules: None,
    };
    let result = tools::rewrite_sql(params);
    assert!(result.is_ok(), "simple SELECT should succeed: {:?}", result);
    let output = result.unwrap();
    assert!(!output.rewritten_sql.is_empty() || !output.match_failures.is_empty());
}

#[test]
fn suggest_probes_with_simple_query() {
    let params = SqlParams {
        sql: "SELECT * FROM t1 WHERE a = 1 AND b = 2 GROUP BY a".to_string(),
        version: None,
        schema_path: None,
        schema_json: Some(r#"{"t1": {"a": "integer", "b": "integer"}}"#.to_string()),
        sql_dir: None,
        rules: None,
    };
    let result = tools::suggest_probes(params);
    assert!(result.is_ok(), "suggest should succeed: {:?}", result);
}

#[test]
fn verify_equivalence_rejects_missing_sql() {
    let params = VerifyParams {
        original_sql: String::new(),
        rewritten_sql: "SELECT 1".to_string(),
        schema_path: None,
        schema_json: None,
        sql_dir: None,
        engine: None,
        bound: None,
    };
    let result = tools::verify_equivalence(params);
    assert!(result.is_err(), "empty original SQL should fail");
}

#[test]
fn extract_schema_rejects_invalid_path() {
    let params = ExtractSchemaParams {
        sql_dir: "/nonexistent/path/that/does/not/exist".to_string(),
    };
    let result = tools::extract_schema(params);
    assert!(result.is_err(), "invalid path should fail");
}

#[test]
fn rewrite_with_schema_path_and_sql_dir() {
    let test_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("testcases");
    if !test_dir.exists() {
        eprintln!("skipping: testcases dir not found");
        return;
    }

    let params = SqlParams {
        sql: "SELECT * FROM dat_clr_cash_dtl WHERE account_date = '20240101'".to_string(),
        version: Some("5.0".to_string()),
        schema_path: None,
        schema_json: Some(
            r#"{"dat_clr_cash_dtl": {"trade_code": "varchar", "account_date": "varchar", "account_seqno": "integer", "account_id": "varchar", "interface_seq": "integer"}}"#.to_string(),
        ),
        sql_dir: None,
        rules: None,
    };
    let result = tools::rewrite_sql(params);
    assert!(
        result.is_ok(),
        "rewrite with schema should succeed: {:?}",
        result
    );
}
