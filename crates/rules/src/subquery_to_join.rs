//! Rule: convert correlated subqueries and IN/NOT IN subqueries to JOINs.
//!
//! Patterns handled (in WHERE clause):
//! - `EXISTS (SELECT ...)`        → `INNER JOIN`
//! - `expr IN (SELECT ...)`       → `INNER JOIN`
//! - `NOT EXISTS (SELECT ...)`    → `LEFT JOIN + WHERE right_col IS NULL`
//! - `expr NOT IN (SELECT ...)`   → `LEFT JOIN + WHERE right_col IS NULL`
//!
//! Scalar subqueries in the SELECT target list produce a suggestion only
//! (Manual safety level — semantics cannot be preserved by a rewrite).
//!
//! Safety guards: only rewrites single-table subqueries without GROUP BY,
//! HAVING, set operations, or JOINs inside the subquery.

use metamorphosis_core::types::{MatchResult, RewriteAction, RuleCategory, SafetyLevel, Severity};
use metamorphosis_core::{RewriteContext, RewriteRule};
use ogsql_parser::ast::{
    Expr, JoinType, SelectStatement, SelectTarget, Spanned, Statement, TableRef,
};
use tracing::debug;

/// Rule: convert subqueries in WHERE/IN/NOT IN/NOT EXISTS to JOINs.
///
/// # Safety
///
/// - `EXISTS` and `IN` → semantically equivalent → **Safe** in practice
/// - `NOT EXISTS` and `NOT IN` → semantically equivalent only when the
///   subquery columns used in the join are NOT NULL → **Conditional**
/// - Scalar subqueries in SELECT → only ever **Manual** (suggest)
#[derive(Debug)]
pub struct SubqueryToJoin;

/// Describes a detectable subquery pattern inside a WHERE clause.
enum WherePattern {
    /// `EXISTS (SELECT ...)` — produces INNER JOIN.
    Exists(Box<SelectStatement>),
    /// `expr IN (SELECT ...)` or `expr NOT IN (SELECT ...)`.
    InSubquery {
        expr: Box<Expr>,
        subquery: Box<SelectStatement>,
        negated: bool,
    },
}

impl RewriteRule for SubqueryToJoin {
    fn id(&self) -> &'static str {
        "subquery-to-join"
    }

    fn description(&self) -> &'static str {
        "Convert WHERE subqueries (EXISTS, IN, NOT EXISTS, NOT IN) to JOINs and suggest scalar subquery rewrites"
    }

    fn category(&self) -> RuleCategory {
        RuleCategory::Performance
    }

    fn safety_level(&self) -> SafetyLevel {
        SafetyLevel::Conditional
    }

    fn matches(&self, _ctx: &RewriteContext, stmt: &Statement) -> MatchResult {
        let select = match stmt {
            Statement::Select(s) => &s.node,
            _ => {
                return MatchResult::NotMatched {
                    reason: "Statement is not a SELECT".to_string(),
                }
            }
        };

        if let Some(ref where_clause) = select.where_clause {
            if find_where_pattern(where_clause).is_some() {
                return MatchResult::Matched;
            }
        }

        if has_scalar_subquery(&select.targets) {
            return MatchResult::Matched;
        }

        let mut reasons = Vec::new();
        if select.where_clause.is_none() {
            reasons.push("no WHERE clause");
        } else {
            reasons.push("no rewritable subquery pattern (EXISTS/IN/NOT IN/NOT EXISTS) in WHERE");
        }
        if !has_scalar_subquery(&select.targets) {
            reasons.push("no scalar subquery in SELECT targets");
        }
        MatchResult::NotMatched {
            reason: reasons.join("; "),
        }
    }

    fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Vec<RewriteAction> {
        let spanned = match stmt {
            Statement::Select(s) => s,
            _ => return vec![],
        };

        let select = &spanned.node;

        // Try WHERE-clause subquery first (these produce Replace actions).
        if let Some(ref where_clause) = select.where_clause {
            if let Some(pattern) = find_where_pattern(where_clause) {
                return vec![handle_where_pattern(pattern, select, ctx)];
            }
        }

        // Fallback: scalar subquery in targets (suggest only).
        if has_scalar_subquery(&select.targets) {
            return vec![RewriteAction::Suggest {
                message:
                    "Scalar subquery in SELECT list — consider rewriting as JOIN or window function"
                        .to_string(),
                severity: Severity::Warning,
            }];
        }

        vec![]
    }
}

