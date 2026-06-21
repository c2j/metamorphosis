use metamorphosis_core::types::{RewriteAction, Severity};
use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry, Suggestion};
use metamorphosis_rules::reject_no_where_dml::RejectNoWhereDml;
use ogsql_parser::ast::Statement;
use ogsql_parser::Parser;

fn test_rewrite(sql: &str) -> (Vec<Statement>, Vec<Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(RejectNoWhereDml)]));
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

// ── DELETE without WHERE ──

#[test]
fn test_delete_without_where() {
    let (_statements, suggestions) = test_rewrite("DELETE FROM users");
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].rule_id, "reject-no-where-dml");
    match &suggestions[0].action {
        RewriteAction::Suggest { message, severity } => {
            assert!(
                message.contains("DELETE"),
                "Message should mention DELETE: {}",
                message
            );
            assert_eq!(*severity, Severity::Critical);
        }
        _ => panic!("Expected Suggest action"),
    }
}

// ── UPDATE without WHERE ──

#[test]
fn test_update_without_where() {
    let (_statements, suggestions) = test_rewrite("UPDATE users SET name = 'x'");
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].rule_id, "reject-no-where-dml");
    match &suggestions[0].action {
        RewriteAction::Suggest { message, severity } => {
            assert!(
                message.contains("UPDATE"),
                "Message should mention UPDATE: {}",
                message
            );
            assert_eq!(*severity, Severity::Critical);
        }
        _ => panic!("Expected Suggest action"),
    }
}

// ── DML with WHERE (no suggestions) ──

#[test]
fn test_delete_with_where() {
    let (_statements, suggestions) = test_rewrite("DELETE FROM users WHERE id = 1");
    assert!(
        suggestions.is_empty(),
        "DELETE with WHERE should not produce suggestions"
    );
}

#[test]
fn test_update_with_where() {
    let (_statements, suggestions) = test_rewrite("UPDATE users SET name = 'x' WHERE id = 1");
    assert!(
        suggestions.is_empty(),
        "UPDATE with WHERE should not produce suggestions"
    );
}

// ── Non-DML (no suggestions) ──

#[test]
fn test_select_statement() {
    let (_statements, suggestions) = test_rewrite("SELECT * FROM users");
    assert!(
        suggestions.is_empty(),
        "SELECT should not produce suggestions"
    );
}
