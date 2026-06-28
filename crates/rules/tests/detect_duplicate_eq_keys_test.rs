use metamorphosis_core::types::RewriteAction;
use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry};
use metamorphosis_rules::detect_duplicate_eq_keys::DetectDuplicateEqKeys;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::Parser;
use std::collections::HashSet;

fn test_suggest(sql: &str) -> (Vec<Statement>, Vec<metamorphosis_core::Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(DetectDuplicateEqKeys)]));
    let config = RewriteConfig::default();
    let ctx = RewriteContext {
        version: None,
        schema: None,
        config: &config,
        source_file: None,
        known_variables: None,
        diagnostic_hints: None,
    };

    let (stmts, _errors) = Parser::parse_sql(sql);
    let statements: Vec<Statement> = stmts.into_iter().map(|si| si.statement).collect();

    let result = engine.rewrite(&ctx, statements);
    (result.statements, result.suggestions)
}

fn test_suggest_with_vars(
    sql: &str,
    known_variables: HashSet<String>,
) -> (Vec<Statement>, Vec<metamorphosis_core::Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(DetectDuplicateEqKeys)]));
    let config = RewriteConfig::default();
    let ctx = RewriteContext {
        version: None,
        schema: None,
        config: &config,
        source_file: None,
        known_variables: Some(&known_variables),
        diagnostic_hints: None,
    };

    let (stmts, _errors) = Parser::parse_sql(sql);
    let statements: Vec<Statement> = stmts.into_iter().map(|si| si.statement).collect();

    let result = engine.rewrite(&ctx, statements);
    (result.statements, result.suggestions)
}

fn format_probe(suggestions: &[metamorphosis_core::Suggestion]) -> Option<String> {
    suggestions.first().and_then(|s| match &s.action {
        RewriteAction::Generate { stmt, .. } => Some(SqlFormatter::new().format_statement(stmt)),
        _ => None,
    })
}

#[test]
fn test_generate_probe_for_two_eq_keys() {
    // Column = unknown-variable patterns (v_ prefixed names not in FROM aliases)
    let (_statements, suggestions) = test_suggest(
        "SELECT * FROM orders WHERE orders.account_id = v_user_id AND orders.status = v_status",
    );
    assert!(
        !suggestions.is_empty(),
        "Rule should detect two eq conditions"
    );
    assert_eq!(suggestions[0].rule_id, "detect-duplicate-eq-keys");
}

#[test]
fn test_probe_sql_contains_group_by() {
    // Column = unknown-variable patterns (v_ prefixed names not in FROM aliases)
    let (_statements, suggestions) = test_suggest(
        "SELECT * FROM users WHERE users.tenant_id = v_tenant AND users.user_id = v_user",
    );

    assert!(!suggestions.is_empty());
    if let RewriteAction::Generate { ref stmt, .. } = suggestions[0].action {
        let sql = SqlFormatter::new().format_statement(stmt);
        let upper = sql.to_uppercase();
        assert!(
            upper.contains("GROUP BY"),
            "Probe SQL must have GROUP BY: {}",
            sql
        );
        assert!(
            upper.contains("HAVING"),
            "Probe SQL must have HAVING: {}",
            sql
        );
        assert!(
            sql.contains("tenant_id"),
            "Probe must reference tenant_id, got: {}",
            sql
        );
        assert!(
            sql.contains("user_id"),
            "Probe must reference user_id, got: {}",
            sql
        );
    } else {
        panic!("Expected Generate action");
    }
}

#[test]
fn test_single_eq_no_match() {
    let (_statements, suggestions) = test_suggest("SELECT * FROM users WHERE users.id = v_id");
    assert!(
        suggestions.is_empty(),
        "Single eq condition should not match"
    );
}

#[test]
fn test_no_eq_no_match() {
    let (_statements, suggestions) = test_suggest("SELECT * FROM users");
    assert!(suggestions.is_empty());
}

// ── CUD + subquery support ──

#[test]
fn test_update_two_param_eqs() {
    let mut vars = HashSet::new();
    vars.insert("v_a".to_string());
    vars.insert("v_b".to_string());
    let (_statements, suggestions) = test_suggest_with_vars(
        "UPDATE orders SET x = 1 WHERE col_a = v_a AND col_b = v_b AND region = 'EAST'",
        vars,
    );
    assert!(
        !suggestions.is_empty(),
        "Should match UPDATE with 2 param eqs"
    );
    let probe = format_probe(&suggestions).expect("Expected probe");
    assert!(
        probe.contains("col_a") && probe.contains("col_b"),
        "Probe must reference both cols: {}",
        probe
    );
}

#[test]
fn test_delete_subquery_two_params() {
    let (_statements, suggestions) = test_suggest(
        "DELETE FROM t WHERE id IN (SELECT id FROM t2 WHERE t2.col_a = v_a AND t2.col_b = v_b)",
    );
    assert!(
        !suggestions.is_empty(),
        "Should match subquery with 2 params"
    );
}

#[test]
fn test_update_single_param_no_match() {
    let (_statements, suggestions) = test_suggest("UPDATE t SET x = 1 WHERE col = v_col");
    assert!(
        suggestions.is_empty(),
        "Single param should not match detect_duplicate_eq_keys"
    );
}
