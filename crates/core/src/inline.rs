//! Parameter substitution engine — replaces SQL parameter placeholders
//! with literal values to produce directly executable SQL.
//!
//! Supports three parameter styles:
//! - JDBC `?` positional parameters
//! - PostgreSQL-style `$1`, `$2` numbered parameters
//! - MyBatis `#{name}` / `${name}` named parameters
//! - Stored procedure variables (ColumnRef with known_variables gate)

mod inline_walker;

use inline_walker::{
    inline_delete_mut, inline_insert_mut, inline_merge_mut, inline_select_mut, inline_update_mut,
};
use ogsql_parser::ast::{Expr, Literal, Statement};
use std::collections::{HashMap, HashSet};

/// A parameter value mapped to a SQL literal type.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum InlineValue {
    /// SQL string literal (will be single-quoted with escaped quotes).
    String(String),
    /// SQL integer literal.
    Integer(i64),
    /// SQL float literal (stored as String to match [`Literal::Float`]).
    Float(String),
    /// SQL boolean literal (`TRUE` / `FALSE`).
    Boolean(bool),
    /// SQL `NULL`.
    Null,
}

impl InlineValue {
    /// Produce the SQL literal string representation.
    ///
    /// # Examples
    ///
    /// ```
    /// # use metamorphosis_core::inline::InlineValue;
    /// assert_eq!(InlineValue::Null.to_sql_literal(), "NULL");
    /// assert_eq!(InlineValue::Boolean(true).to_sql_literal(), "TRUE");
    /// assert_eq!(InlineValue::Integer(42).to_sql_literal(), "42");
    /// assert_eq!(InlineValue::String("O'Brien".into()).to_sql_literal(), "'O''Brien'");
    /// ```
    pub fn to_sql_literal(&self) -> String {
        match self {
            Self::Null => "NULL".to_string(),
            Self::Boolean(true) => "TRUE".to_string(),
            Self::Boolean(false) => "FALSE".to_string(),
            Self::Integer(n) => n.to_string(),
            Self::Float(s) => s.clone(),
            Self::String(s) => {
                let escaped = s.replace('\'', "''");
                format!("'{}'", escaped)
            }
        }
    }

    /// Convert this value to an AST [`Expr`] node.
    pub fn to_expr(&self) -> Expr {
        match self {
            Self::Null => Expr::Literal(Literal::Null),
            Self::Boolean(b) => Expr::Literal(Literal::Boolean(*b)),
            Self::Integer(n) => Expr::Literal(Literal::Integer(*n)),
            Self::Float(s) => Expr::Literal(Literal::Float(s.clone())),
            Self::String(s) => Expr::Literal(Literal::String(s.clone())),
        }
    }
}

/// Named and positional parameter values for substitution.
#[derive(Debug, Clone, Default)]
pub struct InlineParams {
    /// Named parameters keyed by name (for MyBatis `#{name}`, `${name}`, and variables).
    pub named: HashMap<String, InlineValue>,
    /// Positional parameters in order (for JDBC `?` and `$1`, `$2`).
    pub positional: Vec<InlineValue>,
}

/// Describes a placeholder that was NOT replaced (no matching parameter value).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RemainingPlaceholder {
    /// The kind of placeholder: `"jdbc"`, `"mybatis"`, `"parameter"`, or `"variable"`.
    pub kind: &'static str,
    /// Parameter name (for named placeholders).
    pub name: Option<String>,
    /// Positional index (for positional placeholders).
    pub position: Option<usize>,
}

/// Result of inlining a single statement.
#[derive(Debug)]
#[non_exhaustive]
pub struct InlineResult {
    /// The statement with placeholders replaced (where possible).
    pub statement: Statement,
    /// Count of named parameters that were replaced.
    pub replaced_named: usize,
    /// Count of positional parameters that were replaced.
    pub replaced_positional: usize,
    /// Placeholders that could not be replaced (no matching parameter value).
    pub remaining: Vec<RemainingPlaceholder>,
}

