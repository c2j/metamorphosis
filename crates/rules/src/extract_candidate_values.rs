use crate::eq_analyzer;
use metamorphosis_core::types::{
    Confidence, MatchResult, RewriteAction, RuleCategory, SafetyLevel,
};
use metamorphosis_core::{RewriteContext, RewriteRule};
use ogsql_parser::ast::{
    Expr, GroupByItem, Literal, ObjectName, OrderByItem, SelectStatement, SelectTarget, Spanned,
    Statement, TableRef,
};
use std::collections::HashSet;
use tracing::debug;

/// Rule: extract candidate values from parameterized WHERE equalities and generate
/// a GROUP BY probe SQL showing all existing values for those columns.
///
/// Manual level: only generates suggestions (probe SQL), never replaces.
///
/// # Purpose
///
/// When a SQL query uses `WHERE col = :param` and the input parameter value does not
/// exist in the data, the query returns nothing. This rule generates a probe to show
/// what values *do* exist (filtered by non-parameterized conditions), enabling the
/// user to find a valid input value.
///
/// # DML Support
///
/// Supports SELECT, UPDATE, DELETE, INSERT ... SELECT, and MERGE statements.
/// For each statement, the rule extracts multiple query scopes (main query,
/// subqueries in WHERE, CTEs) and generates one probe per scope with tier-1
/// (parameterized) equality columns.
///
/// # Example
///
/// Input:  `SELECT t.special_sql FROM t WHERE t.clear_type = '4' AND t.task_status = p_ts`
/// Probe:  `SELECT t.task_status, count(1) AS cnt FROM t WHERE t.clear_type = '4' GROUP BY t.task_status ORDER BY cnt DESC`
#[derive(Debug)]
pub struct ExtractCandidateValues;

struct ProbeCandidate {
    stmt: Box<Statement>,
    label: String,
    group_cols: Vec<ObjectName>,
    has_correlated_ref: bool,
    has_subquery: bool,
}

fn scope_sort_priority(label: &str) -> u8 {
    if label.starts_with("subquery") || label.starts_with("cte:") {
        0
    } else {
        1
    }
}

impl RewriteRule for ExtractCandidateValues {
    fn id(&self) -> &'static str {
        "extract-candidate-values"
    }

    fn description(&self) -> &'static str {
        "Generate probe SQL showing existing values of parameterized WHERE equality columns"
    }

    fn category(&self) -> RuleCategory {
        RuleCategory::DataQuality
    }

    fn safety_level(&self) -> SafetyLevel {
        SafetyLevel::Manual
    }

    fn matches(&self, ctx: &RewriteContext, stmt: &Statement) -> MatchResult {
        let scopes = eq_analyzer::extract_statement_scopes(stmt, ctx.known_variables);
        for scope in &scopes {
            let collector = eq_analyzer::collect_eq_predicates(
                &scope.where_clause,
                &scope.from,
                ctx.known_variables,
            );
            if !collector.tier1.is_empty() {
                return MatchResult::Matched;
            }
        }
        MatchResult::NotMatched {
            reason: "No parameterized equality conditions found in any query scope".to_string(),
        }
    }

    fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Vec<RewriteAction> {
        let scopes = eq_analyzer::extract_statement_scopes(stmt, ctx.known_variables);

        let mut candidates: Vec<ProbeCandidate> = Vec::new();

        for scope in &scopes {
            let collector = eq_analyzer::collect_eq_predicates(
                &scope.where_clause,
                &scope.from,
                ctx.known_variables,
            );
            if collector.tier1.is_empty() {
                continue;
            }

            let mut seen = HashSet::new();
            let mut group_cols: Vec<ObjectName> = Vec::new();
            for col_name in collector.tier1.iter() {
                let key = col_name
                    .last()
                    .map(|i| i.as_str().to_string())
                    .unwrap_or_default();
                if seen.insert(key) {
                    group_cols.push(col_name.clone());
                }
            }

            if group_cols.is_empty() {
                continue;
            }

            let limit = ctx.config.probe_default_limit;
            let non_param = collector.non_param_exprs();
            let probe = build_candidate_probe_statement(
                &scope.from,
                &collector.keep_exprs,
                &non_param,
                &group_cols,
                limit,
            );

            debug!(
                rule_id = self.id(),
                scope = %scope.label,
                group_cols = ?group_cols,
                "Generated candidate value probe"
            );

            candidates.push(ProbeCandidate {
                stmt: Box::new(Statement::Select(probe)),
                label: scope.label.clone(),
                group_cols,
                has_correlated_ref: collector.has_correlated_ref,
                has_subquery: collector.has_subquery,
            });
        }

        candidates.sort_by_key(|c| scope_sort_priority(&c.label));

        let total = candidates.len();
        candidates
            .into_iter()
            .enumerate()
            .map(|(idx, c)| {
                let probe_num = idx + 1;
                let cols: Vec<String> = c.group_cols.iter().map(|g| g.join(".")).collect();
                let mut purpose = format!(
                    "Probe {} of {}: candidate values for [{}] [scope: {}]",
                    probe_num,
                    total,
                    cols.join(", "),
                    c.label,
                );

                if c.has_correlated_ref {
                    purpose.push_str(
                        "\nContains correlated reference — remove WHERE or substitute a value to run standalone",
                    );
                }
                if c.has_subquery && total > 1 {
                    purpose.push_str(
                        "\nContains subquery with unsubstituted parameters — run earlier probes first and substitute",
                    );
                }

                RewriteAction::Generate {
                    stmt: c.stmt,
                    purpose,
                    confidence: if c.has_subquery || c.has_correlated_ref {
                        Confidence::Medium
                    } else {
                        Confidence::High
                    },
                }
            })
            .collect()
    }
}

