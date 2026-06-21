//! Rule: generate probe SQL checking referential integrity for JOIN queries.
//!
//! Manual level: only generates suggestions (probe SQL), never replaces.
//!
//! # Purpose
//!
//! In multi-table JOIN queries, low match rates between joined tables may
//! indicate orphan records or data integrity issues. This probe performs
//! a LEFT JOIN and counts matched vs total rows, revealing the percentage
//! of left-table rows that have corresponding right-table entries.

use metamorphosis_core::types::{
    Confidence, MatchResult, RewriteAction, RuleCategory, SafetyLevel,
};
use metamorphosis_core::{RewriteContext, RewriteRule};
use ogsql_parser::ast::{
    Expr, JoinType, Literal, ObjectName, SelectStatement, SelectTarget, Spanned, Statement,
    TableRef,
};
use tracing::debug;

/// Rule: generate probe SQL checking referential integrity for JOIN queries.
///
/// Manual level: only generates suggestions, never replaces the original SQL.
#[derive(Debug)]
pub struct ProbeJoinIntegrity;

struct JoinInfo {
    left: TableRef,
    right: TableRef,
    condition: Expr,
}

impl RewriteRule for ProbeJoinIntegrity {
    fn id(&self) -> &'static str {
        "probe-join-integrity"
    }

    fn description(&self) -> &'static str {
        "Generate probe SQL checking referential integrity for JOIN queries"
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

        let joins = collect_joins(&select.from);
        if joins.is_empty() {
            return MatchResult::NotMatched {
                reason: "No JOIN found in FROM clause".to_string(),
            };
        }

        MatchResult::Matched
    }

    fn apply(&self, _ctx: &RewriteContext, stmt: &Statement) -> Vec<RewriteAction> {
        let select = match stmt {
            Statement::Select(s) => s,
            _ => return vec![],
        };

        let joins = collect_joins(&select.from);
        if joins.is_empty() {
            return vec![];
        }

        let mut actions = Vec::new();
        for (idx, join) in joins.iter().enumerate() {
            let right_alias = table_ref_label(&join.right);
            let match_col = find_right_column(&join.condition, &join.right);

            let probe = match (right_alias.as_deref(), match_col.as_ref()) {
                (_, Some(col)) => build_integrity_probe(&join.left, &join.right, &join.condition, col),
                _ => continue,
            };

            debug!(
                rule_id = self.id(),
                join_index = idx,
                right_table = ?right_alias,
                "Generated join integrity probe"
            );

            let left_name = table_ref_label(&join.left)
                .unwrap_or_else(|| "left".to_string());
            let right_name = table_ref_label(&join.right)
                .unwrap_or_else(|| "right".to_string());

            actions.push(RewriteAction::Generate {
                stmt: Box::new(Statement::Select(probe)),
                purpose: format!(
                    "Join integrity probe {} — referential check between [{ }] and [{ }]: shows matched vs total row count",
                    idx + 1,
                    left_name,
                    right_name,
                ),
                confidence: Confidence::Medium,
            });
        }

        actions
    }
}

fn collect_joins(from: &[TableRef]) -> Vec<JoinInfo> {
    let mut result = Vec::new();
    for tr in from {
        collect_joins_recursive(tr, &mut result);
    }
    result
}

fn collect_joins_recursive(tr: &TableRef, result: &mut Vec<JoinInfo>) {
    if let TableRef::Join { left, right, condition, .. } = tr {
        if let Some(cond) = condition {
            result.push(JoinInfo {
                left: left.as_ref().clone(),
                right: right.as_ref().clone(),
                condition: cond.clone(),
            });
        }
        collect_joins_recursive(left, result);
        collect_joins_recursive(right, result);
    }
}

fn table_ref_label(tr: &TableRef) -> Option<String> {
    match tr {
        TableRef::Table { name, alias, .. } => {
            alias
                .as_ref()
                .map(|a| a.as_str().to_string())
                .or_else(|| name.last().map(|i| i.as_str().to_string()))
        }
        TableRef::Subquery { alias, .. }
        | TableRef::Values { alias, .. }
        | TableRef::FunctionCall { alias, .. } => {
            alias.as_ref().map(|a| a.as_str().to_string())
        }
        _ => None,
    }
}

fn find_right_column(condition: &Expr, right_table: &TableRef) -> Option<ObjectName> {
    let right_label = table_ref_label(right_table)?;
    let mut found: Option<ObjectName> = None;
    find_column_for_label(condition, &right_label, &mut found);
    found
}

fn find_column_for_label(expr: &Expr, label: &str, found: &mut Option<ObjectName>) {
    if found.is_some() {
        return;
    }
    match expr {
        Expr::ColumnRef(name) => {
            if name
                .first()
                .map(|i| i.as_str().eq_ignore_ascii_case(label))
                .unwrap_or(false)
            {
                *found = Some(name.clone());
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            find_column_for_label(left, label, found);
            find_column_for_label(right, label, found);
        }
        Expr::UnaryOp { expr: inner, .. } => find_column_for_label(inner, label, found),
        Expr::Parenthesized(inner) => find_column_for_label(inner, label, found),
        _ => {}
    }
}

fn build_integrity_probe(
    left: &TableRef,
    right: &TableRef,
    condition: &Expr,
    match_col: &ObjectName,
) -> Spanned<SelectStatement> {
    let total_target = SelectTarget::Expr(
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
        Some("total".into()),
    );

    let matched_target = SelectTarget::Expr(
        Expr::FunctionCall {
            name: vec!["count".into()],
            args: vec![Expr::ColumnRef(match_col.clone())],
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
        Some("matched".into()),
    );

    let joined_from = TableRef::Join {
        left: Box::new(left.clone()),
        right: Box::new(right.clone()),
        join_type: JoinType::Left,
        condition: Some(condition.clone()),
        natural: false,
        using_columns: vec![],
    };

    Spanned::without_span(SelectStatement {
        hints: vec![],
        with: None,
        distinct: false,
        distinct_on: vec![],
        targets: vec![total_target, matched_target],
        into_targets: None,
        bulk_collect: false,
        into_table: None,
        from: vec![joined_from],
        where_clause: None,
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
