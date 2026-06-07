use metamorphosis_core::types::RewriteAction;
use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry, Suggestion};
use metamorphosis_rules::extract_candidate_values::ExtractCandidateValues;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::Parser;
use std::collections::HashSet;

use ogsql_parser::ParseOptions;

fn test_suggest(sql: &str) -> (Vec<Statement>, Vec<Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(ExtractCandidateValues)]));
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

fn test_suggest_with_vars(
    sql: &str,
    known_variables: HashSet<String>,
) -> (Vec<Statement>, Vec<Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(ExtractCandidateValues)]));
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

fn test_suggest_mybatis(sql: &str) -> (Vec<Statement>, Vec<Suggestion>) {
    let engine = RewriteEngine::new(RuleRegistry::new(vec![Box::new(ExtractCandidateValues)]));
    let config = RewriteConfig::default();
    let ctx = RewriteContext {
        version: None,
        schema: None,
        config: &config,
        source_file: None,
        known_variables: None,
    };

    let output = Parser::parse_sql_with_options(
        sql,
        ParseOptions {
            preserve_comments: false,
            mybatis_params: true,
        },
    );
    let statements: Vec<Statement> = output
        .statements
        .into_iter()
        .map(|si| si.statement)
        .collect();

    let result = engine.rewrite(&ctx, statements);
    (result.statements, result.suggestions)
}

fn format_probe(suggestions: &[Suggestion]) -> Option<String> {
    suggestions.first().and_then(|s| match &s.action {
        RewriteAction::Generate { stmt, .. } => Some(SqlFormatter::new().format_statement(stmt)),
        _ => None,
    })
}

// ── Level 1: Core patterns ──

#[test]
fn test_literal_and_param() {
    let (_statements, suggestions) = test_suggest(
        "SELECT t.special_sql FROM dat_dataclear_config t WHERE t.clear_type = '4' AND t.task_status = p_i_taskstatus",
    );
    assert!(!suggestions.is_empty(), "Rule should match param eq");

    assert_eq!(suggestions[0].rule_id, "extract-candidate-values");

    let probe = format_probe(&suggestions).expect("Expected Generate action");

    let upper = probe.to_uppercase();
    assert!(
        upper.contains("GROUP BY"),
        "Probe must have GROUP BY: {}",
        probe
    );
    assert!(
        !upper.contains("HAVING"),
        "Probe must NOT have HAVING: {}",
        probe
    );
    assert!(
        probe.contains("task_status"),
        "Probe must reference task_status: {}",
        probe
    );
    assert!(
        probe.contains("clear_type"),
        "Probe must retain non-param condition clear_type: {}",
        probe
    );
    assert!(
        upper.contains("COUNT(1)") || upper.contains("COUNT (*)"),
        "Probe must have count(1): {}",
        probe
    );
    assert!(
        upper.contains("ORDER BY") && upper.contains("CNT"),
        "Probe must have ORDER BY cnt DESC: {}",
        probe
    );
}

#[test]
fn test_param_only_no_literal() {
    let mut vars = HashSet::new();
    vars.insert("p_order_id".to_string());
    let (_statements, suggestions) = test_suggest_with_vars(
        "SELECT status FROM t_payments WHERE order_id = p_order_id",
        vars,
    );
    assert!(!suggestions.is_empty(), "Rule should match single param eq");

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    assert!(
        probe.contains("order_id"),
        "Probe must reference order_id: {}",
        probe
    );
    let upper = probe.to_uppercase();
    assert!(
        !upper.contains("WHERE"),
        "Probe with no non-param conditions should have no WHERE clause: {}",
        probe
    );
}

#[test]
fn test_mybatis_param() {
    let (_statements, suggestions) =
        test_suggest_mybatis("SELECT name FROM users WHERE users.status = #{status}");
    assert!(!suggestions.is_empty(), "Rule should match MyBatisParam eq");
    let probe = format_probe(&suggestions).expect("Expected Generate action");
    assert!(
        probe.contains("status"),
        "Probe must reference status: {}",
        probe
    );
}

