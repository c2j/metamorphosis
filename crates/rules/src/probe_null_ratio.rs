//! Rule: generate probe SQL measuring NULL ratio for columns used in WHERE/JOIN conditions.
//!
//! Manual level: only generates suggestions (probe SQL), never replaces.
//!
//! # Purpose
//!
//! When query conditions reference columns with high NULL ratios, the query may produce
//! unexpected results due to SQL three-valued logic (e.g., `NOT IN` against a column
//! containing NULLs returns no rows). This rule generates a probe that shows the total
//! row count and the non-null count for each column used in the WHERE clause and JOIN
//! ON conditions, enabling the user to assess whether NULLs could affect query correctness.
//!
//! # Example
//!
//! Input:  `SELECT * FROM t WHERE col1 = 1 AND col2 = 2`
//! Probe:  `SELECT COUNT(*) AS total, COUNT(col1) AS col1_non_null, COUNT(col2) AS col2_non_null FROM t`

use metamorphosis_core::types::{
    Confidence, MatchResult, RewriteAction, RuleCategory, SafetyLevel,
};
use metamorphosis_core::{RewriteContext, RewriteRule};
use ogsql_parser::ast::{
    Expr, Literal, ObjectName, SelectStatement, SelectTarget, Spanned, Statement, TableRef,
};
use std::collections::HashSet;
use tracing::debug;

/// Rule: generate probe SQL measuring NULL ratio for columns in WHERE/JOIN conditions.
///
/// Manual level: only generates suggestions, never replaces the original SQL.
#[derive(Debug)]
pub struct ProbeNullRatio;

impl RewriteRule for ProbeNullRatio {
    fn id(&self) -> &'static str {
        "probe-null-ratio"
    }

    fn description(&self) -> &'static str {
        "Generate probe SQL measuring NULL ratio for WHERE/JOIN condition columns"
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

        let mut cols = Vec::new();

        // Extract from WHERE clause
        if let Some(where_expr) = &select.where_clause {
            extract_columns(where_expr, &mut cols);
        }

        // Extract from JOIN ON conditions
        for table_ref in &select.from {
            extract_columns_from_table_ref(table_ref, &mut cols);
        }

        if cols.is_empty() {
            MatchResult::NotMatched {
                reason: "No column references found in WHERE or JOIN conditions".to_string(),
            }
        } else {
            MatchResult::Matched
        }
    }

    fn apply(&self, _ctx: &RewriteContext, stmt: &Statement) -> Vec<RewriteAction> {
        let select = match stmt {
            Statement::Select(s) => s,
            _ => return vec![],
        };

        let mut cols = Vec::new();

        // Extract from WHERE clause
        if let Some(where_expr) = &select.where_clause {
            extract_columns(where_expr, &mut cols);
        }

        // Extract from JOIN ON conditions
        for table_ref in &select.from {
            extract_columns_from_table_ref(table_ref, &mut cols);
        }

        if cols.is_empty() {
            return vec![];
        }

        // Deduplicate columns by their qualified name string
        let mut seen = HashSet::new();
        let mut unique_cols: Vec<ObjectName> = Vec::new();
        for col in cols {
            let key: String = col.iter().map(|i| i.as_str()).collect::<Vec<_>>().join(".");
            if seen.insert(key) {
                unique_cols.push(col);
            }
        }

        let probe = build_null_ratio_probe(&select.from, &unique_cols);

        debug!(
            rule_id = self.id(),
            columns = ?unique_cols,
            "Generated null ratio probe"
        );

        let col_names: Vec<String> = unique_cols
            .iter()
            .map(|c| c.iter().map(|i| i.as_str()).collect::<Vec<_>>().join("."))
            .collect();

        vec![RewriteAction::Generate {
            stmt: Box::new(Statement::Select(probe)),
            purpose: format!(
                "NULL ratio probe for columns: [{}] — measure NULL density in WHERE/JOIN condition columns",
                col_names.join(", "),
            ),
            confidence: Confidence::High,
        }]
    }
}

/// Recursively extract columns from a [`TableRef`] tree, descending into JOIN conditions.
fn extract_columns_from_table_ref(table_ref: &TableRef, cols: &mut Vec<ObjectName>) {
    match table_ref {
        TableRef::Join {
            left,
            right,
            condition,
            ..
        } => {
            if let Some(cond) = condition {
                extract_columns(cond, cols);
            }
            extract_columns_from_table_ref(left, cols);
            extract_columns_from_table_ref(right, cols);
        }
        TableRef::Pivot { source, .. } | TableRef::Unpivot { source, .. } => {
            extract_columns_from_table_ref(source, cols);
        }
        // Table, Subquery, Values, FunctionCall — no conditions to extract
        _ => {}
    }
}

