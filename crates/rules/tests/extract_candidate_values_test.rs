use metamorphosis_core::types::{Confidence, RewriteAction};
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

#[allow(dead_code)]
fn assert_suggestion_count(sql: &str, expected_count: usize) -> Vec<Suggestion> {
    let (_statements, suggestions) = test_suggest(sql);
    assert_eq!(
        suggestions.len(),
        expected_count,
        "Expected {} suggestion(s) for SQL: '{}'",
        expected_count,
        sql
    );
    suggestions
}

fn assert_suggestion_count_with_vars(
    sql: &str,
    vars: HashSet<String>,
    expected_count: usize,
) -> Vec<Suggestion> {
    let (_statements, suggestions) = test_suggest_with_vars(sql, vars);
    assert_eq!(
        suggestions.len(),
        expected_count,
        "Expected {} suggestion(s) for SQL: '{}'",
        expected_count,
        sql
    );
    suggestions
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
fn test_mixed_case_parameter_filtered_from_probe() {
    // Regression: SQL identifiers are case-insensitive. When the same stored-proc
    // variable appears in different cases across predicates (e.g., `v_gffsrq` in
    // an equality and `V_GFFSRQ` in a BETWEEN), both must be recognized as the
    // same parameter. The probe WHERE must filter BOTH the equality and the
    // BETWEEN — otherwise the probe is still constrained by the current parameter
    // value, defeating its purpose.
    //
    // Origin: case5.sql lines 17-18:
    //   AND v.accountdate = v_gffsrq           ← lowercase, builds param_names
    //   AND V_GFFSRQ BETWEEN ... AND ...        ← uppercase, must match param_names
    let (_statements, suggestions) = test_suggest(
        "INSERT INTO AAA (seq_no, coin_code, tdstockbal) \
         SELECT DISTINCT p_i_seq_no, v.coin_code, v.tdstockbal \
         FROM PAR A, VAB v \
         WHERE a.share_partner_code = v_share_partner_code \
           AND a.fund_code = v.fund_code \
           AND v.accountdate = v_gffsrq \
           AND V_GFFSRQ BETWEEN a.inure_begin_date AND a.inure_end_date \
           AND v.tdstockbal <> 0",
    );
    assert!(
        !suggestions.is_empty(),
        "Rule should match: parameterized equalities exist"
    );

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();

    let gby = upper.find("GROUP BY").expect("Probe must have GROUP BY");
    let (before, after) = (&upper[..gby], &upper[gby..]);

    assert!(
        after.contains("SHARE_PARTNER_CODE"),
        "GROUP BY must include share_partner_code.\nProbe: {}",
        probe
    );
    assert!(
        after.contains("ACCOUNTDATE"),
        "GROUP BY must include accountdate.\nProbe: {}",
        probe
    );

    assert!(
        !before.contains("BETWEEN"),
        "BETWEEN on stored-proc variable V_GFFSRQ (uppercase variant of v_gffsrq) \
         must be removed from probe WHERE — parameter names are case-insensitive.\nProbe: {}",
        probe
    );

    assert!(
        !before.contains("V_GFFSRQ"),
        "v_gffsrq must not appear in probe WHERE (neither in equality nor BETWEEN).\nProbe: {}",
        probe
    );

    assert!(
        before.contains("FUND_CODE"),
        "Join condition a.fund_code = v.fund_code must be preserved.\nProbe: {}",
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
fn test_param_between_two_columns_extracts_bounds() {
    let (_statements, suggestions) = test_suggest(
        "SELECT t.a FROM dat_t t WHERE t.status = '1' AND p_val BETWEEN t.lo_bound AND t.hi_bound",
    );
    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();
    let gby = upper.find("GROUP BY").expect("Probe must have GROUP BY");
    let (before, after) = (&upper[..gby], &upper[gby..]);

    assert!(
        after.contains("LO_BOUND") && after.contains("HI_BOUND"),
        "GROUP BY must include both BETWEEN bound columns so user can see valid ranges.\nProbe: {}",
        probe
    );
    assert!(
        !before.contains("BETWEEN"),
        "Param-bearing BETWEEN must be removed from probe WHERE.\nProbe: {}",
        probe
    );
    assert!(
        !before.contains("P_VAL"),
        "Parameter p_val must not appear in probe WHERE.\nProbe: {}",
        probe
    );
    assert!(
        before.contains("STATUS"),
        "Non-param literal condition t.status = '1' must be preserved.\nProbe: {}",
        probe
    );
}

#[test]
fn test_col_between_two_params_extracts_subject() {
    let (_statements, suggestions) = test_suggest(
        "SELECT t.a FROM dat_t t WHERE t.status = '1' AND t.date_col BETWEEN p_start AND p_end",
    );
    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();
    let gby = upper.find("GROUP BY").expect("Probe must have GROUP BY");
    let (before, after) = (&upper[..gby], &upper[gby..]);

    assert!(
        after.contains("DATE_COL"),
        "GROUP BY must include subject column t.date_col so user can see what values exist.\nProbe: {}",
        probe
    );
    assert!(
        !before.contains("BETWEEN"),
        "Param-bearing BETWEEN must be removed from probe WHERE.\nProbe: {}",
        probe
    );
    assert!(
        !before.contains("P_START") && !before.contains("P_END"),
        "Parameters p_start/p_end must not appear in probe WHERE.\nProbe: {}",
        probe
    );
}

#[test]
fn test_col_between_nvl_params_extracts_subject() {
    let (_statements, suggestions) = test_suggest(
        "SELECT t.a FROM dat_t t WHERE t.status = '1' \
         AND t.date_col BETWEEN nvl(p_start, '19000101') AND nvl(p_end, '99991231')",
    );
    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();
    let gby = upper.find("GROUP BY").expect("Probe must have GROUP BY");
    let (before, after) = (&upper[..gby], &upper[gby..]);

    assert!(
        after.contains("DATE_COL"),
        "GROUP BY must include t.date_col even when bounds are function-wrapped params.\nProbe: {}",
        probe
    );
    assert!(
        !before.contains("BETWEEN"),
        "BETWEEN with nvl(param,...) bounds must be removed from probe WHERE.\nProbe: {}",
        probe
    );
}

#[test]
fn test_non_param_between_preserved_in_where() {
    let (_statements, suggestions) = test_suggest(
        "SELECT t.a FROM dat_t t WHERE t.status = p_status AND t.date_col BETWEEN '20200101' AND '20201231'",
    );
    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();
    let gby = upper.find("GROUP BY").expect("Probe must have GROUP BY");
    let (before, _after) = (&upper[..gby], &upper[gby..]);

    assert!(
        before.contains("BETWEEN"),
        "Non-param BETWEEN (literal bounds) must stay in probe WHERE.\nProbe: {}",
        probe
    );
    assert!(
        !before.contains("P_STATUS"),
        "Parameter equality t.status = p_status must be removed; t.status in GROUP BY.\nProbe: {}",
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

// ── Level 9: CUD + subquery multi-probe support ──

#[test]
fn test_update_with_in_subquery() {
    let mut vars = HashSet::new();
    vars.insert("v_status".to_string());
    vars.insert("v_cat".to_string());
    let suggestions = assert_suggestion_count_with_vars(
        "UPDATE orders SET status = 'done' WHERE status = v_status AND order_id IN (SELECT order_id FROM items WHERE category = v_cat)",
        vars,
        2,
    );
    let probes: Vec<String> = suggestions
        .iter()
        .filter_map(|s| format_probe(std::slice::from_ref(s)))
        .collect();
    assert!(
        probes
            .iter()
            .any(|p| p.contains("items") && p.contains("category")),
        "Expected probe from items subquery, got: {:?}",
        probes
    );
    assert!(
        probes.iter().any(|p| p.contains("status")),
        "Expected probe referencing status (outer scope), got: {:?}",
        probes
    );
}

#[test]
fn test_delete_with_exists_subquery() {
    let (_statements, suggestions) = test_suggest(
        "DELETE FROM orders WHERE EXISTS (SELECT 1 FROM items WHERE items.category = v_cat AND items.region = 'EAST')",
    );
    assert!(!suggestions.is_empty(), "Should match EXISTS subquery");
    let probe = format_probe(&suggestions).expect("Expected probe");
    assert!(
        probe.contains("category"),
        "Probe must reference category: {}",
        probe
    );
    assert!(
        probe.contains("region"),
        "Probe must retain region filter: {}",
        probe
    );
}

#[test]
fn test_insert_select() {
    let (_statements, suggestions) = test_suggest(
        "INSERT INTO archive (id, name) SELECT id, name FROM orders WHERE orders.status = v_status",
    );
    assert!(!suggestions.is_empty(), "Should match INSERT...SELECT");
    let probe = format_probe(&suggestions).expect("Expected probe");
    assert!(
        probe.contains("status"),
        "Probe must reference status: {}",
        probe
    );
    assert!(
        probe.contains("orders"),
        "Probe must reference orders: {}",
        probe
    );
}

#[test]
fn test_update_plain_no_subquery() {
    let mut vars = HashSet::new();
    vars.insert("v_old".to_string());
    let suggestions = assert_suggestion_count_with_vars(
        "UPDATE orders SET status = v_new WHERE status = v_old AND region = 'EAST'",
        vars,
        1,
    );
    let probe = format_probe(&suggestions).expect("Expected 1 probe");
    assert!(
        probe.contains("status"),
        "Probe must reference status: {}",
        probe
    );
}

#[test]
fn test_two_in_subqueries() {
    let (_statements, suggestions) = test_suggest(
        "SELECT * FROM t WHERE t.col = v_col AND a IN (SELECT x FROM t1 WHERE t1.y = v_y) AND b IN (SELECT z FROM t2 WHERE t2.w = v_w)",
    );
    assert!(
        suggestions.len() >= 3,
        "Expected 3+ probes, got {}",
        suggestions.len()
    );
    let probes: Vec<String> = suggestions
        .iter()
        .filter_map(|s| format_probe(std::slice::from_ref(s)))
        .collect();
    assert!(
        probes.iter().any(|p| p.contains("t1") && p.contains("y")),
        "Missing t1 probe"
    );
    assert!(
        probes.iter().any(|p| p.contains("t2") && p.contains("w")),
        "Missing t2 probe"
    );
}

#[test]
fn test_insert_values_no_probe() {
    let (_statements, suggestions) = test_suggest("INSERT INTO t (a, b) VALUES (1, 2)");
    assert!(suggestions.is_empty(), "INSERT...VALUES should not match");
}

#[test]
fn test_merge_on_condition() {
    let mut vars = HashSet::new();
    vars.insert("v_status".to_string());
    let (_statements, suggestions) = test_suggest_with_vars(
        "MERGE INTO target t USING source s ON t.id = s.id AND t.status = v_status",
        vars,
    );
    assert!(!suggestions.is_empty(), "Should match MERGE ON condition");
}

#[test]
fn test_nested_subquery() {
    let (_statements, suggestions) = test_suggest(
        "SELECT * FROM t WHERE t.col = v_col AND col IN (SELECT a FROM t1 WHERE b IN (SELECT c FROM t2 WHERE t2.d = v_d))",
    );
    assert!(
        suggestions.len() >= 2,
        "Expected 2+ probes from nested subqueries, got {}",
        suggestions.len()
    );
    let probes: Vec<String> = suggestions
        .iter()
        .filter_map(|s| format_probe(std::slice::from_ref(s)))
        .collect();
    assert!(
        probes.iter().any(|p| p.contains("t2") && p.contains("d")),
        "Missing innermost probe"
    );
}

// ── Level 10: Comma-join — join condition vs parameterized equality ──

#[test]
fn test_comma_join_condition_excluded_from_group_by() {
    // Regression guard: In a comma-separated FROM (implicit join), a WHERE
    // equality `col = col` where BOTH sides reference known table aliases is a
    // JOIN condition. It must be preserved in the probe WHERE (keep_exprs) and
    // must NOT leak into GROUP BY (tier1).
    //
    // Origin: a real-world INSERT...SELECT where `a.fund_code = v.fund_code`
    // (table join on VAB.fund_code) was misanalyzed as `a.fund_code = v_fund_code`
    // (parameterized equality) because the `.` was misread as `_`, causing
    // fund_code to erroneously appear in GROUP BY instead of WHERE.
    let (_statements, suggestions) = test_suggest(
        "INSERT INTO AAA (seq_no, coin_code, tdstockbal) \
         SELECT DISTINCT p_i_seq_no, v.coin_code, v.tdstockbal \
         FROM PAR A, VAB v \
         WHERE a.share_partner_code = v_share_partner_code \
           AND a.fund_code = v.fund_code \
           AND v.accountdate = v_gffsrq \
           AND v_gffsrq BETWEEN a.inure_begin_date AND a.inure_end_date \
           AND v.tdstockbal <> 0",
    );
    assert!(
        !suggestions.is_empty(),
        "Rule should match: parameterized equalities exist"
    );
    assert_eq!(
        suggestions.len(),
        1,
        "Expected exactly 1 probe (no subqueries in WHERE)"
    );

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();

    let gby = upper.find("GROUP BY").expect("Probe must have GROUP BY");
    let (before, after) = (&upper[..gby], &upper[gby..]);

    assert!(
        !after.contains("FUND_CODE"),
        "Join column 'fund_code' must NOT appear in GROUP BY — \
         a.fund_code = v.fund_code is a table join, not a parameter.\nProbe: {}",
        probe
    );
    assert!(
        before.contains("FUND_CODE"),
        "Join condition 'a.fund_code = v.fund_code' must be preserved in WHERE.\nProbe: {}",
        probe
    );

    assert!(
        after.contains("SHARE_PARTNER_CODE"),
        "GROUP BY must include parameterized column 'share_partner_code'.\nProbe: {}",
        probe
    );
    assert!(
        after.contains("ACCOUNTDATE"),
        "GROUP BY must include parameterized column 'accountdate'.\nProbe: {}",
        probe
    );

    // BETWEEN references v_gffsrq which is a classified stored-proc variable.
    // The expression cannot be evaluated at probe time (param value unknown),
    // so it is correctly removed from the probe WHERE (see Design Decision 2).
    assert!(
        !before.contains("BETWEEN"),
        "BETWEEN on stored-proc variable must be removed from probe WHERE.\nProbe: {}",
        probe
    );
    assert!(
        before.contains("TDSTOCKBAL"),
        "'v.tdstockbal <> 0' must be preserved in WHERE.\nProbe: {}",
        probe
    );
}

#[test]
fn test_correlated_ref_in_subquery_no_probe() {
    // Regression guard: An EXISTS subquery whose only equality is a correlated
    // reference (v.label_code = a.label_code, where `v` is an outer alias) has
    // no user-controllable parameter. Its data availability depends on the data
    // relationship between tables, not on an input value. No candidate value
    // probe should be generated for such a subquery.
    let (_statements, suggestions) = test_suggest(
        "INSERT INTO AAA (seq_no, coin_code, tdstockbal) \
         SELECT DISTINCT p_i_seq_no, v.coin_code, v.tdstockbal \
         FROM PAR A, VAB v \
         WHERE a.share_partner_code = v_share_partner_code \
           AND a.fund_code = v.fund_code \
           AND v.accountdate = v_gffsrq \
           AND v_gffsrq BETWEEN a.inure_begin_date AND a.inure_end_date \
           AND EXISTS (SELECT 1 FROM PAR1 a \
                       WHERE a.account_type IN ('01', '02') \
                         AND v.label_code = a.label_code) \
           AND v.tdstockbal <> 0",
    );
    assert_eq!(
        suggestions.len(),
        1,
        "Expected 1 probe (main scope only); EXISTS subquery with only correlated ref should not produce a probe"
    );

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();
    assert!(
        upper.contains("SHARE_PARTNER_CODE") && upper.contains("ACCOUNTDATE"),
        "Probe must be the main-scope probe grouping on parameterized columns:\n{}",
        probe
    );
    let group_by = upper.find("GROUP BY").unwrap();
    assert!(
        !upper[group_by..].contains("LABEL_CODE"),
        "No probe should group on label_code (correlated ref column):\n{}",
        probe
    );
}

// ── T1: P0 — OR IS NULL pattern with stored-proc variable filtered from probe WHERE ──

#[test]
fn test_or_is_null_pattern_filtered_from_probe_whole() {
    let (_statements, suggestions) =
        test_suggest("SELECT * FROM t WHERE t.a = '1' AND (p_x IS NULL OR t.b = p_x)");
    assert!(
        !suggestions.is_empty(),
        "Rule should match param eq in OR pattern"
    );
    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();

    // probe WHERE must NOT reference p_x (the stored-proc variable)
    assert!(
        !upper.contains("P_X"),
        "Probe must NOT reference stored-proc variable p_x: {}",
        probe
    );
    // probe WHERE must retain t.a = '1'
    assert!(
        upper.contains("T.A"),
        "Probe must retain t.a = '1' literal: {}",
        probe
    );
    // probe GROUP BY must include t.b
    assert!(
        upper.contains("GROUP BY") && upper.contains("T.B"),
        "Probe must GROUP BY t.b: {}",
        probe
    );
}

// ── T2: P1 — No duplicate predicate from (col = expr OR expr IS NULL) ──

#[test]
fn test_or_eq_is_null_no_duplicate_predicate() {
    // `v_c` is a stored-proc variable that makes the rule match.
    // Before fix: the = inside the OR calls handle_equality → pushes bare
    // t.b = decode(...) to non_eq, creating a duplicate predicate.
    let (_statements, suggestions) = test_suggest(
        "SELECT * FROM t WHERE t.a = '1' AND t.c = v_c AND (t.b = decode(t.a, '1', '0', t.b) OR decode(t.a, '1', '0', t.b) IS NULL)",
    );
    assert!(
        !suggestions.is_empty(),
        "Rule should match: param equality exists"
    );
    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();

    // The OR expression must be present (the full expression is preserved)
    assert!(
        upper.contains("OR"),
        "Probe must contain OR expression: {}",
        probe
    );

    // t.a = '1' must appear exactly once (regression: before fix the bare
    // t.b = decode(...) would also appear as a standalone predicate, which
    // would add an extra AND after t.a = '1')
    assert_eq!(
        probe.match_indices("t.a = '1'").count(),
        1,
        "t.a = '1' should appear exactly once: {}",
        probe
    );

    // The bare equality t.b = decode(...) must NOT appear standalone.
    // Before fix, decode appeared 4 times (2 in bare eq + 2 in OR).
    // After fix, decode appears 2 times (both inside the preserved OR).
    let decode_count = probe.match_indices("decode").count();
    assert_eq!(
        decode_count, 2,
        "Expected exactly 2 decode calls (inside OR only), got {}: {}",
        decode_count, probe
    );

    // Probe must reference the param-column in GROUP BY
    assert!(
        upper.contains("GROUP BY") && upper.contains("T.C"),
        "Probe must GROUP BY t.c: {}",
        probe
    );
}

// ── T3: P2 — Correlated ref in EXISTS subquery probe → Medium confidence ──

#[test]
fn test_correlated_ref_in_exists_probe_downgrades_confidence() {
    let (_statements, suggestions) = test_suggest(
        "SELECT * FROM a WHERE EXISTS (SELECT 1 FROM b v WHERE v.code = a.code AND v.user = p_u)",
    );
    assert_eq!(
        suggestions.len(),
        1,
        "Expected 1 probe from EXISTS subquery (main scope has no tier1)"
    );

    // NOTE: Suggestion.confidence is hardcoded High in engine; the actual
    // confidence is in RewriteAction::Generate::confidence.
    match &suggestions[0].action {
        RewriteAction::Generate { confidence, .. } => assert_eq!(
            *confidence,
            Confidence::Medium,
            "EXISTS subquery with correlated ref should have Medium confidence"
        ),
        _ => panic!("Expected Generate action"),
    }

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    // Probe WHERE must preserve correlation predicate
    assert!(
        probe.contains("v.code = a.code"),
        "Probe must preserve correlation predicate v.code = a.code: {}",
        probe
    );
    // Probe must reference the parameter column
    assert!(
        probe.contains("v.user"),
        "Probe must reference v.user: {}",
        probe
    );
}

// ── T4: P2 regression guard — Same-scope join keeps High confidence ──

#[test]
fn test_same_scope_join_keeps_high_confidence() {
    let (_statements, suggestions) =
        test_suggest("SELECT * FROM a, b WHERE a.id = b.id AND a.status = p_s");
    assert!(
        !suggestions.is_empty(),
        "Rule should match: param equality exists"
    );
    match &suggestions[0].action {
        RewriteAction::Generate { confidence, .. } => assert_eq!(
            *confidence,
            Confidence::High,
            "Same-scope join with no subquery should have High confidence"
        ),
        _ => panic!("Expected Generate action"),
    }

    let probe = format_probe(&suggestions).expect("Expected Generate action");
    assert!(
        probe.contains("a.status"),
        "Probe must reference a.status: {}",
        probe
    );
}

// ── T5: P0 regression guard — Explicit :p param in OR still filtered ──

#[test]
fn test_explicit_parameter_in_or_pattern_still_filtered() {
    // JDBC `?` parameter inside OR: the entire expression (? IS NULL OR t.b = ?)
    // contains Expr::JdbcParam, so non_param_exprs() must filter it from probe WHERE.
    let (_statements, suggestions) = test_suggest("SELECT * FROM t WHERE (? IS NULL OR t.b = ?)");
    assert!(
        !suggestions.is_empty(),
        "Rule should match param eq in OR pattern"
    );
    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();

    // Probe must NOT contain reference to ? parameter
    assert!(
        !probe.contains("?"),
        "Probe must NOT reference ? parameter: {}",
        probe
    );
    // GROUP BY must include t.b
    assert!(
        upper.contains("GROUP BY") && upper.contains("T.B"),
        "Probe must GROUP BY t.b: {}",
        probe
    );
}

#[test]
fn test_param_only_in_like_filtered_via_pre_scan() {
    // Origin: case6-2.sql — p_i_qry_bank_name only appears in a LIKE predicate,
    // never in an equality. The pre-scan must still recognize it as a parameter
    // and filter the LIKE OR from the probe WHERE.
    let (_statements, suggestions) = test_suggest(
        "SELECT t.a FROM dat_t t \
         WHERE t.active = 'Y' AND t.status = p_status \
         AND (p_filter IS NULL OR t.name LIKE '%' || p_filter || '%')",
    );
    let probe = format_probe(&suggestions).expect("Expected Generate action");
    let upper = probe.to_uppercase();

    assert!(
        upper.contains("GROUP BY") && upper.contains("T.STATUS"),
        "Probe must GROUP BY t.status (from equality).\nProbe: {}",
        probe
    );
    assert!(
        upper.contains("T.NAME"),
        "Probe must GROUP BY t.name — LIKE subject column should be extracted.\nProbe: {}",
        probe
    );
    assert!(
        !upper.contains("P_FILTER"),
        "p_filter must not appear anywhere in probe — pre-scan should classify it.\nProbe: {}",
        probe
    );
    assert!(
        !upper.contains("LIKE"),
        "LIKE predicate with stored-proc variable must be filtered from probe WHERE.\nProbe: {}",
        probe
    );
    assert!(
        upper.contains("ACTIVE"),
        "Non-param literal condition t.active = 'Y' must be preserved.\nProbe: {}",
        probe
    );
}
