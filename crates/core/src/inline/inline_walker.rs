//! Internal walker functions for parameter substitution in AST nodes.
//!
//! This module contains the recursive tree-walking logic that traverses
//! SQL statement ASTs and replaces parameter placeholders with literal values.
//! It is a sibling module of `inline` and uses its types via `super::`.

use crate::inline::{InlineParams, InlineStats, RemainingPlaceholder};
use ogsql_parser::ast::{
    self, ConnectByClause, Expr, GroupByItem, InsertSource, OrderByItem, SelectStatement,
    SelectTarget, SetOperation, TableRef, UpdateAssignment, WhenClause, WithClause,
};
use std::collections::HashSet;

pub(crate) fn inline_select_mut(
    select: &mut SelectStatement,
    params: &InlineParams,
    known_vars: Option<&HashSet<String>>,
    pos_counter: &mut usize,
    stats: &mut InlineStats,
) {
    for target in &mut select.targets {
        if let SelectTarget::Expr(ref mut expr, _) = target {
            *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
        }
    }

    for expr in &mut select.distinct_on {
        *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
    }

    if let Some(ref mut targets) = select.into_targets {
        for target in targets {
            if let SelectTarget::Expr(ref mut expr, _) = target {
                *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
            }
        }
    }

    for table_ref in &mut select.from {
        substitute_table_ref(table_ref, params, known_vars, pos_counter, stats);
    }

    if let Some(ref mut expr) = select.where_clause {
        *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
    }

    if let Some(ref mut cb) = select.connect_by {
        substitute_connect_by(cb, params, known_vars, pos_counter, stats);
    }

    for item in &mut select.group_by {
        substitute_group_by_item(item, params, known_vars, pos_counter, stats);
    }

    if let Some(ref mut expr) = select.having {
        *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
    }

    for named_win in &mut select.window_clause {
        for expr in &mut named_win.spec.partition_by {
            *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
        }
        for item in &mut named_win.spec.order_by {
            item.expr = substitute_expr(&item.expr, params, known_vars, pos_counter, stats);
            if let Some(ref mut expr) = item.using {
                *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
            }
        }
    }

    for item in &mut select.order_by {
        substitute_order_by_item(item, params, known_vars, pos_counter, stats);
    }

    if let Some(ref mut expr) = select.limit {
        *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
    }

    if let Some(ref mut expr) = select.offset {
        *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
    }

    if let Some(ref mut fetch) = select.fetch {
        if let Some(ref mut expr) = fetch.count {
            *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
        }
    }

    if let Some(ref mut with) = select.with {
        substitute_with_clause(with, params, known_vars, pos_counter, stats);
    }

    if let Some(ref mut set_op) = select.set_operation {
        substitute_set_operation(set_op, params, known_vars, pos_counter, stats);
    }
}

pub(crate) fn inline_update_mut(
    update: &mut ast::UpdateStatement,
    params: &InlineParams,
    known_vars: Option<&HashSet<String>>,
    pos_counter: &mut usize,
    stats: &mut InlineStats,
) {
    for table_ref in &mut update.tables {
        substitute_table_ref(table_ref, params, known_vars, pos_counter, stats);
    }

    for assignment in &mut update.assignments {
        substitute_assignment(assignment, params, known_vars, pos_counter, stats);
    }

    for table_ref in &mut update.from {
        substitute_table_ref(table_ref, params, known_vars, pos_counter, stats);
    }

    if let Some(ref mut expr) = update.where_clause {
        *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
    }

    if let Some(ref mut items) = update.order_by {
        for item in items {
            substitute_order_by_item(item, params, known_vars, pos_counter, stats);
        }
    }

    if let Some(ref mut expr) = update.limit {
        *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
    }

    for target in &mut update.returning {
        if let SelectTarget::Expr(ref mut expr, _) = target {
            *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
        }
    }

    if let Some(ref mut with) = update.with {
        substitute_with_clause(with, params, known_vars, pos_counter, stats);
    }
}

