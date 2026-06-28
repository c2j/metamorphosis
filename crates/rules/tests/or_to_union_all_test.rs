use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry, Suggestion};
use metamorphosis_rules::or_to_union_all::OrToUnionAll;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::Parser;

fn test_rewrite(sql: &str) -> (Vec<Statement>, Vec<Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(OrToUnionAll)]));
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

#[test]
fn test_top_level_or_becomes_union_all() {
    let (statements, _) = test_rewrite("SELECT * FROM t WHERE a = 1 OR b = 2");
    let sql = SqlFormatter::new().format_statement(&statements[0]);
    let upper = sql.to_uppercase();
    assert!(
        upper.contains("UNION ALL"),
        "Expected UNION ALL, got: {}",
        sql
    );
}

#[test]
fn test_and_not_matched() {
    let (statements, _) = test_rewrite("SELECT * FROM t WHERE a = 1 AND b = 2");
    let sql = SqlFormatter::new().format_statement(&statements[0]);
    let upper = sql.to_uppercase();
    assert!(
        !upper.contains("UNION ALL"),
        "AND should not produce UNION ALL: {}",
        sql
    );
}

#[test]
fn test_no_where_no_match() {
    let (statements, _) = test_rewrite("SELECT * FROM t WHERE a = 1");
    let sql = SqlFormatter::new().format_statement(&statements[0]);
    let upper = sql.to_uppercase();
    assert!(
        !upper.contains("UNION ALL"),
        "Single condition should not match: {}",
        sql
    );
}

#[test]
fn test_distinct_blocks_rewrite() {
    let (statements, _) = test_rewrite("SELECT DISTINCT * FROM t WHERE a = 1 OR b = 2");
    let sql = SqlFormatter::new().format_statement(&statements[0]);
    let upper = sql.to_uppercase();
    assert!(
        !upper.contains("UNION ALL"),
        "DISTINCT should block rewrite: {}",
        sql
    );
    assert!(
        upper.contains("DISTINCT"),
        "Should preserve DISTINCT: {}",
        sql
    );
}

#[test]
fn test_join_blocks_rewrite() {
    let (statements, _) =
        test_rewrite("SELECT * FROM t1 JOIN t2 ON t1.id = t2.id WHERE t1.a = 1 OR t2.b = 2");
    let sql = SqlFormatter::new().format_statement(&statements[0]);
    let upper = sql.to_uppercase();
    assert!(
        !upper.contains("UNION ALL"),
        "JOIN should block rewrite: {}",
        sql
    );
}
