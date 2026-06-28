use metamorphosis_core::types::RewriteAction;
use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry, Suggestion};
use metamorphosis_rules::probe_data_skew::ProbeDataSkew;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::Parser;

fn test_rewrite(sql: &str) -> (Vec<Statement>, Vec<Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(ProbeDataSkew)]));
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
fn test_single_group_by_generates_probe() {
    let (_statements, suggestions) =
        test_rewrite("SELECT dept, COUNT(*) FROM employees GROUP BY dept");
    assert_eq!(suggestions.len(), 1);

    match &suggestions[0].action {
        RewriteAction::Generate { stmt, .. } => {
            let probe = SqlFormatter::new().format_statement(stmt);
            let upper = probe.to_uppercase();
            assert!(
                upper.contains("GROUP BY"),
                "Probe should have GROUP BY: {}",
                probe
            );
            assert!(
                upper.contains("ORDER BY"),
                "Probe should have ORDER BY: {}",
                probe
            );
            assert!(
                upper.contains("CNT"),
                "Probe should have CNT alias: {}",
                probe
            );
        }
        _ => panic!("Expected Generate action"),
    }
}

#[test]
fn test_multiple_group_by_columns() {
    let (_statements, suggestions) =
        test_rewrite("SELECT dept, role, COUNT(*) FROM employees GROUP BY dept, role");
    assert_eq!(suggestions.len(), 1);

    match &suggestions[0].action {
        RewriteAction::Generate { stmt, .. } => {
            let probe = SqlFormatter::new().format_statement(stmt);
            assert!(
                probe.contains("dept"),
                "Probe should reference dept: {}",
                probe
            );
            assert!(
                probe.contains("role"),
                "Probe should reference role: {}",
                probe
            );
        }
        _ => panic!("Expected Generate action"),
    }
}

#[test]
fn test_no_group_by_no_match() {
    let (_statements, suggestions) = test_rewrite("SELECT * FROM employees");
    assert!(suggestions.is_empty(), "Should not match without GROUP BY");
}

#[test]
fn test_select_without_aggregate_but_with_group_by() {
    let (_statements, suggestions) = test_rewrite("SELECT dept FROM employees GROUP BY dept");
    assert_eq!(suggestions.len(), 1, "Should match any GROUP BY query");
}
