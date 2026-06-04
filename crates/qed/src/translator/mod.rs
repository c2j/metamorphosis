//! ogsql-parser AST → QED [`QedRelation`] tree translator.
//!
//! Translates a parsed [`Statement::Select`] into the QED intermediate
//! representation suitable for equivalence verification. Non-SELECT
//! statements return [`TranslateError::UnsupportedStatement`].

use ogsql_parser::ast::{
    Expr, GroupByItem, Literal, SelectStatement, SelectTarget, SetOperation,
    Statement, TableRef,
};
use crate::ir::{QedAggArg, QedAggCall, QedExpr, QedRelation, QedValue};
use crate::schema::RichSchema;

// ── Error type ───────────────────────────────────────────────────────────

/// Errors produced during AST → QED translation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TranslateError {
    /// The statement kind is not supported (only SELECT is translated).
    #[error("unsupported statement type: {0}")]
    UnsupportedStatement(String),
    /// A column reference could not be resolved in the current scope.
    #[error("column not found: {0}")]
    ColumnNotFound(String),
    /// A referenced table does not exist in the schema.
    #[error("table not found in schema: {0}")]
    TableNotFound(String),
    /// An expression form is not yet supported by the translator.
    #[error("unsupported expression: {0}")]
    UnsupportedExpr(String),
    /// A column reference matches multiple tables (ambiguous).
    #[error("ambiguous column reference: {0}")]
    AmbiguousColumn(String),
}

// ── Column scope ─────────────────────────────────────────────────────────

/// Tracks available columns and maps name references to 0-based indices.
struct ColumnScope {
    columns: Vec<(Option<String>, String)>,
}

impl ColumnScope {
    fn from_table(
        table_name: &str, alias: Option<&str>, schema: &RichSchema,
    ) -> Result<Self, TranslateError> {
        let lower = table_name.to_lowercase();
        let info = schema.tables.get(&lower)
            .ok_or_else(|| TranslateError::TableNotFound(table_name.to_string()))?;
        let alias_key = alias.map(|a| a.to_lowercase());
        let columns = info.columns.iter()
            .map(|c| (alias_key.clone(), c.name.clone()))
            .collect();
        Ok(Self { columns })
    }

    fn join(left: Self, right: Self) -> Self {
        let mut columns = left.columns;
        columns.extend(right.columns);
        Self { columns }
    }

    fn resolve(&self, table_alias: Option<&str>, col_name: &str) -> Result<usize, TranslateError> {
        let lower = col_name.to_lowercase();
        let alias_lower = table_alias.map(|a| a.to_lowercase());
        let mut matches: Vec<usize> = self.columns.iter().enumerate()
            .filter(|(_, (tbl, col))| col == &lower && alias_lower.as_deref() == tbl.as_deref())
            .map(|(i, _)| i)
            .collect();
        if matches.is_empty() && table_alias.is_none() {
            matches = self.columns.iter().enumerate()
                .filter(|(_, (_, col))| col == &lower)
                .map(|(i, _)| i).collect();
        }
        match matches.len() {
            0 => Err(TranslateError::ColumnNotFound(
                table_alias.map_or(lower.clone(), |a| format!("{a}.{lower}"))
            )),
            1 => Ok(matches[0]),
            _ => Err(TranslateError::AmbiguousColumn(lower)),
        }
    }

    fn len(&self) -> usize { self.columns.len() }
}

// ── Helpers ──────────────────────────────────────────────────────────────

const AGG_FUNCS: &[&str] = &[
    "count", "sum", "avg", "min", "max", "group_concat", "string_agg", "array_agg",
];

fn is_aggregate(name: &str) -> bool {
    AGG_FUNCS.contains(&name.to_lowercase().as_str())
}

fn is_star_arg(args: &[Expr]) -> bool {
    if args.is_empty() { return true; }
    if args.len() != 1 { return false; }
    matches!(&args[0], Expr::Literal(Literal::Integer(1)) | Expr::QualifiedStar(_))
        || matches!(&args[0], Expr::ColumnRef(n) if n.len() == 1 && n[0] == "*")
}