pub(crate) fn inline_delete_mut(
    delete: &mut ast::DeleteStatement,
    params: &InlineParams,
    known_vars: Option<&HashSet<String>>,
    pos_counter: &mut usize,
    stats: &mut InlineStats,
) {
    for table_ref in &mut delete.tables {
        substitute_table_ref(table_ref, params, known_vars, pos_counter, stats);
    }

    for table_ref in &mut delete.using {
        substitute_table_ref(table_ref, params, known_vars, pos_counter, stats);
    }

    if let Some(ref mut expr) = delete.where_clause {
        *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
    }

    if let Some(ref mut items) = delete.order_by {
        for item in items {
            substitute_order_by_item(item, params, known_vars, pos_counter, stats);
        }
    }

    if let Some(ref mut expr) = delete.limit {
        *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
    }

    for target in &mut delete.returning {
        if let SelectTarget::Expr(ref mut expr, _) = target {
            *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
        }
    }

    if let Some(ref mut with) = delete.with {
        substitute_with_clause(with, params, known_vars, pos_counter, stats);
    }
}

pub(crate) fn inline_insert_mut(
    insert: &mut ast::InsertStatement,
    params: &InlineParams,
    known_vars: Option<&HashSet<String>>,
    pos_counter: &mut usize,
    stats: &mut InlineStats,
) {
    match &mut insert.source {
        InsertSource::Values(rows) => {
            for row in rows {
                for expr in row {
                    *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
                }
            }
        }
        InsertSource::Select(ref mut select) => {
            inline_select_mut(select, params, known_vars, pos_counter, stats);
        }
        InsertSource::Set(assignments) => {
            for assignment in assignments {
                substitute_assignment(assignment, params, known_vars, pos_counter, stats);
            }
        }
        InsertSource::RecordVariable(ref mut expr) => {
            *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
        }
        InsertSource::DefaultValues => {}
    }

    for target in &mut insert.returning {
        if let SelectTarget::Expr(ref mut expr, _) = target {
            *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
        }
    }

    if let Some(ref mut odk) = insert.on_duplicate_key {
        for assignment in &mut odk.assignments {
            substitute_assignment(assignment, params, known_vars, pos_counter, stats);
        }
        if let Some(ref mut expr) = odk.where_clause {
            *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
        }
    }

    if let Some(ref mut oc) = insert.on_conflict {
        if let ast::ConflictAction::DoUpdate {
            ref mut assignments,
            ref mut where_clause,
        } = oc.action
        {
            for assignment in assignments {
                substitute_assignment(assignment, params, known_vars, pos_counter, stats);
            }
            if let Some(ref mut expr) = where_clause {
                *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
            }
        }
    }

    if let Some(ref mut with) = insert.with {
        substitute_with_clause(with, params, known_vars, pos_counter, stats);
    }
}

pub(crate) fn inline_merge_mut(
    merge: &mut ast::MergeStatement,
    params: &InlineParams,
    known_vars: Option<&HashSet<String>>,
    pos_counter: &mut usize,
    stats: &mut InlineStats,
) {
    substitute_table_ref(&mut merge.target, params, known_vars, pos_counter, stats);
    substitute_table_ref(&mut merge.source, params, known_vars, pos_counter, stats);
    merge.on_condition =
        substitute_expr(&merge.on_condition, params, known_vars, pos_counter, stats);

    for clause in &mut merge.when_clauses {
        if let Some(ref mut expr) = clause.where_clause {
            *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
        }
        match &mut clause.action {
            ast::MergeAction::Update(assignments) => {
                for assignment in assignments {
                    substitute_assignment(assignment, params, known_vars, pos_counter, stats);
                }
            }
            ast::MergeAction::Insert { ref mut values, .. } => {
                for expr in values {
                    *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
                }
            }
            ast::MergeAction::Delete => {}
        }
    }
}

fn substitute_with_clause(
    with: &mut WithClause,
    params: &InlineParams,
    known_vars: Option<&HashSet<String>>,
    pos_counter: &mut usize,
    stats: &mut InlineStats,
) {
    for cte in &mut with.ctes {
        inline_select_mut(&mut cte.query, params, known_vars, pos_counter, stats);
    }
}

fn substitute_set_operation(
    set_op: &mut SetOperation,
    params: &InlineParams,
    known_vars: Option<&HashSet<String>>,
    pos_counter: &mut usize,
    stats: &mut InlineStats,
) {
    match set_op {
        SetOperation::Union { right, .. }
        | SetOperation::Intersect { right, .. }
        | SetOperation::Except { right, .. } => {
            inline_select_mut(right, params, known_vars, pos_counter, stats);
        }
    }
}

