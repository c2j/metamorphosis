//! Rule: generate probe SQL showing value distribution for GROUP BY columns.
//!
//! Manual level: only generates suggestions (probe SQL), never replaces.
//!
//! # Purpose
//!
//! High concentration of rows in a few group values indicates data skew,
//! which can cause uneven parallel execution in distributed databases.
//! This probe shows the top-N most frequent values and their counts,
//! enabling the user to assess skew risk.

use metamorphosis_core::types::{
    Confidence, MatchResult, RewriteAction, RuleCategory, SafetyLevel,
};
use metamorphosis_core::{RewriteContext, RewriteRule};
use ogsql_parser::ast::{
    Expr, GroupByItem, Literal, ObjectName, OrderByItem, SelectStatement, SelectTarget, Spanned,
    Statement, TableRef,
};
use tracing::debug;

/// Rule: generate probe SQL showing value distribution for GROUP BY columns.
///
/// Manual level: only generates suggestions, never replaces the original SQL.
#[derive(Debug)]
pub struct ProbeDataSkew;

impl RewriteRule for ProbeDataSkew {
    fn id(&self) -> &'static str {
        "probe-data-skew"
    }

    fn description(&self) -> &'static str {
        "Generate probe SQL showing value distribution for GROUP BY columns"
    }

    fn category(&self) -> RuleCategory {
        RuleCategory::DataQuality
    }

    fn safety_level(&self) -> SafetyLevel {
        SafetyLevel::Manual
    }

    fn matches(&self, _ctx: &RewriteContext, stmt: &Statement) -> MatchResult {
        let select = match stmt {
            Statement::Select(s) => s,
            _ => {
                return MatchResult::NotMatched {
                    reason: "Not a SELECT statement".to_string(),
                };
            }
        };

        if select.group_by.is_empty() {
            return MatchResult::NotMatched {
                reason: "No GROUP BY clause found".to_string(),
            };
        }

        MatchResult::Matched
    }

    fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Vec<RewriteAction> {
        let select = match stmt {
            Statement::Select(s) => s,
            _ => return vec![],
        };

        let group_cols = extract_group_by_columns(&select.group_by);
        if group_cols.is_empty() {
            return vec![];
        }

        let limit = ctx.config.probe_default_limit;
        let probe = build_skew_probe(&select.from, &select.where_clause, &group_cols, limit);

        debug!(
            rule_id = self.id(),
            columns = ?group_cols,
            "Generated data skew probe"
        );

        let col_names: Vec<String> = group_cols
            .iter()
            .map(|c| c.iter().map(|i| i.as_str()).collect::<Vec<_>>().join("."))
            .collect();

        vec![RewriteAction::Generate {
            stmt: Box::new(Statement::Select(probe)),
            purpose: format!(
                "Data skew probe for GROUP BY columns [{}] — shows value distribution to assess parallel execution skew risk",
                col_names.join(", "),
            ),
            confidence: Confidence::High,
        }]
    }
}

fn extract_group_by_columns(group_by: &[GroupByItem]) -> Vec<ObjectName> {
    group_by
        .iter()
        .filter_map(|item| match item {
            GroupByItem::Expr(Expr::ColumnRef(name)) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn build_skew_probe(
    from: &[TableRef],
    where_clause: &Option<Expr>,
    group_cols: &[ObjectName],
    limit: usize,
) -> Spanned<SelectStatement> {
    let mut targets: Vec<SelectTarget> = group_cols
        .iter()
        .map(|name| SelectTarget::Expr(Expr::ColumnRef(name.clone()), None))
        .collect();

    targets.push(count_one_alias("cnt"));

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
        where_clause: where_clause.clone(),
        connect_by: None,
        group_by,
        having: None,
        order_by,
        order_siblings: false,
        limit: Some(Expr::Literal(Literal::Integer(limit as i64))),
        offset: None,
        fetch: None,
        lock_clause: None,
        window_clause: vec![],
        set_operation: None,
        raw_body: None,
    })
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
