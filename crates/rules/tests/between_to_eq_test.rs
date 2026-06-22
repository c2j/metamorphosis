use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry, Suggestion};
use metamorphosis_rules::between_to_eq::BetweenToEq;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::Parser;

fn test_rewrite(sql: &str) -> (Vec<Statement>, Vec<Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(BetweenToEq)]));
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

fn format_first(statements: &[Statement]) -> String {
    SqlFormatter::new().format_statement(&statements[0])
}

#[test]
fn test_between_eq_same_integer() {
    let sql = "SELECT * FROM t WHERE col BETWEEN 5 AND 5";
    let (statements, _suggestions) = test_rewrite(sql);
    let result = format_first(&statements);
    let upper = result.to_uppercase();

    assert!(
        !upper.contains("BETWEEN"),
        "BETWEEN should be removed, got: {}",
        result
    );
    assert!(
        result.contains("= 5"),
        "Should contain = 5, got: {}",
        result
    );
}

#[test]
fn test_between_different_bounds_no_change() {
    let sql = "SELECT * FROM t WHERE col BETWEEN 1 AND 10";
    let (statements, _suggestions) = test_rewrite(sql);
    let result = format_first(&statements);
    let upper = result.to_uppercase();

    assert!(
        upper.contains("BETWEEN"),
        "BETWEEN should be preserved, got: {}",
        result
    );
}

#[test]
fn test_not_between_no_change() {
    let sql = "SELECT * FROM t WHERE col NOT BETWEEN 5 AND 5";
    let (statements, _suggestions) = test_rewrite(sql);
    let result = format_first(&statements);
    let upper = result.to_uppercase();

    assert!(
        upper.contains("NOT BETWEEN"),
        "NOT BETWEEN should be preserved, got: {}",
        result
    );
}

#[test]
fn test_between_eq_string() {
    let sql = "SELECT * FROM t WHERE col BETWEEN 'a' AND 'a'";
    let (statements, _suggestions) = test_rewrite(sql);
    let result = format_first(&statements);
    let upper = result.to_uppercase();

    assert!(
        !upper.contains("BETWEEN"),
        "BETWEEN should be removed, got: {}",
        result
    );
    assert!(
        result.contains("= 'a'"),
        "Should contain = 'a', got: {}",
        result
    );
}

#[test]
fn test_no_where_no_match() {
    let sql = "SELECT * FROM t";
    let (statements, _suggestions) = test_rewrite(sql);
    let result = format_first(&statements);
    let upper = result.to_uppercase();

    assert!(
        upper.contains("SELECT"),
        "Should contain SELECT, got: {}",
        result
    );
    assert!(
        !upper.contains("BETWEEN"),
        "Should not contain BETWEEN, got: {}",
        result
    );
}

#[test]
fn test_compound_where_with_between() {
    let sql = "SELECT * FROM t WHERE (col BETWEEN 5 AND 5) AND other = 1";
    let (statements, _suggestions) = test_rewrite(sql);
    let result = format_first(&statements);
    let upper = result.to_uppercase();

    assert!(
        !upper.contains("BETWEEN"),
        "BETWEEN should be removed from compound WHERE, got: {}",
        result
    );
    assert!(
        result.contains("= 5") || result.contains("=5"),
        "Should contain = 5 (from BETWEEN replacement), got: {}",
        result
    );
    assert!(
        result.contains("= 1") || result.contains("=1"),
        "Should preserve other = 1, got: {}",
        result
    );
}
