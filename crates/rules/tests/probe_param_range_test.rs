use metamorphosis_core::types::RewriteAction;
use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry, Suggestion};
use metamorphosis_rules::probe_param_range::ProbeParamRange;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::Parser;
use std::collections::HashSet;

fn test_rewrite_with_vars(
    sql: &str,
    known_variables: HashSet<String>,
) -> (Vec<Statement>, Vec<Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(ProbeParamRange)]));
    let config = RewriteConfig::default();
    let ctx = RewriteContext {
        version: None,
        schema: None,
        config: &config,
        source_file: None,
        known_variables: Some(&known_variables),
    };
    let (stmts, _errors) = Parser::parse_sql(sql);
    let statements: Vec<Statement> = stmts.into_iter().map(|si| si.statement).collect();
    let result = engine.rewrite(&ctx, statements);
    (result.statements, result.suggestions)
}

#[test]
fn test_param_eq_generates_range_probe() {
    let mut vars = HashSet::new();
    vars.insert("p_status".to_string());

    let (_statements, suggestions) = test_rewrite_with_vars(
        "SELECT * FROM t WHERE status = p_status AND type = 'A'",
        vars,
    );

    assert_eq!(suggestions.len(), 1);

    match &suggestions[0].action {
        RewriteAction::Generate { stmt, .. } => {
            let probe = SqlFormatter::new().format_statement(stmt);
            let upper = probe.to_uppercase();
            assert!(upper.contains("MIN("), "Probe should have MIN: {}", probe);
            assert!(upper.contains("MAX("), "Probe should have MAX: {}", probe);
            assert!(
                upper.contains("COUNT(DISTINCT"),
                "Probe should have COUNT(DISTINCT): {}",
                probe
            );
            assert!(
                upper.contains("COUNT"),
                "Probe should have COUNT: {}",
                probe
            );
        }
        _ => panic!("Expected Generate action"),
    }
}

#[test]
fn test_no_param_eq_no_match() {
    let vars = HashSet::new();

    let (_statements, suggestions) = test_rewrite_with_vars("SELECT * FROM t WHERE id = 1", vars);

    assert!(
        suggestions.is_empty(),
        "Should not match without parameterized equality"
    );
}

#[test]
fn test_no_where_no_match() {
    let vars = HashSet::new();

    let (_statements, suggestions) = test_rewrite_with_vars("SELECT * FROM t", vars);

    assert!(suggestions.is_empty(), "Should not match without WHERE");
}

#[test]
fn test_multiple_param_columns() {
    let mut vars = HashSet::new();
    vars.insert("p_status".to_string());
    vars.insert("p_type".to_string());

    let (_statements, suggestions) = test_rewrite_with_vars(
        "SELECT * FROM t WHERE status = p_status AND type = p_type",
        vars,
    );

    assert_eq!(suggestions.len(), 1);

    match &suggestions[0].action {
        RewriteAction::Generate { stmt, .. } => {
            let probe = SqlFormatter::new().format_statement(stmt);
            assert!(
                probe.contains("status"),
                "Probe should reference status: {}",
                probe
            );
            assert!(
                probe.contains("type"),
                "Probe should reference type: {}",
                probe
            );
        }
        _ => panic!("Expected Generate action"),
    }
}
