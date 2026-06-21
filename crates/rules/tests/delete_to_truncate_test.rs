use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry, Suggestion};
use metamorphosis_rules::delete_to_truncate::DeleteToTruncate;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::Parser;

fn test_rewrite(sql: &str) -> (Vec<Statement>, Vec<Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(DeleteToTruncate)]));
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
fn test_delete_without_where_becomes_truncate() {
    let (statements, _suggestions) = test_rewrite("DELETE FROM users");

    let sql = SqlFormatter::new().format_statement(&statements[0]);
    let upper = sql.to_uppercase();
    assert!(upper.contains("TRUNCATE"), "Expected TRUNCATE, got: {}", sql);
    assert!(upper.contains("USERS"), "Should reference users table: {}", sql);
}

#[test]
fn test_delete_with_where_no_change() {
    let (statements, _suggestions) = test_rewrite("DELETE FROM users WHERE id = 1");

    let sql = SqlFormatter::new().format_statement(&statements[0]);
    let upper = sql.to_uppercase();
    assert!(!upper.contains("TRUNCATE"), "Should not TRUNCATE with WHERE: {}", sql);
    assert!(upper.contains("DELETE"), "Should remain DELETE: {}", sql);
}

#[test]
fn test_delete_with_returning_no_change() {
    let (statements, _suggestions) = test_rewrite("DELETE FROM users RETURNING *");

    let sql = SqlFormatter::new().format_statement(&statements[0]);
    let upper = sql.to_uppercase();
    assert!(!upper.contains("TRUNCATE"), "Should not TRUNCATE with RETURNING: {}", sql);
}

#[test]
fn test_select_no_match() {
    let (statements, _suggestions) = test_rewrite("SELECT * FROM users");

    let sql = SqlFormatter::new().format_statement(&statements[0]);
    let upper = sql.to_uppercase();
    assert!(!upper.contains("TRUNCATE"), "SELECT should not match: {}", sql);
}