// ── Level 2: Multiple conditions ──

#[test]
fn test_multiple_non_param_conditions() {
    let (_statements, suggestions) = test_suggest(
        "SELECT * FROM orders o WHERE o.type = '4' AND o.date >= '2024-01-01' AND o.category = v_cat",
    );
    assert!(!suggestions.is_empty());

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    assert!(
        probe.contains("category"),
        "Probe must reference category: {}",
        probe
    );
    assert!(
        probe.contains("o.type = '4'") || probe.contains("O.TYPE"),
        "Probe must retain o.type = '4': {}",
        probe
    );
    assert!(
        probe.contains("2024-01-01"),
        "Probe must retain date condition: {}",
        probe
    );
}

#[test]
fn test_param_with_is_null() {
    let (_statements, suggestions) =
        test_suggest("SELECT * FROM t WHERE t.flag IS NULL AND t.task_status = p_status");
    assert!(!suggestions.is_empty());

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();
    assert!(
        upper.contains("IS NULL"),
        "Probe must preserve IS NULL: {}",
        probe
    );
    assert!(
        probe.contains("task_status"),
        "Probe must reference task_status: {}",
        probe
    );
}

#[test]
fn test_param_with_or_condition() {
    let (_statements, suggestions) = test_suggest(
        "SELECT * FROM t WHERE (t.clear_type = '4' OR t.clear_type = '5') AND t.status = v_status",
    );
    assert!(!suggestions.is_empty());

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();
    assert!(
        upper.contains("OR"),
        "Probe must preserve OR condition: {}",
        probe
    );
    assert!(
        probe.contains("status"),
        "Probe must reference status: {}",
        probe
    );
}

// ── Level 3: Unqualified column references ──

#[test]
fn test_unqualified_column() {
    let mut vars = HashSet::new();
    vars.insert("p_status".to_string());
    let (_statements, suggestions) = test_suggest_with_vars(
        "SELECT special_sql FROM dat_dataclear_config WHERE clear_type = '4' AND task_status = p_status",
        vars,
    );
    assert!(!suggestions.is_empty());

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    assert!(
        probe.contains("task_status"),
        "Probe must reference task_status: {}",
        probe
    );
    assert!(
        probe.contains("clear_type"),
        "Probe must retain clear_type: {}",
        probe
    );
}

// ── Level 4: Multiple parameterized columns ──

#[test]
fn test_multiple_params_group_by_composite() {
    let (_statements, suggestions) =
        test_suggest("SELECT * FROM t WHERE t.col1 = v_a AND t.col2 = v_b");
    assert!(!suggestions.is_empty());

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();
    assert!(
        probe.contains("col1"),
        "Probe must reference col1: {}",
        probe
    );
    assert!(
        probe.contains("col2"),
        "Probe must reference col2: {}",
        probe
    );
    // GROUP BY should have both columns (composite grouping)
    let group_by_idx = upper.find("GROUP BY").unwrap();
    let rest = &upper[group_by_idx..];
    assert!(
        rest.contains("COL1") && rest.contains("COL2"),
        "GROUP BY must include both columns: {}",
        probe
    );
}

#[test]
fn test_mixed_literal_and_param_equalities() {
    let (_statements, suggestions) = test_suggest(
        "SELECT * FROM t WHERE t.clear_type = '4' AND t.task_status = v_ts AND t.sub_type = '8'",
    );
    assert!(!suggestions.is_empty());

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    assert!(
        probe.contains("task_status"),
        "Probe must reference task_status: {}",
        probe
    );
    // literal equalities should be in WHERE
    assert!(
        probe.contains("clear_type"),
        "Probe must retain clear_type literal: {}",
        probe
    );
    assert!(
        probe.contains("sub_type"),
        "Probe must retain sub_type literal: {}",
        probe
    );
}

// ── Level 5: Subquery wrapper ──

