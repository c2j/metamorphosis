//! Rule: detect duplicate candidate keys from equality conditions and generate
//! a GROUP BY probe SQL to verify uniqueness.
//!
//! Manual level: only generates suggestions (probe SQL), never replaces.
//!
//! # DML Support
//!
//! Supports SELECT, UPDATE, DELETE, INSERT ... SELECT, and MERGE statements.
//! For each statement, the rule extracts multiple query scopes (main query,
//! subqueries in WHERE, CTEs) and generates one probe per scope with ≥2
//! tier-1 (parameterized) equality columns.

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

/// Rule: detect duplicate candidate keys from equality conditions and generate
/// a GROUP BY probe SQL to verify uniqueness.
///
/// Manual level: only generates suggestions (probe SQL), never replaces.
#[derive(Debug)]
pub struct DetectDuplicateEqKeys;

impl RewriteRule for DetectDuplicateEqKeys {
    fn id(&self) -> &'static str {
        "detect-duplicate-eq-keys"
    }

    fn description(&self) -> &'static str {
        "Detect candidate keys from equality conditions and generate uniqueness probe"
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
            if collector.tier1.len() >= 2 {
                return MatchResult::Matched;
            }
        }
        MatchResult::NotMatched {
            reason: "No scope with ≥2 equality conditions found".to_string(),
        }
    }

    fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Vec<RewriteAction> {
        let scopes = eq_analyzer::extract_statement_scopes(stmt, ctx.known_variables);
        let mut actions = Vec::new();

        for scope in &scopes {
            let collector = eq_analyzer::collect_eq_predicates(
                &scope.where_clause,
                &scope.from,
                ctx.known_variables,
            );
            if collector.tier1.len() < 2 {
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
            let probe = build_probe_statement(
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
                "Generated duplicate key probe"
            );

            actions.push(RewriteAction::Generate {
                stmt: Box::new(Statement::Select(probe)),
                purpose: format!(
                    "Candidate key duplicate detection: verify uniqueness of equality-condition columns [scope: {}]",
                    scope.label
                ),
                confidence: if collector.has_subquery {
                    Confidence::Medium
                } else {
                    Confidence::High
                },
            });
        }

        actions
    }
}

/// Build probe SQL preserving FROM and JOIN conditions (tier1 equalities excluded):
/// `SELECT col1, col2, ..., count(1) AS cnt FROM tables WHERE join_conds AND non_eq GROUP BY col1, col2, ... HAVING count(1) > 1 ORDER BY cnt DESC LIMIT N`
fn build_probe_statement(
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

    let having = Some(Expr::BinaryOp {
        left: Box::new(Expr::FunctionCall {
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
        }),
        op: ">".to_string(),
        right: Box::new(Expr::Literal(Literal::Integer(1))),
    });

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
        having,
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
