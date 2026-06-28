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
use ogsql_parser::ast::{DataType, Expr, Ident, Literal, Statement};
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
    /// SQL typed literal: `<value>::<type>` (e.g. `'20260101'::date`, `42::int`).
    ///
    /// The base `value` is inferred normally from the text before `::`; the
    /// `type_name` is preserved verbatim (including any precision/length such
    /// as `numeric(10,2)`).
    Cast {
        /// The base literal value (left-hand side of `::`).
        value: Box<InlineValue>,
        /// The target SQL type name (right-hand side of `::`), e.g. `date`.
        type_name: String,
    },
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
    /// assert_eq!(
    ///     InlineValue::Cast { value: Box::new(InlineValue::String("20260101".into())), type_name: "date".into() }.to_sql_literal(),
    ///     "'20260101'::date"
    /// );
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
            Self::Cast { value, type_name } => {
                format!("{}::{}", value.to_sql_literal(), type_name)
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
            Self::Cast { value, type_name } => Expr::TypeCast {
                expr: Box::new(value.to_expr()),
                // Use `DataType::Custom` with the type name as a single unquoted
                // ident so the formatter reproduces the user's exact spelling
                // (including precision/length such as `numeric(10,2)`).
                type_name: DataType::Custom(
                    vec![Ident {
                        value: type_name.clone(),
                        quote_style: None,
                    }],
                    vec![],
                ),
                default: None,
                format: None,
            },
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
/// - **Cast**: a `<base>::<type>` suffix (with `::` outside any quotes) →
///   [`Cast`](InlineValue::Cast); `base` is inferred recursively (e.g.
///   `'20260101'::date`, `42::int`, `'5'::numeric(10,2)`).
/// - `"NULL"` (case insensitive) → [`Null`](InlineValue::Null)
/// - `"TRUE"` / `"FALSE"` (case insensitive) → [`Boolean`](InlineValue::Boolean)
/// - Rule 2: SQL-style single-quoted string (e.g. `"'O''Brien'"`, `"'001'"`) →
///   [`String`](InlineValue::String) with `''` unescaped to `'`
/// - Rule 1: Leading-zero all-digit string (e.g. `"001"`, `"010"`) →
///   [`String`](InlineValue::String) (preserves numeric codes)
/// - Parses as `i64` → [`Integer`](InlineValue::Integer)
/// - Parses as `f64` → [`Float`](InlineValue::Float) (stores original string)
/// - Otherwise → [`String`](InlineValue::String)
pub fn infer_value(s: &str) -> InlineValue {
    if let Some((base, type_name)) = split_cast(s) {
        let type_name = type_name.trim();
        if !type_name.is_empty() && looks_like_type_name(type_name) {
            return InlineValue::Cast {
                value: Box::new(infer_value(base)),
                type_name: type_name.to_string(),
            };
        }
    }

    match s.to_uppercase().as_str() {
        "NULL" => return InlineValue::Null,
        "TRUE" => return InlineValue::Boolean(true),
        "FALSE" => return InlineValue::Boolean(false),
        _ => {}
    }

    // Rule 2: `'...'` forces string type; SQL `''` escape → `'`.
    if s.len() >= 2 && s.starts_with('\'') && s.ends_with('\'') {
        let inner = &s[1..s.len() - 1];
        return InlineValue::String(inner.replace("''", "'"));
    }

    // Rule 1: leading-zero digits (e.g. "001") → String; bare "0" stays Integer.
    if s.len() > 1 && s.starts_with('0') && s.bytes().all(|b| b.is_ascii_digit()) {
        return InlineValue::String(s.to_string());
    }

    if let Ok(n) = s.parse::<i64>() {
        return InlineValue::Integer(n);
    }
    if s.parse::<f64>().is_ok() {
        return InlineValue::Float(s.to_string());
    }
    InlineValue::String(s.to_string())
}

/// Split `<base>::<type>` at the first `::` outside any single-quoted region,
/// so quoted text like `'a::b'` is not mistaken for a cast. Returns `None` if
/// there is no top-level `::` or the base is empty.
fn split_cast(s: &str) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut in_quote = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_quote {
            if c == b'\'' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    i += 2;
                    continue;
                }
                in_quote = false;
            }
            i += 1;
            continue;
        }
        if c == b'\'' {
            in_quote = true;
            i += 1;
            continue;
        }
        if c == b':' && i + 1 < bytes.len() && bytes[i + 1] == b':' {
            let base = &s[..i];
            if base.is_empty() {
                return None;
            }
            return Some((base, &s[i + 2..]));
        }
        i += 1;
    }
    None
}

fn looks_like_type_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    s.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || c == '_'
            || c == '('
            || c == ')'
            || c == ','
            || c == '.'
            || c == ' '
    })
}

/// Replace parameters/placeholders in a parsed SQL statement with literal values.
///
/// # Parameters
///
/// * `stmt` — The parsed SQL [`Statement`].
/// * `params` — Named and positional parameter values to substitute.
/// * `known_variables` — Optional set of variable names from a stored procedure
///   declaration. When provided, only names in this set are substituted
///   (whitelist is exclusive). When `None`, ColumnRef nodes whose last segment
///   matches a key in `params.named` are substituted (user-provided --param
///   values serve as an implicit variable whitelist).
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
