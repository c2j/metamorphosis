//! Rule: generate probe SQL showing MIN/MAX/COUNT(DISTINCT)/COUNT(*) for
//! parameterized WHERE equality columns.
//!
//! Manual level: only generates suggestions (probe SQL), never replaces.
//!
//! # Purpose
//!
//! When a query uses `WHERE col = :param`, the user may pass a value that
//! does not exist in the data, resulting in an empty result set. This probe
//! shows the value range and cardinality of the parameter column, helping
//! the user choose valid input values.

use crate::eq_analyzer;
use metamorphosis_core::types::{
    Confidence, MatchResult, RewriteAction, RuleCategory, SafetyLevel,
};
use metamorphosis_core::{RewriteContext, RewriteRule};
use ogsql_parser::ast::{
    Expr, Literal, ObjectName, SelectStatement, SelectTarget, Spanned, Statement, TableRef,
};
use std::collections::HashSet;
use tracing::debug;

/// Rule: generate probe SQL showing value range for parameterized equality columns.
///
/// Manual level: only generates suggestions, never replaces the original SQL.
#[derive(Debug)]
pub struct ProbeParamRange;

impl RewriteRule for ProbeParamRange {
    fn id(&self) -> &'static str {
        "probe-param-range"
    }

    fn description(&self) -> &'static str {
        "Generate probe SQL showing MIN/MAX/COUNT(DISTINCT) for parameterized equality columns"
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
        let mut actions = Vec::new();

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
            let mut unique_cols: Vec<ObjectName> = Vec::new();
            for col_name in &collector.tier1 {
                let key = col_name
                    .last()
                    .map(|i| i.as_str().to_string())
                    .unwrap_or_default();
                if seen.insert(key) {
                    unique_cols.push(col_name.clone());
                }
            }

            if unique_cols.is_empty() {
                continue;
            }

            let non_param = collector.non_param_exprs();
            let probe =
                build_range_probe(&scope.from, &collector.keep_exprs, &non_param, &unique_cols);

            debug!(
                rule_id = self.id(),
                scope = %scope.label,
                columns = ?unique_cols,
                "Generated param range probe"
            );

            let col_names: Vec<String> = unique_cols
                .iter()
                .map(|c| c.iter().map(|i| i.as_str()).collect::<Vec<_>>().join("."))
                .collect();

            actions.push(RewriteAction::Generate {
                stmt: Box::new(Statement::Select(probe)),
                purpose: format!(
                    "Parameter range probe for [{}] [scope: {}] — shows MIN/MAX/cardinality to validate parameter values",
                    col_names.join(", "),
                    scope.label,
                ),
                confidence: Confidence::High,
            });
        }

        actions
    }
}

fn build_range_probe(
    from: &[TableRef],
    keep_exprs: &[Expr],
    non_param_exprs: &[Expr],
    columns: &[ObjectName],
) -> Spanned<SelectStatement> {
    let mut targets: Vec<SelectTarget> = Vec::new();

    for col in columns {
        let col_ref = Expr::ColumnRef(col.clone());
        let col_label: String = col.iter().map(|i| i.as_str()).collect::<Vec<_>>().join("_");

        targets.push(func_target(
            "min",
            &col_ref,
            false,
            &format!("{col_label}_min"),
        ));
        targets.push(func_target(
            "max",
            &col_ref,
            false,
            &format!("{col_label}_max"),
        ));
        targets.push(func_target(
            "count",
            &col_ref,
            true,
            &format!("{col_label}_distinct"),
        ));
    }

    targets.push(count_one_alias("total"));

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
        group_by: vec![],
        having: None,
        order_by: vec![],
        order_siblings: false,
        limit: None,
        offset: None,
        fetch: None,
        lock_clause: None,
        window_clause: vec![],
        set_operation: None,
        raw_body: None,
    })
}

fn func_target(name: &str, arg: &Expr, distinct: bool, alias: &str) -> SelectTarget {
    SelectTarget::Expr(
        Expr::FunctionCall {
            name: vec![name.into()],
            args: vec![arg.clone()],
            distinct,
            over: None,
            filter: None,
            within_group: vec![],
            separator: None,
            default: None,
            conversion_format: None,
            agg_from: None,
            builtin: None,
        },
        Some(alias.into()),
    )
}

fn count_one_alias(alias: &str) -> SelectTarget {
    SelectTarget::Expr(
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
        Some(alias.into()),
    )
}
