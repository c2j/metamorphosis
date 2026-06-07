use crate::eq_analyzer;
use metamorphosis_core::types::{Confidence, RewriteAction, RuleCategory, SafetyLevel};
use metamorphosis_core::{RewriteContext, RewriteRule};
use ogsql_parser::ast::{
    Expr, GroupByItem, Literal, OrderByItem, SelectStatement, SelectTarget, Spanned, Statement,
    TableRef,
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
/// # Example
///
/// Input:  `SELECT t.special_sql FROM t WHERE t.clear_type = '4' AND t.task_status = p_ts`
/// Probe:  `SELECT t.task_status, count(1) AS cnt FROM t WHERE t.clear_type = '4' GROUP BY t.task_status ORDER BY cnt DESC`
#[derive(Debug)]
pub struct ExtractCandidateValues;

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

    fn matches(&self, ctx: &RewriteContext, stmt: &Statement) -> bool {
        let select = match stmt {
            Statement::Select(s) => &s.node,
            _ => return false,
        };

        let (where_clause, from) = eq_analyzer::resolve_query(select);
        let collector = eq_analyzer::collect_eq_predicates(where_clause, from, ctx.known_variables);
        !collector.tier1.is_empty()
    }

    fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Option<RewriteAction> {
        let select = match stmt {
            Statement::Select(s) => &s.node,
            _ => return None,
        };

        let (where_clause, from) = eq_analyzer::resolve_query(select);
        let collector = eq_analyzer::collect_eq_predicates(where_clause, from, ctx.known_variables);

        let mut seen = HashSet::new();
        let mut group_cols: Vec<Vec<String>> = Vec::new();
        for col_name in collector.tier1.iter() {
            let key = col_name.last().cloned().unwrap_or_default();
            if seen.insert(key) {
                group_cols.push(col_name.clone());
            }
        }

        if group_cols.is_empty() {
            return None;
        }

        let limit = ctx.config.probe_default_limit;
        let non_param = collector.non_param_exprs();
        let probe = build_candidate_probe_statement(
            from,
            &collector.keep_exprs,
            &non_param,
            &group_cols,
            limit,
        );

        debug!(
            rule_id = self.id(),
            group_cols = ?group_cols,
            "Generated candidate value probe"
        );

        let purpose = if group_cols.len() == 1 {
            let display = group_cols[0].join(".");
            format!(
                "Candidate value extraction: show existing values for column '{}'",
                display
            )
        } else {
            let displays: Vec<String> = group_cols.iter().map(|c| c.join(".")).collect();
            format!(
                "Candidate value extraction: show existing value combinations for columns [{}]",
                displays.join(", ")
            )
        };

        Some(RewriteAction::Generate {
            stmt: Box::new(Statement::Select(probe)),
            purpose,
            confidence: if collector.has_subquery {
                Confidence::Medium
            } else {
                Confidence::High
            },
        })
    }
}

/// Build probe SQL preserving FROM and non-parameterized conditions:
/// `SELECT col1, col2, ..., count(1) AS cnt FROM tables WHERE keep_conds AND non_eq GROUP BY col1, col2, ... ORDER BY cnt DESC LIMIT N`
fn build_candidate_probe_statement(
    from: &[TableRef],
    keep_exprs: &[Expr],
    non_param_exprs: &[Expr],
    group_cols: &[Vec<String>],
    limit: usize,
) -> Spanned<SelectStatement> {
    let mut targets: Vec<SelectTarget> = group_cols
        .iter()
        .map(|name| SelectTarget::Expr(Expr::ColumnRef(name.clone()), None))
        .collect();

    targets.push(SelectTarget::Expr(
        Expr::FunctionCall {
            name: vec!["count".to_string()],
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
        Some("cnt".to_string()),
    ));

    let group_by: Vec<GroupByItem> = group_cols
        .iter()
        .map(|name| GroupByItem::Expr(Expr::ColumnRef(name.clone())))
        .collect();

    let order_by = vec![OrderByItem {
        expr: Expr::ColumnRef(vec!["cnt".to_string()]),
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