/// Recursively extract [`ColumnRef`](Expr::ColumnRef) expressions from an expression tree.
fn extract_columns(expr: &Expr, cols: &mut Vec<ObjectName>) {
    match expr {
        Expr::ColumnRef(name) | Expr::ColumnRefOuterJoin(name) => cols.push(name.clone()),
        Expr::BinaryOp { left, right, .. } => {
            extract_columns(left, cols);
            extract_columns(right, cols);
        }
        Expr::UnaryOp { expr: inner, .. } => extract_columns(inner, cols),
        Expr::IsNull { expr: inner, .. } => extract_columns(inner, cols),
        Expr::IsBoolean { expr: inner, .. } => extract_columns(inner, cols),
        Expr::Like {
            expr: inner,
            pattern,
            escape,
            ..
        } => {
            extract_columns(inner, cols);
            extract_columns(pattern, cols);
            if let Some(esc) = escape {
                extract_columns(esc, cols);
            }
        }
        Expr::Between {
            expr: inner,
            low,
            high,
            ..
        } => {
            extract_columns(inner, cols);
            extract_columns(low, cols);
            extract_columns(high, cols);
        }
        Expr::InList { expr: inner, list, .. } => {
            extract_columns(inner, cols);
            for item in list {
                extract_columns(item, cols);
            }
        }
        Expr::InSubquery { expr: inner, .. } => {
            extract_columns(inner, cols);
        }
        Expr::Parenthesized(inner) => extract_columns(inner, cols),
        Expr::Case {
            operand,
            whens,
            else_expr,
            ..
        } => {
            if let Some(op) = operand {
                extract_columns(op, cols);
            }
            for when in whens {
                extract_columns(&when.condition, cols);
                extract_columns(&when.result, cols);
            }
            if let Some(ee) = else_expr {
                extract_columns(ee, cols);
            }
        }
        Expr::FunctionCall { args, .. } => {
            for arg in args {
                extract_columns(arg, cols);
            }
        }
        Expr::SpecialFunction { args, .. } => {
            for arg in args {
                extract_columns(arg, cols);
            }
        }
        Expr::Subscript {
            object,
            lower,
            upper,
            ..
        } => {
            extract_columns(object, cols);
            if let Some(l) = lower {
                extract_columns(l, cols);
            }
            if let Some(u) = upper {
                extract_columns(u, cols);
            }
        }
        Expr::FieldAccess { object, .. } => extract_columns(object, cols),
        Expr::TypeCast { expr: inner, .. } => extract_columns(inner, cols),
        Expr::Treat { expr: inner, .. } => extract_columns(inner, cols),
        Expr::CollationFor { expr: inner } => extract_columns(inner, cols),
        Expr::Prior(inner) => extract_columns(inner, cols),
        Expr::RowConstructor(items) => {
            for item in items {
                extract_columns(item, cols);
            }
        }
        Expr::Array(items) => {
            for item in items {
                extract_columns(item, cols);
            }
        }
        Expr::CursorAttribute { cursor, .. } => extract_columns(cursor, cols),
        Expr::ScalarSublink { expr: inner, .. } => extract_columns(inner, cols),
        // Remaining variants (literals, parameters, XML, etc.) contain no column refs
        _ => {}
    }
}

/// Build the null ratio probe SELECT statement.
///
/// Produces:
/// ```sql
/// SELECT COUNT(*) AS total, COUNT(col1) AS col1_non_null, COUNT(col2) AS col2_non_null, ...
/// FROM original_tables
/// ```
///
/// The WHERE clause is intentionally not preserved to measure table-wide NULL density.
fn build_null_ratio_probe(from: &[TableRef], columns: &[ObjectName]) -> Spanned<SelectStatement> {
    let mut targets: Vec<SelectTarget> = Vec::with_capacity(1 + columns.len());

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
        Some("total".into()),
    ));

    for col in columns {
        let alias: String = format!(
            "{}_non_null",
            col.iter()
                .map(|i| i.as_str())
                .collect::<Vec<_>>()
                .join("_")
        );
        targets.push(SelectTarget::Expr(
            Expr::FunctionCall {
                name: vec!["count".into()],
                args: vec![Expr::ColumnRef(col.clone())],
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
        ));
    }

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
