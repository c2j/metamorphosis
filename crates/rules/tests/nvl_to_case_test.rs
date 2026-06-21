use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry, Suggestion};
use metamorphosis_rules::nvl_to_case::NvlToCase;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::Parser;

fn test_rewrite(sql: &str) -> (Vec<Statement>, Vec<Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(NvlToCase)]));
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
fn test_nvl_in_target_replaced() {
    let (statements, _) = test_rewrite("SELECT NVL(col, 0) FROM t");
    let sql = SqlFormatter::new().format_statement(&statements[0]);
    let upper = sql.to_uppercase();
    assert!(upper.contains("CASE"), "Expected CASE, got: {}", sql);
    assert!(upper.contains("IS NULL"), "Expected IS NULL, got: {}", sql);
    assert!(upper.contains("ELSE"), "Expected ELSE, got: {}", sql);
    assert!(!upper.contains("NVL"), "Should not contain NVL, got: {}", sql);
}

#[test]
fn test_nvl_in_where_replaced() {
    let (statements, _) = test_rewrite("SELECT * FROM t WHERE NVL(status, 'X') = 'Y'");
    let sql = SqlFormatter::new().format_statement(&statements[0]);
    let upper = sql.to_uppercase();
    assert!(upper.contains("CASE"), "Expected CASE in WHERE, got: {}", sql);
    assert!(!upper.contains("NVL"), "Should not contain NVL, got: {}", sql);
}

#[test]
fn test_lowercase_nvl_matched() {
    let (statements, _) = test_rewrite("SELECT nvl(col, 0) FROM t");
    let sql = SqlFormatter::new().format_statement(&statements[0]);
    let upper = sql.to_uppercase();
    assert!(upper.contains("CASE"), "Lowercase nvl should be matched: {}", sql);
}

#[test]
fn test_no_nvl_no_match() {
    let (statements, _) = test_rewrite("SELECT MAX(col) FROM t");
    let sql = SqlFormatter::new().format_statement(&statements[0]);
    let upper = sql.to_uppercase();
    assert!(!upper.contains("CASE"), "Should not add CASE without NVL: {}", sql);
}

#[test]
fn test_nested_nvl_replaced() {
    let (statements, _) = test_rewrite("SELECT col1, NVL(col2, 'default') FROM t");
    let sql = SqlFormatter::new().format_statement(&statements[0]);
    let upper = sql.to_uppercase();
    assert!(upper.contains("CASE"), "Expected CASE for nested NVL: {}", sql);
    assert!(!upper.contains("NVL"), "Should not contain NVL: {}", sql);
}