fn expr_column_name(expr: &Expr) -> String {
    match expr {
        Expr::ColumnRef(name) => name.last().cloned().unwrap_or_else(|| "?column?".to_string()),
        _ => "?column?".to_string(),
    }
}

fn map_binop(op: &str) -> String {
    match op.to_uppercase().as_str() {
        "=" => "eq", ">" => "gt", "<" => "lt", ">=" => "gte", "<=" => "lte",
        "<>" | "!=" => "neq", "AND" => "and", "OR" => "or",
        "+" => "add", "-" => "sub", "*" => "mul", "/" => "div", "%" => "mod",
        "||" => "concat", "IS" => "eq", "IS NOT" => "neq",
        "LIKE" => "like", "ILIKE" => "ilike",
        "~" => "regex_match", "~*" => "regex_imatch",
        "!~" => "regex_not_match", "!~*" => "regex_not_imatch",
        _ => return op.to_lowercase(),
    }.to_string()
}

fn split_column_ref(name: &[String]) -> (Option<&str>, &str) {
    match name.len() {
        1 => (None, &name[0]),
        2 => (Some(&name[0]), &name[1]),
        _ => (Some(&name[name.len() - 2]), &name[name.len() - 1]),
    }
}

fn set_op_right(op: &SetOperation) -> &SelectStatement {
    match op {
        SetOperation::Union { right, .. }
        | SetOperation::Intersect { right, .. }
        | SetOperation::Except { right, .. } => right,
    }
}

// ── Translator ───────────────────────────────────────────────────────────

/// Translates ogsql-parser AST statements into QED relation trees.
pub struct AstTranslator<'a> {
    schema: &'a RichSchema,
}