fn substitute_connect_by(
    cb: &mut ConnectByClause,
    params: &InlineParams,
    known_vars: Option<&HashSet<String>>,
    pos_counter: &mut usize,
    stats: &mut InlineStats,
) {
    cb.condition = substitute_expr(&cb.condition, params, known_vars, pos_counter, stats);
    if let Some(ref mut expr) = cb.start_with {
        *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
    }
}

fn substitute_group_by_item(
    item: &mut GroupByItem,
    params: &InlineParams,
    known_vars: Option<&HashSet<String>>,
    pos_counter: &mut usize,
    stats: &mut InlineStats,
) {
    match item {
        GroupByItem::Expr(ref mut expr) => {
            *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
        }
        GroupByItem::GroupingSets(sets) => {
            for exprs in sets {
                for expr in exprs {
                    *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
                }
            }
        }
        GroupByItem::Rollup(exprs) | GroupByItem::Cube(exprs) => {
            for expr in exprs {
                *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
            }
        }
    }
}

fn substitute_order_by_item(
    item: &mut OrderByItem,
    params: &InlineParams,
    known_vars: Option<&HashSet<String>>,
    pos_counter: &mut usize,
    stats: &mut InlineStats,
) {
    item.expr = substitute_expr(&item.expr, params, known_vars, pos_counter, stats);
    if let Some(ref mut expr) = item.using {
        *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
    }
}

fn substitute_assignment(
    assignment: &mut UpdateAssignment,
    params: &InlineParams,
    known_vars: Option<&HashSet<String>>,
    pos_counter: &mut usize,
    stats: &mut InlineStats,
) {
    assignment.value = substitute_expr(&assignment.value, params, known_vars, pos_counter, stats);
}

fn substitute_table_ref(
    tr: &mut TableRef,
    params: &InlineParams,
    known_vars: Option<&HashSet<String>>,
    pos_counter: &mut usize,
    stats: &mut InlineStats,
) {
    match tr {
        TableRef::Join {
            left,
            right,
            condition,
            ..
        } => {
            substitute_table_ref(left, params, known_vars, pos_counter, stats);
            substitute_table_ref(right, params, known_vars, pos_counter, stats);
            if let Some(ref mut expr) = condition {
                *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
            }
        }
        TableRef::Subquery { query, .. } => {
            inline_select_mut(query, params, known_vars, pos_counter, stats);
        }
        TableRef::FunctionCall { args, .. } => {
            for expr in args {
                *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
            }
        }
        TableRef::Values { values, .. } => {
            for row in &mut values.rows {
                for expr in row {
                    *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
                }
            }
            for item in &mut values.order_by {
                substitute_order_by_item(item, params, known_vars, pos_counter, stats);
            }
            if let Some(ref mut expr) = values.limit {
                *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
            }
            if let Some(ref mut expr) = values.offset {
                *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
            }
        }
        TableRef::Table {
            timecapsule,
            tablesample,
            ..
        } => {
            if let Some(ref mut expr) = timecapsule {
                *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
            }
            if let Some(ref mut ts) = tablesample {
                for expr in &mut ts.arguments {
                    *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
                }
                if let Some(ref mut expr) = ts.repeatable {
                    *expr = substitute_expr(expr, params, known_vars, pos_counter, stats);
                }
            }
        }
        TableRef::Pivot { source, pivot } => {
            substitute_table_ref(source, params, known_vars, pos_counter, stats);
            pivot.aggregate =
                substitute_expr(&pivot.aggregate, params, known_vars, pos_counter, stats);
            for pv in &mut pivot.values {
                pv.value = substitute_expr(&pv.value, params, known_vars, pos_counter, stats);
            }
        }
        TableRef::Unpivot { source, unpivot } => {
            substitute_table_ref(source, params, known_vars, pos_counter, stats);
            for pv in &mut unpivot.columns {
                pv.value = substitute_expr(&pv.value, params, known_vars, pos_counter, stats);
            }
        }
    }
}