/// Internal stats accumulator shared with [`inline_walker`].
#[derive(Debug, Default)]
pub(crate) struct InlineStats {
    pub(crate) replaced_named: usize,
    pub(crate) replaced_positional: usize,
    pub(crate) remaining: Vec<RemainingPlaceholder>,
}

/// Infer the [`InlineValue`] type from a string representation.
///
/// Rules (in order):
/// - `"NULL"` (case insensitive) → [`Null`](InlineValue::Null)
/// - `"TRUE"` / `"FALSE"` (case insensitive) → [`Boolean`](InlineValue::Boolean)
/// - Parses as `i64` → [`Integer`](InlineValue::Integer)
/// - Parses as `f64` → [`Float`](InlineValue::Float) (stores original string)
/// - Otherwise → [`String`](InlineValue::String)
pub fn infer_value(s: &str) -> InlineValue {
    match s.to_uppercase().as_str() {
        "NULL" => return InlineValue::Null,
        "TRUE" => return InlineValue::Boolean(true),
        "FALSE" => return InlineValue::Boolean(false),
        _ => {}
    }
    if let Ok(n) = s.parse::<i64>() {
        return InlineValue::Integer(n);
    }
    if s.parse::<f64>().is_ok() {
        return InlineValue::Float(s.to_string());
    }
    InlineValue::String(s.to_string())
}

/// Replace parameters/placeholders in a parsed SQL statement with literal values.
///
/// # Parameters
///
/// * `stmt` — The parsed SQL [`Statement`].
/// * `params` — Named and positional parameter values to substitute.
/// * `known_variables` — Optional set of variable names from a stored procedure
///   declaration. When provided, [`Expr::ColumnRef`] nodes whose last segment
///   matches a known variable name are replaced. When `None`, `ColumnRef` nodes
///   are never touched (only explicit parameter nodes are substituted).
pub fn inline_statement(
    stmt: &Statement,
    params: &InlineParams,
    known_variables: Option<&HashSet<String>>,
) -> InlineResult {
    let mut pos_counter: usize = 0;
    let mut stats = InlineStats::default();

    let new_stmt = match stmt {
        Statement::Select(spanned) => {
            let mut select = spanned.node.clone();
            select.into_targets = None;
            select.into_table = None;
            select.bulk_collect = false;
            inline_select_mut(
                &mut select,
                params,
                known_variables,
                &mut pos_counter,
                &mut stats,
            );
            Statement::Select(ogsql_parser::ast::Spanned::without_span(select))
        }
        Statement::Update(spanned) => {
            let mut update = spanned.node.clone();
            update.into_targets = None;
            update.bulk_collect = false;
            inline_update_mut(
                &mut update,
                params,
                known_variables,
                &mut pos_counter,
                &mut stats,
            );
            Statement::Update(ogsql_parser::ast::Spanned::without_span(update))
        }
        Statement::Delete(spanned) => {
            let mut delete = spanned.node.clone();
            delete.into_targets = None;
            delete.bulk_collect = false;
            inline_delete_mut(
                &mut delete,
                params,
                known_variables,
                &mut pos_counter,
                &mut stats,
            );
            Statement::Delete(ogsql_parser::ast::Spanned::without_span(delete))
        }
        Statement::Insert(spanned) => {
            let mut insert = spanned.node.clone();
            insert.into_targets = None;
            insert.bulk_collect = false;
            inline_insert_mut(
                &mut insert,
                params,
                known_variables,
                &mut pos_counter,
                &mut stats,
            );
            Statement::Insert(ogsql_parser::ast::Spanned::without_span(insert))
        }
        Statement::Merge(spanned) => {
            let mut merge = spanned.node.clone();
            inline_merge_mut(
                &mut merge,
                params,
                known_variables,
                &mut pos_counter,
                &mut stats,
            );
            Statement::Merge(ogsql_parser::ast::Spanned::without_span(merge))
        }
        other => other.clone(),
    };

    InlineResult {
        statement: new_stmt,
        replaced_named: stats.replaced_named,
        replaced_positional: stats.replaced_positional,
        remaining: stats.remaining,
    }
}

#[cfg(test)]
mod tests;
