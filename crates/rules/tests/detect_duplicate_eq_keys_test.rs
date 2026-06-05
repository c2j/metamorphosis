use metamorphosis_core::types::RewriteAction;
use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry};
use metamorphosis_rules::detect_duplicate_eq_keys::DetectDuplicateEqKeys;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::Parser;

fn test_suggest(sql: &str) -> (Vec<Statement>, Vec<metamorphosis_core::Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(DetectDuplicateEqKeys)]));
    let config = RewriteConfig::default();
    let ctx = RewriteContext {
        version: None,
        schema: None,
        config: &config,
        source_file: None,
        known_variables: None,
    };

    let (stmts, _errors) = Parser::parse_sql(sql);
    let statements: Vec<Statement> = stmts.into_iter().map(|si| si.statement).collect();

    let result = engine.rewrite(&ctx, statements);
    (result.statements, result.suggestions)
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