/// Substitute parameters within an [`Expr`] tree.
///
/// Recursively walks the expression tree, replacing parameter nodes with their
/// corresponding literal values from `params`. The `pos_counter` tracks the
/// current position for JDBC `?` parameters (which advance sequentially).
fn substitute_expr(
    expr: &Expr,
    params: &InlineParams,
    known_vars: Option<&HashSet<String>>,
    pos_counter: &mut usize,
    stats: &mut InlineStats,
) -> Expr {
    match expr {
        Expr::JdbcParam => match params.positional.get(*pos_counter) {
            Some(val) => {
                *pos_counter += 1;
                stats.replaced_positional += 1;
                val.to_expr()
            }
            None => {
                stats.remaining.push(RemainingPlaceholder {
                    kind: "jdbc",
                    name: None,
                    position: Some(*pos_counter),
                });
                *pos_counter += 1;
                expr.clone()
            }
        },

        Expr::Parameter(n) => {
            // Parameter(n) is 1-indexed: $1 → positional[0]
            let idx = (*n as usize).saturating_sub(1);
            match params.positional.get(idx) {
                Some(val) => {
                    stats.replaced_positional += 1;
                    val.to_expr()
                }
                None => {
                    stats.remaining.push(RemainingPlaceholder {
                        kind: "parameter",
                        name: None,
                        position: Some(*n as usize),
                    });
                    expr.clone()
                }
            }
        }

        Expr::MyBatisParam(name) | Expr::MyBatisRawExpr(name) => {
            match params.named.get(name.as_str()) {
                Some(val) => {
                    stats.replaced_named += 1;
                    val.to_expr()
                }
                None => {
                    stats.remaining.push(RemainingPlaceholder {
                        kind: "mybatis",
                        name: Some(name.clone()),
                        position: None,
                    });
                    expr.clone()
                }
            }
        }

        Expr::ColumnRef(parts) | Expr::ColumnRefOuterJoin(parts) => {
            if let Some(name) = parts.last() {
                let name_lower = name.to_lowercase();

                if let Some(vars) = known_vars {
                    // Strict whitelist mode (--procedure was provided):
                    // only substitute names that are declared variables.
                    if vars.contains(&name_lower) {
                        let val = params
                            .named
                            .get(name.as_str())
                            .or_else(|| params.named.get(&name_lower));
                        return match val {
                            Some(val) => {
                                stats.replaced_named += 1;
                                val.to_expr()
                            }
                            None => {
                                stats.remaining.push(RemainingPlaceholder {
                                    kind: "variable",
                                    name: Some(name.to_string()),
                                    position: None,
                                });
                                expr.clone()
                            }
                        };
                    }
                    // Name not in whitelist → treat as real column, no fallback.
                    return expr.clone();
                }

                // Fallback mode (no --procedure): if the user explicitly provided a
                // --param with this name, treat the ColumnRef as a variable.
                // Without this, --param would be silently ignored for bare SQL files.
                if let Some(val) = params
                    .named
                    .get(name.as_str())
                    .or_else(|| params.named.get(&name_lower))
                {
                    stats.replaced_named += 1;
                    return val.to_expr();
                }
            }
            expr.clone()
        }

        Expr::PlVariable(parts) => {
            if let Some(name) = parts.last() {
                let name_lower = name.to_lowercase();
                let val = params
                    .named
                    .get(name.as_str())
                    .or_else(|| params.named.get(&name_lower));
                return match val {
                    Some(val) => {
                        stats.replaced_named += 1;
                        val.to_expr()
                    }
                    None => {
                        stats.remaining.push(RemainingPlaceholder {
                            kind: "variable",
                            name: Some(name.to_string()),
                            position: None,
                        });
                        expr.clone()
                    }
                };
            }
            expr.clone()
        }

        Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
            left: Box::new(substitute_expr(
                left,
                params,
                known_vars,
                pos_counter,
                stats,
            )),
            op: op.clone(),
            right: Box::new(substitute_expr(
                right,
                params,
                known_vars,
                pos_counter,
                stats,
            )),
        },

        Expr::UnaryOp { op, expr: inner } => Expr::UnaryOp {
            op: op.clone(),
            expr: Box::new(substitute_expr(
                inner,
                params,
                known_vars,
                pos_counter,
                stats,
            )),
        },

        Expr::Parenthesized(inner) => Expr::Parenthesized(Box::new(substitute_expr(
            inner,
            params,
            known_vars,
            pos_counter,
            stats,
        ))),

        Expr::IsNull {
            expr: inner,
            negated,
        } => Expr::IsNull {
            expr: Box::new(substitute_expr(
                inner,
                params,
                known_vars,
                pos_counter,
                stats,
            )),
            negated: *negated,
        },

        Expr::IsBoolean {
            expr: inner,
            value,
            negated,
        } => Expr::IsBoolean {
            expr: Box::new(substitute_expr(
                inner,
                params,
                known_vars,
                pos_counter,
                stats,
            )),
            value: *value,
            negated: *negated,
        },

        Expr::TypeCast {
            expr: inner,
            type_name,
            default,
            format,
        } => Expr::TypeCast {
            expr: Box::new(substitute_expr(
                inner,
                params,
                known_vars,
                pos_counter,
                stats,
            )),
            type_name: type_name.clone(),
            default: default
                .as_ref()
                .map(|d| Box::new(substitute_expr(d, params, known_vars, pos_counter, stats))),
            format: format
                .as_ref()
                .map(|f| Box::new(substitute_expr(f, params, known_vars, pos_counter, stats))),
        },

        Expr::Treat {
            expr: inner,
            type_name,
        } => Expr::Treat {
            expr: Box::new(substitute_expr(
                inner,
                params,
                known_vars,
                pos_counter,
                stats,
            )),
            type_name: type_name.clone(),
        },

        Expr::FunctionCall {
            name,
            args,
            distinct,
            over,
            filter,
            within_group,
            separator,
            default,
            conversion_format,
            agg_from,
            builtin,
        } => Expr::FunctionCall {
            name: name.clone(),
            args: args
                .iter()
                .map(|a| substitute_expr(a, params, known_vars, pos_counter, stats))
                .collect(),
            distinct: *distinct,
            over: over.clone(),
            filter: filter
                .as_ref()
                .map(|f| Box::new(substitute_expr(f, params, known_vars, pos_counter, stats))),
            within_group: within_group
                .iter()
                .map(|item| {
                    let mut new_item = item.clone();
                    new_item.expr =
                        substitute_expr(&item.expr, params, known_vars, pos_counter, stats);
                    new_item
                })
                .collect(),
            separator: separator
                .as_ref()
                .map(|s| Box::new(substitute_expr(s, params, known_vars, pos_counter, stats))),
            default: default
                .as_ref()
                .map(|d| Box::new(substitute_expr(d, params, known_vars, pos_counter, stats))),
            conversion_format: conversion_format
                .as_ref()
                .map(|cf| Box::new(substitute_expr(cf, params, known_vars, pos_counter, stats))),
            agg_from: agg_from.clone(),
            builtin: builtin.clone(),
        },

        Expr::SpecialFunction { name, args, .. } => Expr::SpecialFunction {
            name: name.clone(),
            args: args
                .iter()
                .map(|a| substitute_expr(a, params, known_vars, pos_counter, stats))
                .collect(),
            builtin: None,
        },

        Expr::Case {
            operand,
            whens,
            else_expr,
        } => Expr::Case {
            operand: operand
                .as_ref()
                .map(|o| Box::new(substitute_expr(o, params, known_vars, pos_counter, stats))),
            whens: whens
                .iter()
                .map(|w| WhenClause {
                    condition: substitute_expr(
                        &w.condition,
                        params,
                        known_vars,
                        pos_counter,
                        stats,
                    ),
                    result: substitute_expr(&w.result, params, known_vars, pos_counter, stats),
                })
                .collect(),
            else_expr: else_expr
                .as_ref()
                .map(|e| Box::new(substitute_expr(e, params, known_vars, pos_counter, stats))),
        },

        Expr::Between {
            expr: inner,
            low,
            high,
            negated,
        } => Expr::Between {
            expr: Box::new(substitute_expr(
                inner,
                params,
                known_vars,
                pos_counter,
                stats,
            )),
            low: Box::new(substitute_expr(low, params, known_vars, pos_counter, stats)),
            high: Box::new(substitute_expr(
                high,
                params,
                known_vars,
                pos_counter,
                stats,
            )),
            negated: *negated,
        },

        Expr::InList {
            expr: inner,
            list,
            negated,
        } => Expr::InList {
            expr: Box::new(substitute_expr(
                inner,
                params,
                known_vars,
                pos_counter,
                stats,
            )),
            list: list
                .iter()
                .map(|e| substitute_expr(e, params, known_vars, pos_counter, stats))
                .collect(),
            negated: *negated,
        },

        Expr::Like {
            expr: inner,
            pattern,
            escape,
            negated,
            case_insensitive,
        } => Expr::Like {
            expr: Box::new(substitute_expr(
                inner,
                params,
                known_vars,
                pos_counter,
                stats,
            )),
            pattern: Box::new(substitute_expr(
                pattern,
                params,
                known_vars,
                pos_counter,
                stats,
            )),
            escape: escape
                .as_ref()
                .map(|e| Box::new(substitute_expr(e, params, known_vars, pos_counter, stats))),
            negated: *negated,
            case_insensitive: *case_insensitive,
        },

        Expr::Subscript {
            object,
            lower,
            upper,
            is_slice,
        } => Expr::Subscript {
            object: Box::new(substitute_expr(
                object,
                params,
                known_vars,
                pos_counter,
                stats,
            )),
            lower: lower
                .as_ref()
                .map(|l| Box::new(substitute_expr(l, params, known_vars, pos_counter, stats))),
            upper: upper
                .as_ref()
                .map(|u| Box::new(substitute_expr(u, params, known_vars, pos_counter, stats))),
            is_slice: *is_slice,
        },

        Expr::Array(exprs) => Expr::Array(
            exprs
                .iter()
                .map(|e| substitute_expr(e, params, known_vars, pos_counter, stats))
                .collect(),
        ),

        Expr::RowConstructor(exprs) => Expr::RowConstructor(
            exprs
                .iter()
                .map(|e| substitute_expr(e, params, known_vars, pos_counter, stats))
                .collect(),
        ),

        Expr::CollationFor { expr: inner } => Expr::CollationFor {
            expr: Box::new(substitute_expr(
                inner,
                params,
                known_vars,
                pos_counter,
                stats,
            )),
        },

        Expr::Prior(inner) => Expr::Prior(Box::new(substitute_expr(
            inner,
            params,
            known_vars,
            pos_counter,
            stats,
        ))),

        Expr::FieldAccess {
            object: inner,
            field,
        } => Expr::FieldAccess {
            object: Box::new(substitute_expr(
                inner,
                params,
                known_vars,
                pos_counter,
                stats,
            )),
            field: field.clone(),
        },

        Expr::Exists(select) => {
            let mut select = *select.clone();
            inline_select_mut(&mut select, params, known_vars, pos_counter, stats);
            Expr::Exists(Box::new(select))
        }

        Expr::Subquery(select) => {
            let mut select = *select.clone();
            inline_select_mut(&mut select, params, known_vars, pos_counter, stats);
            Expr::Subquery(Box::new(select))
        }

        Expr::InSubquery {
            expr: inner,
            subquery,
            negated,
        } => {
            let mut subquery = *subquery.clone();
            inline_select_mut(&mut subquery, params, known_vars, pos_counter, stats);
            Expr::InSubquery {
                expr: Box::new(substitute_expr(
                    inner,
                    params,
                    known_vars,
                    pos_counter,
                    stats,
                )),
                subquery: Box::new(subquery),
                negated: *negated,
            }
        }

        Expr::ScalarSublink {
            expr: inner,
            op,
            sublink_type,
            subquery,
        } => {
            let mut subquery = *subquery.clone();
            inline_select_mut(&mut subquery, params, known_vars, pos_counter, stats);
            Expr::ScalarSublink {
                expr: Box::new(substitute_expr(
                    inner,
                    params,
                    known_vars,
                    pos_counter,
                    stats,
                )),
                op: op.clone(),
                sublink_type: sublink_type.clone(),
                subquery: Box::new(subquery),
            }
        }

        Expr::Literal(_)
        | Expr::QualifiedStar(_)
        | Expr::Default
        | Expr::CurrentOf { .. }
        | Expr::PredictBy { .. }
        | Expr::SysDate
        | Expr::SequenceValue { .. }
        | Expr::CursorAttribute { .. } => expr.clone(),

        Expr::XmlElement { .. }
        | Expr::XmlConcat(_)
        | Expr::XmlForest(_)
        | Expr::XmlParse { .. }
        | Expr::XmlPi { .. }
        | Expr::XmlRoot { .. }
        | Expr::XmlSerialize { .. } => expr.clone(),
    }
}
