use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry, Suggestion};
use metamorphosis_rules::eliminate_select_star::EliminateSelectStar;
use ogsql_parser::analyzer::schema::SchemaMap;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::Parser;
use std::collections::HashMap;

fn make_schema() -> SchemaMap {
    let mut cols = HashMap::new();
    cols.insert("id".to_string(), "integer".to_string());
    cols.insert("name".to_string(), "varchar".to_string());
    cols.insert("email".to_string(), "varchar".to_string());
    let mut schema = SchemaMap::new();
    schema.insert("users".to_string(), cols);
    schema
}

fn test_rewrite(sql: &str, schema: &SchemaMap) -> (Vec<Statement>, Vec<Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(EliminateSelectStar)]));
    let config = RewriteConfig::default();
    let ctx = RewriteContext {
        version: None,
        schema: Some(schema),
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
fn test_expand_select_star() {
    let (statements, _suggestions) = test_rewrite("SELECT * FROM users", &make_schema());
    assert_eq!(statements.len(), 1);

    let sql = SqlFormatter::new().format_statement(&statements[0]);
    assert!(sql.contains("id"), "Should contain id column, got: {}", sql);
    assert!(
        sql.contains("name"),
        "Should contain name column, got: {}",
        sql
    );
    assert!(
        sql.contains("email"),
        "Should contain email column, got: {}",
        sql
    );
    assert!(
        !sql.contains('*'),
        "Should not contain wildcard, got: {}",
        sql
    );
}

#[test]
fn test_no_star_no_change() {
    let (statements, _suggestions) = test_rewrite("SELECT id, name FROM users", &make_schema());
    let sql = SqlFormatter::new().format_statement(&statements[0]);
    assert!(sql.contains("id"));
    assert!(sql.contains("name"));
}

#[test]
fn test_no_schema_no_match() {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(EliminateSelectStar)]));
    let config = RewriteConfig::default();
    let ctx = RewriteContext {
        version: None,
        schema: None,
        config: &config,
        source_file: None,
        known_variables: None,
        diagnostic_hints: None,
    };

    let (stmts, _errors) = Parser::parse_sql("SELECT * FROM users");
    let statements: Vec<Statement> = stmts.into_iter().map(|si| si.statement).collect();

    let result = engine.rewrite(&ctx, statements);
    let sql = SqlFormatter::new().format_statement(&result.statements[0]);
    assert!(
        sql.contains('*'),
        "Without schema, SELECT * should remain: {}",
        sql
    );
}