impl<'a> AstTranslator<'a> {
    /// Create a new translator with the given schema for name resolution.
    pub fn new(schema: &'a RichSchema) -> Self { Self { schema } }

    /// Main entry point. Translates a [`Statement`] to a [`QedRelation`].
    ///
    /// Only `Statement::Select` is supported. Other kinds return
    /// [`TranslateError::UnsupportedStatement`].
    pub fn translate(&self, stmt: &Statement) -> Result<QedRelation, TranslateError> {
        match stmt {
            Statement::Select(spanned) => self.translate_select(&spanned.node),
            _ => Err(TranslateError::UnsupportedStatement(format!("{stmt:?}"))),
        }
    }

    fn translate_select(&self, select: &SelectStatement) -> Result<QedRelation, TranslateError> {
        let mut rel = if select.from.is_empty() {
            QedRelation::Values { rows: vec![vec![]] }
        } else {
            self.translate_from(&select.from)?
        };
        let scope = self.build_scope_from(&select.from)?;
        let has_agg = self.targets_have_aggregates(&select.targets);

        if let Some(ref wc) = select.where_clause {
            rel = QedRelation::Filter { condition: self.translate_expr(wc, &scope)?, input: Box::new(rel) };
        }

        if !select.group_by.is_empty() || has_agg {
            rel = self.translate_group_by(&select.group_by, &select.targets, &select.having, rel, &scope)?;
        }

        if !self.is_simple_star(&select.targets, &scope) {
            rel = self.translate_projection(&select.targets, rel, &scope)?;
        }

        if select.distinct {
            rel = QedRelation::Distinct { input: Box::new(rel) };
        }

        if !select.order_by.is_empty() {
            let args = select.order_by.iter().map(|item| {
                let expr = self.translate_expr(&item.expr, &scope)?;
                let dir = QedExpr::Literal { value: QedValue::String {
                    value: if item.asc == Some(false) { "desc" } else { "asc" }.to_string(),
                }};
                Ok(QedExpr::Function { name: "SortKey".to_string(), args: vec![expr, dir] })
            }).collect::<Result<Vec<_>, TranslateError>>()?;
            rel = QedRelation::QOp { name: "Sort".to_string(), args, input: Box::new(rel) };
        }

        if let Some(ref e) = select.limit {
            rel = QedRelation::QOp { name: "Limit".to_string(), args: vec![self.translate_expr(e, &scope)?], input: Box::new(rel) };
        }
        if let Some(ref e) = select.offset {
            rel = QedRelation::QOp { name: "Offset".to_string(), args: vec![self.translate_expr(e, &scope)?], input: Box::new(rel) };
        }
        if let Some(ref fetch) = select.fetch {
            if let Some(ref e) = fetch.count {
                rel = QedRelation::QOp { name: "Limit".to_string(), args: vec![self.translate_expr(e, &scope)?], input: Box::new(rel) };
            }
        }

        if let Some(ref set_op) = select.set_operation {
            let right = self.translate_select(set_op_right(set_op))?;
            rel = match set_op {
                SetOperation::Union { all: true, .. } => QedRelation::Union { left: Box::new(rel), right: Box::new(right) },
                SetOperation::Union { all: false, .. } => QedRelation::Distinct {
                    input: Box::new(QedRelation::Union { left: Box::new(rel), right: Box::new(right) }),
                },
                SetOperation::Intersect { .. } => QedRelation::Intersect { left: Box::new(rel), right: Box::new(right) },
                SetOperation::Except { .. } => QedRelation::Except { left: Box::new(rel), right: Box::new(right) },
            };
        }
        Ok(rel)
    }

    fn translate_from(&self, from: &[TableRef]) -> Result<QedRelation, TranslateError> {
        let (mut rel, _) = self.translate_table_ref(&from[0])?;
        for tr in &from[1..] {
            let (right_rel, _) = self.translate_table_ref(tr)?;
            rel = QedRelation::Join { left: Box::new(rel), right: Box::new(right_rel), condition: None };
        }
        Ok(rel)
    }

    fn build_scope_from(&self, from: &[TableRef]) -> Result<ColumnScope, TranslateError> {
        if from.is_empty() { return Ok(ColumnScope { columns: vec![] }); }
        let mut scope = self.scope_for_table_ref(&from[0])?;
        for tr in &from[1..] { scope = ColumnScope::join(scope, self.scope_for_table_ref(tr)?); }
        Ok(scope)
    }

    fn translate_table_ref(&self, tr: &TableRef) -> Result<(QedRelation, ColumnScope), TranslateError> {
        match tr {
            TableRef::Table { name, alias, .. } => {
                let table_name = name.join(".");
                let scope = ColumnScope::from_table(&table_name, alias.as_deref(), self.schema)?;
                let rel = QedRelation::Scan { table: table_name.to_lowercase(), fields: vec![] };
                Ok((rel, scope))
            }
            TableRef::Join { left, right, condition, .. } => {
                let (l_rel, l_scope) = self.translate_table_ref(left)?;
                let (r_rel, r_scope) = self.translate_table_ref(right)?;
                let joined = ColumnScope::join(l_scope, r_scope);
                let cond = condition.as_ref().map(|e| self.translate_expr(e, &joined)).transpose()?;
                Ok((QedRelation::Join { left: Box::new(l_rel), right: Box::new(r_rel), condition: cond }, joined))
            }
            TableRef::Subquery { query, alias, .. } => {
                let sub_rel = self.translate_select(query)?;
                let sub_scope = self.scope_from_subquery(query, alias.as_deref())?;
                Ok((sub_rel, sub_scope))
            }
            _ => Err(TranslateError::UnsupportedExpr(format!("unsupported TableRef: {tr:?}"))),
        }
    }

    fn scope_for_table_ref(&self, tr: &TableRef) -> Result<ColumnScope, TranslateError> {
        match tr {
            TableRef::Table { name, alias, .. } =>
                ColumnScope::from_table(&name.join("."), alias.as_deref(), self.schema),
            TableRef::Join { left, right, .. } =>
                Ok(ColumnScope::join(self.scope_for_table_ref(left)?, self.scope_for_table_ref(right)?)),
            TableRef::Subquery { query, alias, .. } =>
                self.scope_from_subquery(query, alias.as_deref()),
            _ => Err(TranslateError::UnsupportedExpr(format!("unsupported TableRef for scope: {tr:?}"))),
        }
    }

    fn scope_from_subquery(&self, query: &SelectStatement, alias: Option<&str>) -> Result<ColumnScope, TranslateError> {
        let inner = self.build_scope_from(&query.from)?;
        let alias_key = alias.map(|a| a.to_lowercase());
        let mut columns = Vec::new();
        for target in &query.targets {
            match target {
                SelectTarget::Star(tbl_alias) => {
                    let fa = tbl_alias.as_ref().map(|a| a.to_lowercase());
                    for (tbl, col) in &inner.columns {
                        if fa.as_deref() == tbl.as_deref() || fa.is_none() {
                            columns.push((alias_key.clone(), col.clone()));
                        }
                    }
                }
                SelectTarget::Expr(expr, col_alias) => {
                    let name = col_alias.clone().unwrap_or_else(|| expr_column_name(expr)).to_lowercase();
                    columns.push((alias_key.clone(), name));
                }
            }
        }
        Ok(ColumnScope { columns })
    }

    fn translate_projection(&self, targets: &[SelectTarget], input: QedRelation, scope: &ColumnScope) -> Result<QedRelation, TranslateError> {
        let mut exprs = Vec::with_capacity(targets.len());
        for target in targets {
            match target {
                SelectTarget::Star(tbl_alias) => {
                    let fa = tbl_alias.as_ref().map(|a| a.to_lowercase());
                    for (i, (tbl, _)) in scope.columns.iter().enumerate() {
                        if fa.as_deref() == tbl.as_deref() || fa.is_none() {
                            exprs.push(QedExpr::ColumnRef { index: i });
                        }
                    }
                }
                SelectTarget::Expr(expr, _) => { exprs.push(self.translate_expr(expr, scope)?); }
            }
        }
        Ok(QedRelation::Project { exprs, input: Box::new(input) })
    }

    fn is_simple_star(&self, targets: &[SelectTarget], scope: &ColumnScope) -> bool {
        targets.len() == 1 && matches!(targets[0], SelectTarget::Star(None)) && scope.len() > 0
    }

    fn targets_have_aggregates(&self, targets: &[SelectTarget]) -> bool {
        targets.iter().any(|t| matches!(t, SelectTarget::Expr(e, _) if self.expr_has_aggregate(e)))
    }

    fn expr_has_aggregate(&self, expr: &Expr) -> bool {
        match expr {
            Expr::FunctionCall { name, args, .. } => {
                let f = name.last().map(|s| s.as_str()).unwrap_or("");
                is_aggregate(f) || args.iter().any(|a| self.expr_has_aggregate(a))
            }
            Expr::BinaryOp { left, right, .. } =>
                self.expr_has_aggregate(left) || self.expr_has_aggregate(right),
            Expr::UnaryOp { expr, .. } => self.expr_has_aggregate(expr),
            Expr::Parenthesized(inner) => self.expr_has_aggregate(inner),
            Expr::Case { operand, whens, else_expr } => {
                operand.as_ref().is_some_and(|e| self.expr_has_aggregate(e))
                    || whens.iter().any(|w| self.expr_has_aggregate(&w.condition) || self.expr_has_aggregate(&w.result))
                    || else_expr.as_ref().is_some_and(|e| self.expr_has_aggregate(e))
            }
            Expr::SpecialFunction { args, .. } => args.iter().any(|a| self.expr_has_aggregate(a)),
            _ => false,
        }
    }

    fn translate_group_by(&self, group_by: &[GroupByItem], targets: &[SelectTarget], having: &Option<Expr>, input: QedRelation, scope: &ColumnScope) -> Result<QedRelation, TranslateError> {
        let keys: Vec<usize> = group_by.iter().map(|item| match item {
            GroupByItem::Expr(e) => self.resolve_expr_index(e, scope),
            _ => Err(TranslateError::UnsupportedExpr(format!("unsupported GROUP BY: {item:?}"))),
        }).collect::<Result<_, _>>()?;
        let aggs = self.extract_aggregates(targets, scope)?;
        let agg_rel = QedRelation::Aggregate { keys, aggs, input: Box::new(input) };
        if let Some(ref h) = having {
            Ok(QedRelation::Filter { condition: self.translate_expr(h, scope)?, input: Box::new(agg_rel) })
        } else { Ok(agg_rel) }
    }

    fn resolve_expr_index(&self, expr: &Expr, scope: &ColumnScope) -> Result<usize, TranslateError> {
        match expr {
            Expr::ColumnRef(name) => { let (ta, cn) = split_column_ref(name); scope.resolve(ta, cn) }
            _ => Err(TranslateError::UnsupportedExpr(format!("GROUP BY non-column: {expr:?}"))),
        }
    }

    fn extract_aggregates(&self, targets: &[SelectTarget], scope: &ColumnScope) -> Result<Vec<QedAggCall>, TranslateError> {
        let mut aggs = Vec::new();
        for t in targets { if let SelectTarget::Expr(e, _) = t { self.collect_aggs(e, scope, &mut aggs)?; } }
        Ok(aggs)
    }

    fn collect_aggs(&self, expr: &Expr, scope: &ColumnScope, aggs: &mut Vec<QedAggCall>) -> Result<(), TranslateError> {
        match expr {
            Expr::FunctionCall { name, args, distinct, .. } => {
                let f = name.last().map(|s| s.as_str()).unwrap_or("");
                if is_aggregate(f) {
                    let arg = if is_star_arg(args) {
                        QedAggArg::Star
                    } else { QedAggArg::Expr(self.translate_expr(&args[0], scope)?) };
                    aggs.push(QedAggCall { func: f.to_lowercase(), arg, distinct: *distinct });
                    return Ok(());
                }
                for a in args { self.collect_aggs(a, scope, aggs)?; }
                Ok(())
            }
            Expr::BinaryOp { left, right, .. } => { self.collect_aggs(left, scope, aggs)?; self.collect_aggs(right, scope, aggs) }
            Expr::UnaryOp { expr, .. } => self.collect_aggs(expr, scope, aggs),
            Expr::Parenthesized(inner) => self.collect_aggs(inner, scope, aggs),
            _ => Ok(()),
        }
    }

    fn translate_expr(&self, expr: &Expr, scope: &ColumnScope) -> Result<QedExpr, TranslateError> {
        match expr {
            Expr::Literal(lit) => Ok(self.translate_literal(lit)),
            Expr::ColumnRef(name) => {
                if name.len() == 1 && name[0] == "*" {
                    return Ok(QedExpr::Literal { value: QedValue::String { value: "*".to_string() } });
                }
                let (ta, cn) = split_column_ref(name);
                Ok(QedExpr::ColumnRef { index: scope.resolve(ta, cn)? })
            }
            Expr::BinaryOp { left, op, right } => Ok(QedExpr::BinOp {
                op: map_binop(op),
                left: Box::new(self.translate_expr(left, scope)?),
                right: Box::new(self.translate_expr(right, scope)?),
            }),
            Expr::UnaryOp { op, expr } => {
                let qed_op = match op.to_uppercase().as_str() {
                    "NOT" => "not".to_string(), "-" => "neg".to_string(), _ => op.to_lowercase(),
                };
                Ok(QedExpr::UnOp { op: qed_op, expr: Box::new(self.translate_expr(expr, scope)?) })
            }
            Expr::FunctionCall { name, args, .. } => Ok(QedExpr::Function {
                name: name.last().map(|s| s.to_lowercase()).unwrap_or_default(),
                args: args.iter().map(|a| self.translate_expr(a, scope)).collect::<Result<Vec<_>, _>>()?,
            }),
            Expr::SpecialFunction { name, args } => Ok(QedExpr::Function {
                name: name.to_lowercase(),
                args: args.iter().map(|a| self.translate_expr(a, scope)).collect::<Result<Vec<_>, _>>()?,
            }),
            Expr::Parenthesized(inner) => self.translate_expr(inner, scope),
            Expr::IsNull { expr, negated } => {
                let inner = self.translate_expr(expr, scope)?;
                Ok(QedExpr::BinOp { op: if *negated { "neq" } else { "eq" }.to_string(), left: Box::new(inner), right: Box::new(QedExpr::Null) })
            }
            Expr::Between { expr, low, high, negated } => {
                let e = self.translate_expr(expr, scope)?;
                let lo = self.translate_expr(low, scope)?;
                let hi = self.translate_expr(high, scope)?;
                let between = QedExpr::BinOp { op: "and".to_string(),
                    left: Box::new(QedExpr::BinOp { op: "gte".to_string(), left: Box::new(e.clone()), right: Box::new(lo) }),
                    right: Box::new(QedExpr::BinOp { op: "lte".to_string(), left: Box::new(e), right: Box::new(hi) }),
                };
                if *negated { Ok(QedExpr::UnOp { op: "not".to_string(), expr: Box::new(between) }) } else { Ok(between) }
            }
            Expr::InList { expr, list, negated } => {
                let inner = self.translate_expr(expr, scope)?;
                let mut chain = list.iter().map(|item| {
                    Ok(QedExpr::BinOp { op: "eq".to_string(), left: Box::new(inner.clone()), right: Box::new(self.translate_expr(item, scope)?) })
                });
                let first = chain.next().transpose()?;
                let result = chain.fold(first, |acc, next| Some(QedExpr::BinOp { op: "or".to_string(), left: Box::new(acc?), right: Box::new(next.ok()?) }));
                let result = result.ok_or_else(|| TranslateError::UnsupportedExpr("empty IN list".to_string()))?;
                if *negated { Ok(QedExpr::UnOp { op: "not".to_string(), expr: Box::new(result) }) } else { Ok(result) }
            }
            Expr::Exists(subquery) => Ok(QedExpr::Quantified { cmp: "eq".to_string(), quantifier: "some".to_string(), subquery: Box::new(self.translate_select(subquery)?) }),
            Expr::Subquery(subquery) => { let _ = self.translate_select(subquery)?; Ok(QedExpr::Function { name: "ScalarSubquery".to_string(), args: vec![] }) }
            Expr::Parameter(n) => Ok(QedExpr::Function { name: "Param".to_string(), args: vec![QedExpr::Literal { value: QedValue::Integer { value: i64::from(*n) } }] }),
            Expr::MyBatisParam(name) => Ok(QedExpr::Function { name: "Param".to_string(), args: vec![QedExpr::Literal { value: QedValue::String { value: name.clone() } }] }),
            Expr::Case { operand, whens, else_expr } => {
                let mut args = Vec::new();
                if let Some(ref op) = operand { args.push(self.translate_expr(op, scope)?); }
                for w in whens { args.push(self.translate_expr(&w.condition, scope)?); args.push(self.translate_expr(&w.result, scope)?); }
                if let Some(ref el) = else_expr { args.push(self.translate_expr(el, scope)?); }
                Ok(QedExpr::Function { name: "CASE".to_string(), args })
            }
            Expr::TypeCast { expr, .. } => self.translate_expr(expr, scope),
            Expr::Like { expr, pattern, negated, .. } => {
                let like = QedExpr::BinOp { op: "like".to_string(), left: Box::new(self.translate_expr(expr, scope)?), right: Box::new(self.translate_expr(pattern, scope)?) };
                if *negated { Ok(QedExpr::UnOp { op: "not".to_string(), expr: Box::new(like) }) } else { Ok(like) }
            }
            _ => Err(TranslateError::UnsupportedExpr(format!("{expr:?}"))),
        }
    }

    fn translate_literal(&self, lit: &Literal) -> QedExpr {
        match lit {
            Literal::Null => QedExpr::Null,
            _ => QedExpr::Literal { value: match lit {
                Literal::Integer(n) => QedValue::Integer { value: *n },
                Literal::Float(s) => QedValue::Float { value: s.parse().unwrap_or(0.0) },
                Literal::String(s) => QedValue::String { value: s.clone() },
                Literal::Boolean(b) => QedValue::Boolean { value: *b },
                other => QedValue::String { value: format!("{other:?}") },
            }},
        }
    }
}

#[cfg(test)]
mod tests;