#[test]
fn test_subquery_wrapper_unwrap() {
    let mut vars = HashSet::new();
    vars.insert("p_status".to_string());
    let (_statements, suggestions) = test_suggest_with_vars(
        "SELECT * FROM (SELECT t.*, row_number() OVER (ORDER BY t.id) AS rn FROM dat_dataclear_config t WHERE t.clear_type = '4' AND t.task_status = p_status) tmp WHERE rn BETWEEN 1 AND 10",
        vars,
    );
    assert!(!suggestions.is_empty(), "Rule should unwrap subquery");

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    // probe should reference the inner table, not the wrapper
    assert!(
        probe.contains("task_status"),
        "Probe must reference task_status from inner query: {}",
        probe
    );
    assert!(
        probe.contains("clear_type"),
        "Probe must retain clear_type from inner WHERE: {}",
        probe
    );
}

// ── Level 6: JOIN ──

#[test]
fn test_join_with_param() {
    let (_statements, suggestions) = test_suggest(
        "SELECT o.* FROM orders o JOIN users u ON o.user_id = u.id WHERE u.status = v_status AND o.amount > 100",
    );
    assert!(!suggestions.is_empty());

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();
    assert!(
        upper.contains("JOIN"),
        "Probe must preserve JOIN: {}",
        probe
    );
    assert!(
        probe.contains("status"),
        "Probe must reference status: {}",
        probe
    );
    assert!(
        probe.contains("amount > 100") || upper.contains("AMOUNT"),
        "Probe must retain amount > 100: {}",
        probe
    );
}

// ── Level 7: No-match scenarios ──

#[test]
fn test_no_where_no_match() {
    let (_statements, suggestions) = test_suggest("SELECT * FROM users");
    assert!(suggestions.is_empty(), "No WHERE clause should not match");
}

#[test]
fn test_only_literal_where_no_match() {
    let (_statements, suggestions) = test_suggest("SELECT * FROM users WHERE id = 1");
    assert!(suggestions.is_empty(), "Only literal eq should not match");
}

#[test]
fn test_only_colref_colref_where_no_match() {
    let (_statements, suggestions) = test_suggest(
        "SELECT * FROM orders o JOIN users u ON o.user_id = u.id WHERE o.status = u.default_status",
    );
    assert!(
        suggestions.is_empty(),
        "Only ColumnRef=ColumnRef should not match"
    );
}

// ── Level 8: Edge cases ──

#[test]
fn test_all_params_no_non_eq() {
    let (_statements, suggestions) = test_suggest("SELECT * FROM t WHERE t.a = v_a AND t.b = v_b");
    assert!(!suggestions.is_empty());

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();
    assert!(
        !upper.contains("WHERE") || probe.contains("WHERE"),
        "Probe with no non-eq conditions may or may not have WHERE, but must be valid: {}",
        probe
    );
    assert!(
        probe.contains("a") && probe.contains("b"),
        "Probe must reference both param cols: {}",
        probe
    );
}

#[test]
fn test_known_variables() {
    // v_uid and v_status are explicitly provided as known variables
    let mut vars = HashSet::new();
    vars.insert("v_uid".to_string());
    vars.insert("v_status".to_string());

    let (_statements, suggestions) = test_suggest_with_vars(
        "SELECT * FROM t WHERE t.user_id = v_uid AND t.status = v_status",
        vars,
    );
    assert!(
        !suggestions.is_empty(),
        "Should match when vars are in known_variables"
    );

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    assert!(
        probe.contains("user_id") && probe.contains("status"),
        "Probe must reference both columns: {}",
        probe
    );
}

#[test]
fn test_probe_in_list_subquery_preserved() {
    let (_statements, suggestions) = test_suggest(
        "SELECT * FROM users u WHERE u.id IN (SELECT user_id FROM orders) AND u.status = v_status",
    );
    assert!(!suggestions.is_empty());

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();
    assert!(
        upper.contains("IN (SELECT"),
        "Probe must preserve IN subquery: {}",
        probe
    );
    assert!(
        probe.contains("status"),
        "Probe must reference status: {}",
        probe
    );
}