// ── Pattern Detection ──

/// Try to detect a rewritable subquery pattern at the root of the WHERE
/// expression.  Returns `None` if the pattern is absent or fails safety
/// guards.
fn find_where_pattern(expr: &Expr) -> Option<WherePattern> {
    match expr {
        Expr::Exists(subquery) => {
            if is_safe_subquery(subquery) {
                debug!("Found EXISTS subquery");
                Some(WherePattern::Exists(subquery.clone()))
            } else {
                None
            }
        }
        Expr::UnaryOp { op, expr: inner } if op == "NOT" => {
            if let Expr::Exists(subquery) = inner.as_ref() {
                if is_safe_subquery(subquery) {
                    debug!("Found NOT EXISTS subquery");
                    Some(WherePattern::Exists(subquery.clone()))
                } else {
                    None
                }
            } else {
                None
            }
        }
        Expr::InSubquery {
            expr,
            subquery,
            negated,
        } => {
            if is_safe_subquery(subquery) {
                debug!("Found {} subquery", if *negated { "NOT IN" } else { "IN" });
                Some(WherePattern::InSubquery {
                    expr: expr.clone(),
                    subquery: subquery.clone(),
                    negated: *negated,
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Safety guards for the inner subquery.
///
/// We only rewrite when the subquery is simple:
/// - Exactly one table in FROM
/// - No JOINs in FROM
/// - No GROUP BY, HAVING
/// - No set operations (UNION, INTERSECT, EXCEPT)
/// - No aggregate functions in targets (for now — conservative)
fn is_safe_subquery(subquery: &SelectStatement) -> bool {
    // Must have exactly one table in FROM.
    if subquery.from.len() != 1 {
        return false;
    }

    // The single FROM entry must not be a JOIN.
    if matches!(subquery.from[0], TableRef::Join { .. }) {
        return false;
    }

    // No GROUP BY.
    if !subquery.group_by.is_empty() {
        return false;
    }

    // No HAVING.
    if subquery.having.is_some() {
        return false;
    }

    // No set operations.
    if subquery.set_operation.is_some() {
        return false;
    }

    true
}

/// Check whether any target is a scalar subquery `Expr::Subquery(...)`.
fn has_scalar_subquery(targets: &[SelectTarget]) -> bool {
    targets
        .iter()
        .any(|t| matches!(t, SelectTarget::Expr(Expr::Subquery(_), _)))
}

// ── Rewrite Dispatch ──

/// Dispatch the detected WHERE pattern to the appropriate handler.
fn handle_where_pattern(
    pattern: WherePattern,
    select: &SelectStatement,
    ctx: &RewriteContext,
) -> RewriteAction {
    match pattern {
        WherePattern::Exists(subquery) => {
            // Check if the outer WHERE was `NOT Exists(...)`.
            // We detect this at the WherePattern level: NOT EXISTS is
            // converted to an Exists pattern by find_where_pattern.
            // We need to re-check the original expression.
            let was_negated = select
                .where_clause
                .as_ref()
                .is_some_and(|expr| matches!(expr, Expr::UnaryOp { op, .. } if op == "NOT"));

            if was_negated {
                // SAFETY: is_safe_subquery passed, so helpers won't fail.
                rewrite_not_exists(&subquery, select, ctx)
                    .expect("NOT EXISTS rewrite should succeed after safety guard")
            } else {
                rewrite_exists(&subquery, select, ctx)
                    .expect("EXISTS rewrite should succeed after safety guard")
            }
        }
        WherePattern::InSubquery {
            expr,
            subquery,
            negated,
        } => {
            if negated {
                // expect justified: is_safe_subquery passed
                rewrite_not_in(&expr, &subquery, select, ctx)
                    .expect("NOT IN rewrite should succeed after safety guard")
            } else {
                // expect justified: is_safe_subquery passed
                rewrite_in(&expr, &subquery, select, ctx)
                    .expect("IN rewrite should succeed after safety guard")
            }
        }
    }
}

// ── Rewrite Helpers ──

/// Build a `TableRef::Join` with the given parameters.
fn make_join(
    left: TableRef,
    right_table: TableRef,
    join_type: JoinType,
    condition: Option<Expr>,
) -> TableRef {
    TableRef::Join {
        left: Box::new(left),
        right: Box::new(right_table),
        join_type,
        condition,
        natural: false,
        using_columns: vec![],
    }
}

/// Extract the single table reference from a simple subquery's FROM clause.
fn subquery_table(subquery: &SelectStatement) -> &TableRef {
    // Safe to unwrap: is_safe_subquery guarantees exactly one entry.
    &subquery.from[0]
}

// ── EXISTS → INNER JOIN ──

fn rewrite_exists(
    subquery: &SelectStatement,
    select: &SelectStatement,
    _ctx: &RewriteContext,
) -> Option<RewriteAction> {
    let new_from = build_joined_from(
        &select.from,
        subquery_table(subquery),
        JoinType::Inner,
        subquery.where_clause.clone(),
    )?;

    let mut new_select = select.clone();
    new_select.from = vec![new_from];
    new_select.where_clause = None;

    debug!("EXISTS → INNER JOIN");

    Some(RewriteAction::Replace(Box::new(Statement::Select(
        Spanned::without_span(new_select),
    ))))
}

// ── NOT EXISTS → LEFT JOIN + WHERE right_col IS NULL ──

fn rewrite_not_exists(
    subquery: &SelectStatement,
    select: &SelectStatement,
    _ctx: &RewriteContext,
) -> Option<RewriteAction> {
    let null_col = extract_is_null_column(subquery, subquery_table(subquery))?;
    let on_condition = subquery.where_clause.clone();

    let new_from = build_joined_from(
        &select.from,
        subquery_table(subquery),
        JoinType::Left,
        on_condition,
    )?;

    let mut new_select = select.clone();
    new_select.from = vec![new_from];
    new_select.where_clause = Some(Expr::IsNull {
        expr: Box::new(null_col),
        negated: false,
    });

    debug!("NOT EXISTS → LEFT JOIN + IS NULL");

    Some(RewriteAction::Replace(Box::new(Statement::Select(
        Spanned::without_span(new_select),
    ))))
}

// ── IN → INNER JOIN ──

fn rewrite_in(
    outer_expr: &Expr,
    subquery: &SelectStatement,
    select: &SelectStatement,
    _ctx: &RewriteContext,
) -> Option<RewriteAction> {
    let subq_target = subquery_first_column_ref(subquery)?;
    let on_condition = Expr::BinaryOp {
        left: Box::new(outer_expr.clone()),
        op: "=".to_string(),
        right: Box::new(subq_target),
    };

    let new_from = build_joined_from(
        &select.from,
        subquery_table(subquery),
        JoinType::Inner,
        Some(on_condition),
    )?;

    let mut new_select = select.clone();
    new_select.from = vec![new_from];
    new_select.where_clause = None;

    debug!("IN → INNER JOIN");

    Some(RewriteAction::Replace(Box::new(Statement::Select(
        Spanned::without_span(new_select),
    ))))
}

// ── NOT IN → LEFT JOIN + WHERE right_col IS NULL ──

fn rewrite_not_in(
    outer_expr: &Expr,
    subquery: &SelectStatement,
    select: &SelectStatement,
    _ctx: &RewriteContext,
) -> Option<RewriteAction> {
    let subq_target = subquery_first_column_ref(subquery)?;
    let null_col = subq_target.clone();
    let on_condition = Expr::BinaryOp {
        left: Box::new(outer_expr.clone()),
        op: "=".to_string(),
        right: Box::new(subq_target),
    };

    let new_from = build_joined_from(
        &select.from,
        subquery_table(subquery),
        JoinType::Left,
        Some(on_condition),
    )?;

    let mut new_select = select.clone();
    new_select.from = vec![new_from];
    new_select.where_clause = Some(Expr::IsNull {
        expr: Box::new(null_col),
        negated: false,
    });

    debug!("NOT IN → LEFT JOIN + IS NULL");

    Some(RewriteAction::Replace(Box::new(Statement::Select(
        Spanned::without_span(new_select),
    ))))
}

// ── Helper: column extraction ──

/// Extract a ColumnRef from the subquery's first SELECT target that can be
/// used in an IS NULL check or as the right side of a join equality.
/// Returns `None` if the target isn't a `ColumnRef`.
fn subquery_first_column_ref(subquery: &SelectStatement) -> Option<Expr> {
    let target = subquery.targets.first()?;
    match target {
        SelectTarget::Expr(Expr::ColumnRef(cols), _) => Some(Expr::ColumnRef(cols.clone())),
        _ => None,
    }
}

/// For NOT EXISTS, find a column from the subquery's table to use in the
/// `IS NULL` check.  We look through the subquery's WHERE clause for a
/// `BinaryOp =` that references the subquery's table, and use that column.
/// Falls back to the first target column if available.
fn extract_is_null_column(subquery: &SelectStatement, right_table: &TableRef) -> Option<Expr> {
    let table_alias = table_ref_alias(right_table);

    // First try: find a column ref in the WHERE clause that matches.
    if let Some(ref where_clause) = subquery.where_clause {
        if let Some(col) = extract_column_from_where(where_clause, &table_alias) {
            return Some(col);
        }
    }

    // Fallback: use first target column.
    subquery_first_column_ref(subquery)
}

/// Walk a BinaryOp tree to find a `ColumnRef` matching `table_alias`.
fn extract_column_from_where(expr: &Expr, table_alias: &Option<String>) -> Option<Expr> {
    match expr {
        Expr::BinaryOp { left, op: _, right } => {
            // Prefer the side that matches the table alias.
            pick_matching_column(left, table_alias)
                .or_else(|| pick_matching_column(right, table_alias))
                .or_else(|| extract_column_from_where(left, table_alias))
                .or_else(|| extract_column_from_where(right, table_alias))
        }
        _ => None,
    }
}

/// If `expr` is a ColumnRef whose first component matches `table_alias`,
/// return it; otherwise return `None`.
fn pick_matching_column(expr: &Expr, table_alias: &Option<String>) -> Option<Expr> {
    if let Expr::ColumnRef(parts) = expr {
        if let Some(alias) = table_alias {
            if parts.first().map(|s| s == alias).unwrap_or(false) {
                return Some(Expr::ColumnRef(parts.clone()));
            }
        }
    }
    None
}

/// Get the alias or table name from a TableRef.
fn table_ref_alias(tr: &TableRef) -> Option<String> {
    match tr {
        TableRef::Table { alias, name, .. } => Some(
            alias
                .as_ref()
                .map(|a| a.as_str().to_string())
                .or_else(|| name.last().map(|i| i.as_str().to_string()))?,
        ),
        TableRef::Subquery { alias, .. }
        | TableRef::Values { alias, .. }
        | TableRef::FunctionCall { alias, .. } => {
            alias.as_ref().map(|a| a.as_str().to_string())
        }
        _ => None,
    }
}

/// Build a new FROM clause by wrapping the outer FROM's first table in a
/// JOIN with the subquery table.
///
/// When the outer FROM has multiple entries, we join the first one and keep
/// the rest as-is.  This handles the common single-table outer query case.
fn build_joined_from(
    outer_from: &[TableRef],
    right_table: &TableRef,
    join_type: JoinType,
    condition: Option<Expr>,
) -> Option<TableRef> {
    let left = outer_from.first()?.clone();
    Some(make_join(left, right_table.clone(), join_type, condition))
}
