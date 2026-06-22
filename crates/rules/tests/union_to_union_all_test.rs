use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry, Suggestion};
use metamorphosis_rules::union_to_union_all::UnionToUnionAll;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::Parser;

fn test_rewrite(sql: &str) -> (Vec<Statement>, Vec<Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(UnionToUnionAll)]));
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
fn test_union_converts_to_union_all() {
    let (statements, suggestions) = test_rewrite("SELECT 1 UNION SELECT 2");
    assert_eq!(statements.len(), 1, "Should have one statement");

    let sql = SqlFormatter::new().format_statement(&statements[0]);
    assert!(
        sql.contains("UNION ALL"),
        "UNION should be converted to UNION ALL, got: {}",
        sql
    );
    // Empty suggestions because this is a Safe rule (applies Replace directly)
    assert!(
        suggestions.is_empty(),
        "Safe rules should not produce suggestions"
    );
}

#[test]
fn test_union_all_stays_unchanged() {
    let (statements, suggestions) = test_rewrite("SELECT 1 UNION ALL SELECT 2");
    assert_eq!(statements.len(), 1, "Should have one statement");

    let sql = SqlFormatter::new().format_statement(&statements[0]);
    assert!(
        sql.contains("UNION ALL"),
        "UNION ALL should remain, got: {}",
        sql
    );
    // No rewrite happened since it was already UNION ALL
    assert!(
        suggestions.is_empty(),
        "No suggestions should be produced for unchanged UNION ALL"
    );
}

#[test]
fn test_intersect_not_affected() {
    let (statements, suggestions) = test_rewrite("SELECT 1 INTERSECT SELECT 2");
    assert_eq!(statements.len(), 1, "Should have one statement");

    let sql = SqlFormatter::new().format_statement(&statements[0]);
    assert!(
        sql.contains("INTERSECT"),
        "INTERSECT should remain unchanged, got: {}",
        sql
    );
    assert!(
        !sql.contains("UNION"),
        "INTERSECT should not mention UNION, got: {}",
        sql
    );
    assert!(suggestions.is_empty(), "No suggestions for INTERSECT");
}

#[test]
fn test_except_not_affected() {
    let (statements, suggestions) = test_rewrite("SELECT 1 EXCEPT SELECT 2");
    assert_eq!(statements.len(), 1, "Should have one statement");

    let sql = SqlFormatter::new().format_statement(&statements[0]);
    assert!(
        sql.contains("EXCEPT"),
        "EXCEPT should remain unchanged, got: {}",
        sql
    );
    assert!(
        !sql.contains("UNION"),
        "EXCEPT should not mention UNION, got: {}",
        sql
    );
    assert!(suggestions.is_empty(), "No suggestions for EXCEPT");
}

#[test]
fn test_chained_union_becomes_all() {
    // Both UNIONs should become UNION ALL (engine re-runs for the second)
    let (statements, suggestions) = test_rewrite("SELECT 1 UNION SELECT 2 UNION SELECT 3");
    assert_eq!(statements.len(), 1, "Should have one statement");

    let sql = SqlFormatter::new().format_statement(&statements[0]);
    assert!(
        sql.contains("UNION ALL"),
        "All UNIONs should be converted to UNION ALL, got: {}",
        sql
    );
    assert!(
        suggestions.is_empty(),
        "No suggestions for chained union rewrite"
    );
}

#[test]
fn test_non_select_statement_untouched() {
    // Non-SELECT statements should not be affected
    let (statements, suggestions) = test_rewrite("CREATE TABLE t (a INT)");
    assert_eq!(statements.len(), 1, "Should have one statement");
    assert!(
        suggestions.is_empty(),
        "No suggestions for non-SELECT statement"
    );
}
