use metamorphosis_core::types::RewriteAction;
use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry, Suggestion};
use metamorphosis_rules::probe_join_integrity::ProbeJoinIntegrity;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::Parser;

fn test_rewrite(sql: &str) -> (Vec<Statement>, Vec<Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(ProbeJoinIntegrity)]));
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
fn test_inner_join_generates_probe() {
    let (_statements, suggestions) = test_rewrite("SELECT * FROM a JOIN b ON a.id = b.aid");

    assert!(
        !suggestions.is_empty(),
        "Should generate probe for JOIN query"
    );

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
                upper.contains("MATCHED"),
                "Probe should have MATCHED alias: {}",
                probe
            );
        }
        _ => panic!("Expected Generate action"),
    }
}

#[test]
fn test_left_join_generates_probe() {
    let (_statements, suggestions) = test_rewrite("SELECT * FROM a LEFT JOIN b ON a.id = b.aid");

    assert!(!suggestions.is_empty(), "Should match LEFT JOIN");
}

#[test]
fn test_no_join_no_match() {
    let (_statements, suggestions) = test_rewrite("SELECT * FROM a");

    assert!(
        suggestions.is_empty(),
        "Should not match single-table query"
    );
}

#[test]
fn test_multiple_joins_multiple_probes() {
    let (_statements, suggestions) =
        test_rewrite("SELECT * FROM a JOIN b ON a.id = b.aid JOIN c ON b.id = c.bid");

    assert!(
        suggestions.len() >= 2,
        "Should generate at least 2 probes for 2 JOINs, got {}",
        suggestions.len()
    );
}