/// Build probe SQL preserving FROM and non-parameterized conditions:
/// `SELECT col1, col2, ..., count(1) AS cnt FROM tables WHERE keep_conds AND non_eq GROUP BY col1, col2, ... ORDER BY cnt DESC LIMIT N`
fn build_candidate_probe_statement(
    from: &[TableRef],
    keep_exprs: &[Expr],
    non_param_exprs: &[Expr],
    group_cols: &[ObjectName],
    limit: usize,
) -> Spanned<SelectStatement> {
    let mut targets: Vec<SelectTarget> = group_cols
        .iter()
        .map(|name| SelectTarget::Expr(Expr::ColumnRef(name.clone()), None))
        .collect();

    targets.push(SelectTarget::Expr(
        Expr::FunctionCall {
            name: vec!["count".into()],
            args: vec![Expr::Literal(Literal::Integer(1))],
            distinct: false,
            over: None,
            filter: None,
            within_group: vec![],
            separator: None,
            default: None,
            conversion_format: None,
            agg_from: None,
            builtin: None,
        },
        Some("cnt".into()),
    ));

    let group_by: Vec<GroupByItem> = group_cols
        .iter()
        .map(|name| GroupByItem::Expr(Expr::ColumnRef(name.clone())))
        .collect();

    let order_by = vec![OrderByItem {
        expr: Expr::ColumnRef(vec!["cnt".into()]),
        asc: Some(false),
        nulls_first: None,
        using: None,
    }];

    let limit_expr = Some(Expr::Literal(Literal::Integer(limit as i64)));

    let where_clause = eq_analyzer::merge_exprs(keep_exprs, non_param_exprs);

    Spanned::without_span(SelectStatement {
        hints: vec![],
        with: None,
        distinct: false,
        distinct_on: vec![],
        targets,
        into_targets: None,
        bulk_collect: false,
        into_table: None,
        from: from.to_vec(),
        where_clause,
        connect_by: None,
        group_by,
        having: None,
        order_by,
        order_siblings: false,
        limit: limit_expr,
        offset: None,
        fetch: None,
        lock_clause: None,
        window_clause: vec![],
        set_operation: None,
        raw_body: None,
    })
}
