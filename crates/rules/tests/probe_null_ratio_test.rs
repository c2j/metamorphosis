use metamorphosis_core::types::RewriteAction;
use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry, Suggestion};
use metamorphosis_rules::probe_null_ratio::ProbeNullRatio;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::Parser;

fn test_rewrite(sql: &str) -> (Vec<Statement>, Vec<Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(ProbeNullRatio)]));
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

/// Two columns in WHERE → 1 suggestion with COUNT(*), COUNT(col1), COUNT(col2)
#[test]
fn test_where_columns_generate_null_probe() {
    let (_statements, suggestions) = test_rewrite("SELECT * FROM t WHERE col1 = 1 AND col2 = 2");

    assert_eq!(suggestions.len(), 1);

    match &suggestions[0].action {
        RewriteAction::Generate { stmt, .. } => {
            let probe = SqlFormatter::new().format_statement(stmt);
            let upper = probe.to_uppercase();
            assert!(
                upper.contains("COUNT"),
                "Probe should have COUNT: {}",
                probe
            );
            assert!(
                upper.contains("TOTAL"),
                "Probe should have TOTAL alias: {}",
                probe
            );
            assert!(
                probe.contains("col1"),
                "Probe should reference col1: {}",
                probe
            );
            assert!(
                probe.contains("col2"),
                "Probe should reference col2: {}",
                probe
            );
            assert!(
                probe.contains("col1_non_null"),
                "Probe should have col1_non_null alias: {}",
                probe
            );
            assert!(
                probe.contains("col2_non_null"),
                "Probe should have col2_non_null alias: {}",
                probe
            );
        }
        _ => panic!("Expected Generate action"),
    }
}

/// Single column in WHERE → 1 suggestion with COUNT(*) and COUNT(col1)
#[test]
fn test_single_column_where() {
    let (_statements, suggestions) = test_rewrite("SELECT * FROM t WHERE status = 1");

    assert_eq!(suggestions.len(), 1);

    match &suggestions[0].action {
        RewriteAction::Generate { stmt, .. } => {
            let probe = SqlFormatter::new().format_statement(stmt);
            let upper = probe.to_uppercase();
            assert!(
                upper.contains("TOTAL"),
                "Probe should have TOTAL: {}",
                probe
            );
            assert!(
                probe.contains("status"),
                "Probe should reference status: {}",
                probe
            );
            assert!(
                probe.contains("status_non_null"),
                "Probe should have status_non_null alias: {}",
                probe
            );
        }
        _ => panic!("Expected Generate action"),
    }
}

/// No WHERE clause → no suggestions
#[test]
fn test_no_where_no_match() {
    let (_statements, suggestions) = test_rewrite("SELECT * FROM t");
    assert!(suggestions.is_empty(), "Should not match without WHERE");
}

/// Non-SELECT statement (DELETE) → no match
#[test]
fn test_non_select_no_match() {
    let (_statements, suggestions) = test_rewrite("DELETE FROM t WHERE x = 1");
    assert!(
        suggestions.is_empty(),
        "DELETE should not match, SELECT only"
    );
}

/// Qualified column references (table.column)
#[test]
fn test_qualified_columns() {
    let (_statements, suggestions) =
        test_rewrite("SELECT * FROM orders o WHERE o.status = 'active' AND o.amount > 100");
    assert_eq!(
        suggestions.len(),
        1,
        "Expected suggestion for qualified columns"
    );

    match &suggestions[0].action {
        RewriteAction::Generate { stmt, .. } => {
            let probe = SqlFormatter::new().format_statement(stmt);
            assert!(
                probe.contains("o.status"),
                "Probe should reference o.status: {}",
                probe
            );
            assert!(
                probe.contains("o.amount"),
                "Probe should reference o.amount: {}",
                probe
            );
        }
        _ => panic!("Expected Generate action"),
    }
}

/// Duplicate column references should be deduplicated
#[test]
fn test_duplicate_columns_deduplicated() {
    let (_statements, suggestions) =
        test_rewrite("SELECT * FROM t WHERE col1 = 1 AND col1 IS NULL");
    assert_eq!(suggestions.len(), 1);

    match &suggestions[0].action {
        RewriteAction::Generate { stmt, .. } => {
            let probe = SqlFormatter::new().format_statement(stmt);
            // col1 should appear at most: once in COUNT(col1) and once in alias
            let col1_occurrences = probe.matches("col1").count();
            assert!(
                col1_occurrences <= 2,
                "col1 should appear at most twice (COUNT + alias), got {}: {}",
                col1_occurrences,
                probe
            );
        }
        _ => panic!("Expected Generate action"),
    }
}

/// WHERE with BETWEEN
#[test]
fn test_between_columns() {
    let (_statements, suggestions) =
        test_rewrite("SELECT * FROM t WHERE t.date_col BETWEEN '2020-01-01' AND '2020-12-31'");
    assert_eq!(
        suggestions.len(),
        1,
        "Expected suggestion for BETWEEN condition"
    );

    match &suggestions[0].action {
        RewriteAction::Generate { stmt, .. } => {
            let probe = SqlFormatter::new().format_statement(stmt);
            assert!(
                probe.contains("date_col"),
                "Probe should reference date_col: {}",
                probe
            );
        }
        _ => panic!("Expected Generate action"),
    }
}

/// WHERE with LIKE
#[test]
fn test_like_columns() {
    let (_statements, suggestions) = test_rewrite("SELECT * FROM t WHERE t.name LIKE '%test%'");
    assert_eq!(
        suggestions.len(),
        1,
        "Expected suggestion for LIKE condition"
    );

    match &suggestions[0].action {
        RewriteAction::Generate { stmt, .. } => {
            let probe = SqlFormatter::new().format_statement(stmt);
            assert!(
                probe.contains("name"),
                "Probe should reference name: {}",
                probe
            );
        }
        _ => panic!("Expected Generate action"),
    }
}

/// Function-wrapped columns
#[test]
fn test_function_call_columns() {
    let (_statements, suggestions) = test_rewrite("SELECT * FROM t WHERE UPPER(t.name) = 'TEST'");
    assert_eq!(
        suggestions.len(),
        1,
        "Expected suggestion for function-wrapped column"
    );

    match &suggestions[0].action {
        RewriteAction::Generate { stmt, .. } => {
            let probe = SqlFormatter::new().format_statement(stmt);
            assert!(
                probe.contains("name"),
                "Probe should reference name inside function: {}",
                probe
            );
        }
        _ => panic!("Expected Generate action"),
    }
}

/// JOIN condition columns combined with WHERE
#[test]
fn test_join_condition_columns() {
    let (_statements, suggestions) = test_rewrite(
        "SELECT * FROM orders o JOIN users u ON o.user_id = u.id WHERE o.status = 'active'",
    );
    assert_eq!(suggestions.len(), 1, "Expected suggestion for JOIN + WHERE");

    match &suggestions[0].action {
        RewriteAction::Generate { stmt, .. } => {
            let probe = SqlFormatter::new().format_statement(stmt);
            assert!(
                probe.contains("status"),
                "Probe should reference status: {}",
                probe
            );
            assert!(
                probe.contains("user_id") && probe.contains("id"),
                "Probe should reference JOIN columns: {}",
                probe
            );
            assert!(
                probe.contains("user_id_non_null") || probe.contains("id_non_null"),
                "Probe should have non_null aliases: {}",
                probe
            );
        }
        _ => panic!("Expected Generate action"),
    }
}
