//! ogsql-parser AST → QED [`QedRelation`] tree translator.
//!
//! Translates a parsed [`Statement::Select`] into the QED intermediate
//! representation suitable for equivalence verification. Non-SELECT
//! statements return [`TranslateError::UnsupportedStatement`].

use crate::ir::{QedAggArg, QedAggCall, QedExpr, QedRelation, QedValue};
use crate::schema::RichSchema;
use ogsql_parser::ast::{
    Expr, GroupByItem, Ident, Literal, SelectStatement, SelectTarget, SetOperation, Statement,
    TableRef,
};

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
        table_name: &str,
        alias: Option<&str>,
        schema: &RichSchema,
    ) -> Result<Self, TranslateError> {
        let lower = table_name.to_lowercase();
        let info = schema
            .tables
            .get(&lower)
            .ok_or_else(|| TranslateError::TableNotFound(table_name.to_string()))?;
        let alias_key = alias.map(|a| a.to_lowercase());
        let columns = info
            .columns
            .iter()
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
        let mut matches: Vec<usize> = self
            .columns
            .iter()
            .enumerate()
            .filter(|(_, (tbl, col))| col == &lower && alias_lower.as_deref() == tbl.as_deref())
            .map(|(i, _)| i)
            .collect();
        if matches.is_empty() && table_alias.is_none() {
            matches = self
                .columns
                .iter()
                .enumerate()
                .filter(|(_, (_, col))| col == &lower)
                .map(|(i, _)| i)
                .collect();
        }
        match matches.len() {
            0 => Err(TranslateError::ColumnNotFound(
                table_alias.map_or(lower.clone(), |a| format!("{a}.{lower}")),
            )),
            1 => Ok(matches[0]),
            _ => Err(TranslateError::AmbiguousColumn(lower)),
        }
    }

    fn len(&self) -> usize {
        self.columns.len()
    }

    fn try_resolve(&self, table_alias: Option<&str>, col_name: &str) -> Option<usize> {
        let lower = col_name.to_lowercase();
        let alias_lower = table_alias.map(|a| a.to_lowercase());
        let matches: Vec<usize> = self
            .columns
            .iter()
            .enumerate()
            .filter(|(_, (tbl, col))| col == &lower && alias_lower.as_deref() == tbl.as_deref())
            .map(|(i, _)| i)
            .collect();
        if matches.len() == 1 {
            Some(matches[0])
        } else if table_alias.is_none() {
            // Fall back to unqualified resolution (same as resolve())
            let unqualified: Vec<usize> = self
                .columns
                .iter()
                .enumerate()
                .filter(|(_, (_, col))| col == &lower)
                .map(|(i, _)| i)
                .collect();
            if unqualified.len() == 1 {
                Some(unqualified[0])
            } else {
                None
            }
        } else {
            None
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

const AGG_FUNCS: &[&str] = &[
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "group_concat",
    "string_agg",
    "array_agg",
];

fn is_aggregate(name: &str) -> bool {
    AGG_FUNCS.contains(&name.to_lowercase().as_str())
}

fn is_star_arg(args: &[Expr]) -> bool {
    if args.is_empty() {
        return true;
    }
    if args.len() != 1 {
        return false;
    }
    matches!(
        &args[0],
        Expr::Literal(Literal::Integer(1)) | Expr::QualifiedStar(_)
    ) || matches!(&args[0], Expr::ColumnRef(n) if n.len() == 1 && n[0] == "*")
}

fn expr_column_name(expr: &Expr) -> String {
    match expr {
        Expr::ColumnRef(name) => name
            .last()
            .map(|i| i.as_str().to_string())
            .unwrap_or_else(|| "?column?".to_string()),
        _ => "?column?".to_string(),
    }
}

fn map_binop(op: &str) -> String {
    match op.to_uppercase().as_str() {
        "=" => "eq",
        ">" => "gt",
        "<" => "lt",
        ">=" => "gte",
        "<=" => "lte",
        "<>" | "!=" => "neq",
        "AND" => "and",
        "OR" => "or",
        "+" => "add",
        "-" => "sub",
        "*" => "mul",
        "/" => "div",
        "%" => "mod",
        "||" => "concat",
        "IS" => "eq",
        "IS NOT" => "neq",
        "LIKE" => "like",
        "ILIKE" => "ilike",
        "~" => "regex_match",
        "~*" => "regex_imatch",
        "!~" => "regex_not_match",
        "!~*" => "regex_not_imatch",
        _ => return op.to_lowercase(),
    }
    .to_string()
}

fn split_column_ref(name: &[Ident]) -> (Option<&str>, &str) {
    match name.len() {
        1 => (None, name[0].as_str()),
        2 => (Some(name[0].as_str()), name[1].as_str()),
        _ => (
            Some(name[name.len() - 2].as_str()),
            name[name.len() - 1].as_str(),
        ),
    }
}

fn set_op_right(op: &SetOperation) -> &SelectStatement {
    match op {
        SetOperation::Union { right, .. }
        | SetOperation::Intersect { right, .. }
        | SetOperation::Except { right, .. } => right,
    }
}

// ── Decorrelation helpers ────────────────────────────────────────────────

/// Result of extracting correlation predicates from a subquery WHERE clause.
struct CorrelationResult {
    pairs: Vec<(usize, usize)>,
    residual: Option<Expr>,
}

/// Check whether a subquery is safe for decorrelation:
/// single-table FROM, no JOIN, no GROUP BY, no HAVING, no set operations.
fn is_safe_subquery(subquery: &SelectStatement) -> bool {
    if subquery.from.len() != 1 {
        return false;
    }
    if matches!(subquery.from[0], TableRef::Join { .. }) {
        return false;
    }
    if !subquery.group_by.is_empty() || subquery.having.is_some() {
        return false;
    }
    if subquery.set_operation.is_some() {
        return false;
    }
    true
}

fn subquery_first_col_index(subquery: &SelectStatement, scope: &ColumnScope) -> Option<usize> {
    let first = subquery.targets.first()?;
    match first {
        SelectTarget::Expr(Expr::ColumnRef(name), _) => {
            let (tbl, col) = split_column_ref(name);
            scope.try_resolve(tbl, col)
        }
        _ => None,
    }
}

/// Try to extract a correlation pair `(outer_col_index, inner_col_index)` from
/// a binary `=` expression whose both sides are column references.
fn try_extract_correlation(
    left: &Expr,
    right: &Expr,
    outer: &ColumnScope,
    inner: &ColumnScope,
) -> Option<(usize, usize)> {
    match (left, right) {
        (Expr::ColumnRef(n1), Expr::ColumnRef(n2)) => {
            let (t1, c1) = split_column_ref(n1);
            let (t2, c2) = split_column_ref(n2);
            if let (Some(oi), Some(ii)) = (outer.try_resolve(t1, c1), inner.try_resolve(t2, c2)) {
                Some((oi, ii))
            } else if let (Some(oi), Some(ii)) =
                (outer.try_resolve(t2, c2), inner.try_resolve(t1, c1))
            {
                Some((oi, ii))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Walk the subquery WHERE tree and separate correlation predicates (equality
/// between one outer-scope column and one inner-scope column) from residual
/// predicates that apply only to the inner table.
fn collect_correlation_preds(
    where_clause: &Expr,
    outer_scope: &ColumnScope,
    inner_scope: &ColumnScope,
) -> Result<CorrelationResult, TranslateError> {
    match where_clause {
        Expr::BinaryOp { op, left, right } if op.to_uppercase() == "AND" => {
            let mut acc = collect_correlation_preds(left, outer_scope, inner_scope)?;
            let right_res = collect_correlation_preds(right, outer_scope, inner_scope)?;
            acc.pairs.extend(right_res.pairs);
            acc.residual = match (acc.residual.take(), right_res.residual) {
                (Some(e1), Some(e2)) => Some(Expr::BinaryOp {
                    left: Box::new(e1),
                    op: "AND".to_string(),
                    right: Box::new(e2),
                }),
                (e @ Some(_), None) | (None, e @ Some(_)) => e,
                (None, None) => None,
            };
            Ok(acc)
        }
        Expr::BinaryOp { op, left, right } if op == "=" => {
            if let Some(pair) = try_extract_correlation(left, right, outer_scope, inner_scope) {
                Ok(CorrelationResult {
                    pairs: vec![pair],
                    residual: None,
                })
            } else {
                Ok(CorrelationResult {
                    pairs: vec![],
                    residual: Some(where_clause.clone()),
                })
            }
        }
        _ => Ok(CorrelationResult {
            pairs: vec![],
            residual: Some(where_clause.clone()),
        }),
    }
}

// ── Translator ───────────────────────────────────────────────────────────

/// Translates ogsql-parser AST statements into QED relation trees.
pub struct AstTranslator<'a> {
    schema: &'a RichSchema,
}

impl<'a> AstTranslator<'a> {
    /// Create a new translator with the given schema for name resolution.
    pub fn new(schema: &'a RichSchema) -> Self {
        Self { schema }
    }

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
            rel = self.translate_where(wc, &scope, rel)?;
        }

        if !select.group_by.is_empty() || has_agg {
            rel = self.translate_group_by(
                &select.group_by,
                &select.targets,
                &select.having,
                rel,
                &scope,
            )?;
        }

        if !self.is_simple_star(&select.targets, &scope) {
            rel = self.translate_projection(&select.targets, rel, &scope)?;
        }

        if select.distinct {
            rel = QedRelation::Distinct {
                input: Box::new(rel),
            };
        }

        if !select.order_by.is_empty() {
            let args = select
                .order_by
                .iter()
                .map(|item| {
                    let expr = self.translate_expr(&item.expr, &scope)?;
                    let dir = QedExpr::Literal {
                        value: QedValue::String {
                            value: if item.asc == Some(false) {
                                "desc"
                            } else {
                                "asc"
                            }
                            .to_string(),
                        },
                    };
                    Ok(QedExpr::Function {
                        name: "SortKey".to_string(),
                        args: vec![expr, dir],
                    })
                })
                .collect::<Result<Vec<_>, TranslateError>>()?;
            rel = QedRelation::QOp {
                name: "Sort".to_string(),
                args,
                input: Box::new(rel),
            };
        }

        if let Some(ref e) = select.limit {
            rel = QedRelation::QOp {
                name: "Limit".to_string(),
                args: vec![self.translate_expr(e, &scope)?],
                input: Box::new(rel),
            };
        }
        if let Some(ref e) = select.offset {
            rel = QedRelation::QOp {
                name: "Offset".to_string(),
                args: vec![self.translate_expr(e, &scope)?],
                input: Box::new(rel),
            };
        }
        if let Some(ref fetch) = select.fetch {
            if let Some(ref e) = fetch.count {
                rel = QedRelation::QOp {
                    name: "Limit".to_string(),
                    args: vec![self.translate_expr(e, &scope)?],
                    input: Box::new(rel),
                };
            }
        }

        if let Some(ref set_op) = select.set_operation {
            let right = self.translate_select(set_op_right(set_op))?;
            rel = match set_op {
                SetOperation::Union { all: true, .. } => QedRelation::Union {
                    left: Box::new(rel),
                    right: Box::new(right),
                },
                SetOperation::Union { all: false, .. } => QedRelation::Distinct {
                    input: Box::new(QedRelation::Union {
                        left: Box::new(rel),
                        right: Box::new(right),
                    }),
                },
                SetOperation::Intersect { all: true, .. } => QedRelation::Intersect {
                    left: Box::new(rel),
                    right: Box::new(right),
                },
                SetOperation::Intersect { all: false, .. } => QedRelation::Distinct {
                    input: Box::new(QedRelation::Intersect {
                        left: Box::new(rel),
                        right: Box::new(right),
                    }),
                },
                SetOperation::Except { all: true, .. } => QedRelation::Except {
                    left: Box::new(rel),
                    right: Box::new(right),
                },
                SetOperation::Except { all: false, .. } => QedRelation::Distinct {
                    input: Box::new(QedRelation::Except {
                        left: Box::new(rel),
                        right: Box::new(right),
                    }),
                },
            };
        }
        Ok(rel)
    }

    /// Translate a WHERE clause, attempting to decorrelate EXISTS / IN (non-negated)
    /// subqueries into `Distinct(Join(...))`. Falls back to a plain `Filter` when
    /// decorrelation is not applicable.
    fn translate_where(
        &self,
        where_clause: &Expr,
        outer_scope: &ColumnScope,
        outer_rel: QedRelation,
    ) -> Result<QedRelation, TranslateError> {
        match where_clause {
            Expr::Exists(subquery) => {
                match self.try_decorrelate_exists(subquery, outer_scope, outer_rel.clone()) {
                    Ok(decorrelated) => Ok(decorrelated),
                    Err(_) => Ok(QedRelation::Filter {
                        condition: self.translate_expr(where_clause, outer_scope)?,
                        input: Box::new(outer_rel),
                    }),
                }
            }
            Expr::InSubquery {
                expr,
                subquery,
                negated: false,
            } => match self.try_decorrelate_in(expr, subquery, outer_scope, outer_rel.clone()) {
                Ok(decorrelated) => Ok(decorrelated),
                Err(_) => Ok(QedRelation::Filter {
                    condition: self.translate_expr(where_clause, outer_scope)?,
                    input: Box::new(outer_rel),
                }),
            },
            Expr::BinaryOp { op, left, right } if op.to_uppercase() == "AND" => {
                let after_left = self.translate_where(left, outer_scope, outer_rel)?;
                self.translate_where(right, outer_scope, after_left)
            }
            _ => Ok(QedRelation::Filter {
                condition: self.translate_expr(where_clause, outer_scope)?,
                input: Box::new(outer_rel),
            }),
        }
    }

    /// Attempt to decorrelate an `EXISTS (subquery)` as `Distinct(Join(INNER))`.
    ///
    /// Returns `Err` when decorrelation is not applicable (unsafe subquery,
    /// no correlation predicates found, etc.) so the caller can fall back to
    /// `QedExpr::Quantified`.
    fn try_decorrelate_exists(
        &self,
        subquery: &SelectStatement,
        outer_scope: &ColumnScope,
        outer_rel: QedRelation,
    ) -> Result<QedRelation, TranslateError> {
        if !is_safe_subquery(subquery) {
            return Err(TranslateError::UnsupportedExpr(
                "subquery too complex for decorrelation".into(),
            ));
        }
        let inner_scope = self.build_scope_from(&subquery.from)?;
        let inner_rel = self.translate_from(&subquery.from)?;

        let outer_arity = outer_scope.len();

        // Handle no WHERE clause: uncorrelated EXISTS
        // EXISTS(SELECT 1 FROM t) is true if t has any rows → cross join + DISTINCT
        let (inner, conditions) = match subquery.where_clause.as_ref() {
            None => {
                return Ok(QedRelation::Distinct {
                    input: Box::new(QedRelation::Join {
                        left: Box::new(outer_rel),
                        right: Box::new(inner_rel),
                        condition: None,
                    }),
                });
            }
            Some(wc) => {
                let corr = collect_correlation_preds(wc, outer_scope, &inner_scope).unwrap_or(
                    CorrelationResult {
                        pairs: Vec::new(),
                        residual: None,
                    },
                );

                let conditions: Vec<QedExpr> = corr
                    .pairs
                    .iter()
                    .map(|(oi, ii)| QedExpr::BinOp {
                        op: "eq".to_string(),
                        left: Box::new(QedExpr::ColumnRef { index: *oi }),
                        right: Box::new(QedExpr::ColumnRef {
                            index: outer_arity + *ii,
                        }),
                    })
                    .collect();

                let inner = if let Some(residual) = corr.residual {
                    let residual_expr = self.translate_expr(&residual, &inner_scope)?;
                    QedRelation::Filter {
                        condition: residual_expr,
                        input: Box::new(inner_rel),
                    }
                } else {
                    inner_rel
                };

                (inner, conditions)
            }
        };

        let join_condition = if conditions.is_empty() {
            None
        } else if conditions.len() == 1 {
            Some(conditions.into_iter().next().expect("non-empty"))
        } else {
            Some(
                conditions
                    .into_iter()
                    .reduce(|a, b| QedExpr::BinOp {
                        op: "and".to_string(),
                        left: Box::new(a),
                        right: Box::new(b),
                    })
                    .expect("non-empty"),
            )
        };

        let join = QedRelation::Join {
            left: Box::new(outer_rel),
            right: Box::new(inner),
            condition: join_condition,
        };
        Ok(QedRelation::Distinct {
            input: Box::new(join),
        })
    }

    /// Attempt to decorrelate an `IN (subquery)` (non-negated) as `Distinct(Join(INNER))`.
    ///
    /// Returns `Err` when decorrelation is not applicable so the caller can fall back
    /// to `QedExpr::Quantified`.
    fn try_decorrelate_in(
        &self,
        outer_expr: &Expr,
        subquery: &SelectStatement,
        outer_scope: &ColumnScope,
        outer_rel: QedRelation,
    ) -> Result<QedRelation, TranslateError> {
        if !is_safe_subquery(subquery) {
            return Err(TranslateError::UnsupportedExpr(
                "subquery too complex for decorrelation".into(),
            ));
        }
        let inner_scope = self.build_scope_from(&subquery.from)?;
        let inner_rel = self.translate_from(&subquery.from)?;

        let outer_arity = outer_scope.len();

        // Resolve the subquery's first target column in the inner scope.
        let inner_first_col =
            subquery_first_col_index(subquery, &inner_scope).ok_or_else(|| {
                TranslateError::UnsupportedExpr(
                    "IN subquery first target is not a simple column".into(),
                )
            })?;

        // Translate the outer expression within the outer scope.
        let outer_qed = self.translate_expr(outer_expr, outer_scope)?;

        // Build conditions with correlation + IN pair.
        let (inner, conditions) = match subquery.where_clause.as_ref() {
            None => {
                // No WHERE clause: IN subquery without correlation
                // The join condition is just outer_expr = subquery_first_col
                let conditions = vec![QedExpr::BinOp {
                    op: "eq".to_string(),
                    left: Box::new(outer_qed),
                    right: Box::new(QedExpr::ColumnRef {
                        index: outer_arity + inner_first_col,
                    }),
                }];
                let join = QedRelation::Join {
                    left: Box::new(outer_rel),
                    right: Box::new(inner_rel),
                    condition: Some(conditions.into_iter().next().expect("non-empty")),
                };
                return Ok(QedRelation::Distinct {
                    input: Box::new(join),
                });
            }
            Some(wc) => {
                let corr = collect_correlation_preds(wc, outer_scope, &inner_scope).unwrap_or(
                    CorrelationResult {
                        pairs: Vec::new(),
                        residual: None,
                    },
                );

                let mut conditions: Vec<QedExpr> = corr
                    .pairs
                    .iter()
                    .map(|(oi, ii)| QedExpr::BinOp {
                        op: "eq".to_string(),
                        left: Box::new(QedExpr::ColumnRef { index: *oi }),
                        right: Box::new(QedExpr::ColumnRef {
                            index: outer_arity + *ii,
                        }),
                    })
                    .collect();

                // Add the IN expression pairing: outer_expr = subquery_first_col
                conditions.push(QedExpr::BinOp {
                    op: "eq".to_string(),
                    left: Box::new(outer_qed),
                    right: Box::new(QedExpr::ColumnRef {
                        index: outer_arity + inner_first_col,
                    }),
                });

                let inner = if let Some(residual) = corr.residual {
                    let residual_expr = self.translate_expr(&residual, &inner_scope)?;
                    QedRelation::Filter {
                        condition: residual_expr,
                        input: Box::new(inner_rel),
                    }
                } else {
                    inner_rel
                };

                (inner, conditions)
            }
        };

        let join_condition = if conditions.len() == 1 {
            Some(conditions.into_iter().next().expect("non-empty"))
        } else {
            Some(
                conditions
                    .into_iter()
                    .reduce(|a, b| QedExpr::BinOp {
                        op: "and".to_string(),
                        left: Box::new(a),
                        right: Box::new(b),
                    })
                    .expect("non-empty"),
            )
        };

        let join = QedRelation::Join {
            left: Box::new(outer_rel),
            right: Box::new(inner),
            condition: join_condition,
        };
        Ok(QedRelation::Distinct {
            input: Box::new(join),
        })
    }

    fn translate_from(&self, from: &[TableRef]) -> Result<QedRelation, TranslateError> {
        let (mut rel, _) = self.translate_table_ref(&from[0])?;
        for tr in &from[1..] {
            let (right_rel, _) = self.translate_table_ref(tr)?;
            rel = QedRelation::Join {
                left: Box::new(rel),
                right: Box::new(right_rel),
                condition: None,
            };
        }
        Ok(rel)
    }

    fn build_scope_from(&self, from: &[TableRef]) -> Result<ColumnScope, TranslateError> {
        if from.is_empty() {
            return Ok(ColumnScope { columns: vec![] });
        }
        let mut scope = self.scope_for_table_ref(&from[0])?;
        for tr in &from[1..] {
            scope = ColumnScope::join(scope, self.scope_for_table_ref(tr)?);
        }
        Ok(scope)
    }

    fn translate_table_ref(
        &self,
        tr: &TableRef,
    ) -> Result<(QedRelation, ColumnScope), TranslateError> {
        match tr {
            TableRef::Table { name, alias, .. } => {
                let table_name = name.join(".");
                let implicit_alias = alias.as_deref().or_else(|| name.last().map(|s| s.as_str()));
                let scope = ColumnScope::from_table(&table_name, implicit_alias, self.schema)?;
                let rel = QedRelation::Scan {
                    table: table_name.to_lowercase(),
                    fields: vec![],
                };
                Ok((rel, scope))
            }
            TableRef::Join {
                left,
                right,
                condition,
                ..
            } => {
                let (l_rel, l_scope) = self.translate_table_ref(left)?;
                let (r_rel, r_scope) = self.translate_table_ref(right)?;
                let joined = ColumnScope::join(l_scope, r_scope);
                let cond = condition
                    .as_ref()
                    .map(|e| self.translate_expr(e, &joined))
                    .transpose()?;
                Ok((
                    QedRelation::Join {
                        left: Box::new(l_rel),
                        right: Box::new(r_rel),
                        condition: cond,
                    },
                    joined,
                ))
            }
            TableRef::Subquery { query, alias, .. } => {
                let sub_rel = self.translate_select(query)?;
                let sub_scope = self.scope_from_subquery(query, alias.as_deref())?;
                Ok((sub_rel, sub_scope))
            }
            _ => Err(TranslateError::UnsupportedExpr(format!(
                "unsupported TableRef: {tr:?}"
            ))),
        }
    }

    fn scope_for_table_ref(&self, tr: &TableRef) -> Result<ColumnScope, TranslateError> {
        match tr {
            TableRef::Table { name, alias, .. } => {
                let implicit_alias = alias.as_deref().or_else(|| name.last().map(|s| s.as_str()));
                ColumnScope::from_table(&name.join("."), implicit_alias, self.schema)
            }
            TableRef::Join { left, right, .. } => Ok(ColumnScope::join(
                self.scope_for_table_ref(left)?,
                self.scope_for_table_ref(right)?,
            )),
            TableRef::Subquery { query, alias, .. } => {
                self.scope_from_subquery(query, alias.as_deref())
            }
            _ => Err(TranslateError::UnsupportedExpr(format!(
                "unsupported TableRef for scope: {tr:?}"
            ))),
        }
    }

    fn scope_from_subquery(
        &self,
        query: &SelectStatement,
        alias: Option<&str>,
    ) -> Result<ColumnScope, TranslateError> {
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
                    let name = col_alias
                        .as_ref()
                        .map(|a| a.as_str().to_string())
                        .unwrap_or_else(|| expr_column_name(expr))
                        .to_lowercase();
                    columns.push((alias_key.clone(), name));
                }
            }
        }
        Ok(ColumnScope { columns })
    }

    fn translate_projection(
        &self,
        targets: &[SelectTarget],
        input: QedRelation,
        scope: &ColumnScope,
    ) -> Result<QedRelation, TranslateError> {
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
                SelectTarget::Expr(expr, _) => {
                    exprs.push(self.translate_expr(expr, scope)?);
                }
            }
        }
        Ok(QedRelation::Project {
            exprs,
            input: Box::new(input),
        })
    }

    fn is_simple_star(&self, targets: &[SelectTarget], scope: &ColumnScope) -> bool {
        targets.len() == 1 && matches!(targets[0], SelectTarget::Star(None)) && scope.len() > 0
    }

    fn targets_have_aggregates(&self, targets: &[SelectTarget]) -> bool {
        targets
            .iter()
            .any(|t| matches!(t, SelectTarget::Expr(e, _) if self.expr_has_aggregate(e)))
    }

    fn expr_has_aggregate(&self, expr: &Expr) -> bool {
        match expr {
            Expr::FunctionCall { name, args, .. } => {
                let f = name.last().map(|s| s.as_str()).unwrap_or("");
                is_aggregate(f) || args.iter().any(|a| self.expr_has_aggregate(a))
            }
            Expr::BinaryOp { left, right, .. } => {
                self.expr_has_aggregate(left) || self.expr_has_aggregate(right)
            }
            Expr::UnaryOp { expr, .. } => self.expr_has_aggregate(expr),
            Expr::Parenthesized(inner) => self.expr_has_aggregate(inner),
            Expr::Case {
                operand,
                whens,
                else_expr,
            } => {
                operand.as_ref().is_some_and(|e| self.expr_has_aggregate(e))
                    || whens.iter().any(|w| {
                        self.expr_has_aggregate(&w.condition) || self.expr_has_aggregate(&w.result)
                    })
                    || else_expr
                        .as_ref()
                        .is_some_and(|e| self.expr_has_aggregate(e))
            }
            Expr::SpecialFunction { args, .. } => args.iter().any(|a| self.expr_has_aggregate(a)),
            _ => false,
        }
    }

    fn translate_group_by(
        &self,
        group_by: &[GroupByItem],
        targets: &[SelectTarget],
        having: &Option<Expr>,
        input: QedRelation,
        scope: &ColumnScope,
    ) -> Result<QedRelation, TranslateError> {
        let keys: Vec<usize> = group_by
            .iter()
            .map(|item| match item {
                GroupByItem::Expr(e) => self.resolve_expr_index(e, scope),
                _ => Err(TranslateError::UnsupportedExpr(format!(
                    "unsupported GROUP BY: {item:?}"
                ))),
            })
            .collect::<Result<_, _>>()?;
        let aggs = self.extract_aggregates(targets, scope)?;
        let agg_rel = QedRelation::Aggregate {
            keys,
            aggs,
            input: Box::new(input),
        };
        if let Some(ref h) = having {
            // Phase A limitation: HAVING expressions use pre-aggregation scope.
            // GROUP BY key references work (same index), but aggregate function
            // references in HAVING (e.g., HAVING COUNT(*) > 5) will error with
            // ColumnNotFound. Full post-aggregation scope support requires Phase B.
            Ok(QedRelation::Filter {
                condition: self.translate_expr(h, scope)?,
                input: Box::new(agg_rel),
            })
        } else {
            Ok(agg_rel)
        }
    }

    fn resolve_expr_index(
        &self,
        expr: &Expr,
        scope: &ColumnScope,
    ) -> Result<usize, TranslateError> {
        match expr {
            Expr::ColumnRef(name) => {
                let (ta, cn) = split_column_ref(name);
                scope.resolve(ta, cn)
            }
            _ => Err(TranslateError::UnsupportedExpr(format!(
                "GROUP BY non-column: {expr:?}"
            ))),
        }
    }

    fn extract_aggregates(
        &self,
        targets: &[SelectTarget],
        scope: &ColumnScope,
    ) -> Result<Vec<QedAggCall>, TranslateError> {
        let mut aggs = Vec::new();
        for t in targets {
            if let SelectTarget::Expr(e, _) = t {
                self.collect_aggs(e, scope, &mut aggs)?;
            }
        }
        Ok(aggs)
    }

    fn collect_aggs(
        &self,
        expr: &Expr,
        scope: &ColumnScope,
        aggs: &mut Vec<QedAggCall>,
    ) -> Result<(), TranslateError> {
        match expr {
            Expr::FunctionCall {
                name,
                args,
                distinct,
                ..
            } => {
                let f = name.last().map(|s| s.as_str()).unwrap_or("");
                if is_aggregate(f) {
                    let arg = if is_star_arg(args) {
                        QedAggArg::Star
                    } else {
                        QedAggArg::Expr(self.translate_expr(&args[0], scope)?)
                    };
                    aggs.push(QedAggCall {
                        func: f.to_lowercase(),
                        arg,
                        distinct: *distinct,
                    });
                    return Ok(());
                }
                for a in args {
                    self.collect_aggs(a, scope, aggs)?;
                }
                Ok(())
            }
            Expr::BinaryOp { left, right, .. } => {
                self.collect_aggs(left, scope, aggs)?;
                self.collect_aggs(right, scope, aggs)
            }
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
                    return Ok(QedExpr::Literal {
                        value: QedValue::String {
                            value: "*".to_string(),
                        },
                    });
                }
                let (ta, cn) = split_column_ref(name);
                Ok(QedExpr::ColumnRef {
                    index: scope.resolve(ta, cn)?,
                })
            }
            Expr::BinaryOp { left, op, right } => Ok(QedExpr::BinOp {
                op: map_binop(op),
                left: Box::new(self.translate_expr(left, scope)?),
                right: Box::new(self.translate_expr(right, scope)?),
            }),
            Expr::UnaryOp { op, expr } => {
                let qed_op = match op.to_uppercase().as_str() {
                    "NOT" => "not".to_string(),
                    "-" => "neg".to_string(),
                    _ => op.to_lowercase(),
                };
                Ok(QedExpr::UnOp {
                    op: qed_op,
                    expr: Box::new(self.translate_expr(expr, scope)?),
                })
            }
            Expr::FunctionCall { name, args, .. } => Ok(QedExpr::Function {
                name: name.last().map(|s| s.to_lowercase()).unwrap_or_default(),
                args: args
                    .iter()
                    .map(|a| self.translate_expr(a, scope))
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            Expr::SpecialFunction { name, args } => Ok(QedExpr::Function {
                name: name.to_lowercase(),
                args: args
                    .iter()
                    .map(|a| self.translate_expr(a, scope))
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            Expr::Parenthesized(inner) => self.translate_expr(inner, scope),
            Expr::IsNull { expr, negated } => {
                let inner = self.translate_expr(expr, scope)?;
                Ok(QedExpr::BinOp {
                    op: if *negated { "neq" } else { "eq" }.to_string(),
                    left: Box::new(inner),
                    right: Box::new(QedExpr::Null),
                })
            }
            Expr::Between {
                expr,
                low,
                high,
                negated,
            } => {
                let e = self.translate_expr(expr, scope)?;
                let lo = self.translate_expr(low, scope)?;
                let hi = self.translate_expr(high, scope)?;
                let between = QedExpr::BinOp {
                    op: "and".to_string(),
                    left: Box::new(QedExpr::BinOp {
                        op: "gte".to_string(),
                        left: Box::new(e.clone()),
                        right: Box::new(lo),
                    }),
                    right: Box::new(QedExpr::BinOp {
                        op: "lte".to_string(),
                        left: Box::new(e),
                        right: Box::new(hi),
                    }),
                };
                if *negated {
                    Ok(QedExpr::UnOp {
                        op: "not".to_string(),
                        expr: Box::new(between),
                    })
                } else {
                    Ok(between)
                }
            }
            Expr::InList {
                expr,
                list,
                negated,
            } => {
                let inner = self.translate_expr(expr, scope)?;
                let items: Vec<QedExpr> = list
                    .iter()
                    .map(|item| {
                        Ok(QedExpr::BinOp {
                            op: "eq".to_string(),
                            left: Box::new(inner.clone()),
                            right: Box::new(self.translate_expr(item, scope)?),
                        })
                    })
                    .collect::<Result<Vec<_>, TranslateError>>()?;
                if items.is_empty() {
                    return Err(TranslateError::UnsupportedExpr("empty IN list".to_string()));
                }
                let result = items
                    .into_iter()
                    .reduce(|acc, next| QedExpr::BinOp {
                        op: "or".to_string(),
                        left: Box::new(acc),
                        right: Box::new(next),
                    })
                    .expect("non-empty items reduce to one");
                if *negated {
                    Ok(QedExpr::UnOp {
                        op: "not".to_string(),
                        expr: Box::new(result),
                    })
                } else {
                    Ok(result)
                }
            }
            Expr::Exists(subquery) => Ok(QedExpr::Quantified {
                cmp: "eq".to_string(),
                quantifier: "some".to_string(),
                subquery: Box::new(self.translate_select(subquery)?),
            }),
            Expr::InSubquery {
                expr: _,
                subquery,
                negated,
            } => Ok(QedExpr::Quantified {
                cmp: "eq".to_string(),
                quantifier: {
                    if *negated {
                        "none".to_string()
                    } else {
                        "some".to_string()
                    }
                },
                subquery: Box::new(self.translate_select(subquery)?),
            }),
            Expr::Subquery(subquery) => {
                let _ = self.translate_select(subquery)?;
                Ok(QedExpr::Function {
                    name: "ScalarSubquery".to_string(),
                    args: vec![QedExpr::Literal {
                        value: QedValue::String {
                            value: "subquery".to_string(),
                        },
                    }],
                })
            }
            Expr::Parameter(n) => Ok(QedExpr::Function {
                name: "Param".to_string(),
                args: vec![QedExpr::Literal {
                    value: QedValue::Integer {
                        value: i64::from(*n),
                    },
                }],
            }),
            Expr::MyBatisParam(name) => Ok(QedExpr::Function {
                name: "Param".to_string(),
                args: vec![QedExpr::Literal {
                    value: QedValue::String {
                        value: name.clone(),
                    },
                }],
            }),
            Expr::Case {
                operand,
                whens,
                else_expr,
            } => {
                let mut args = Vec::new();
                if let Some(ref op) = operand {
                    args.push(self.translate_expr(op, scope)?);
                }
                for w in whens {
                    args.push(self.translate_expr(&w.condition, scope)?);
                    args.push(self.translate_expr(&w.result, scope)?);
                }
                if let Some(ref el) = else_expr {
                    args.push(self.translate_expr(el, scope)?);
                }
                Ok(QedExpr::Function {
                    name: "CASE".to_string(),
                    args,
                })
            }
            Expr::TypeCast { expr, .. } => self.translate_expr(expr, scope),
            Expr::Like {
                expr,
                pattern,
                negated,
                ..
            } => {
                let like = QedExpr::BinOp {
                    op: "like".to_string(),
                    left: Box::new(self.translate_expr(expr, scope)?),
                    right: Box::new(self.translate_expr(pattern, scope)?),
                };
                if *negated {
                    Ok(QedExpr::UnOp {
                        op: "not".to_string(),
                        expr: Box::new(like),
                    })
                } else {
                    Ok(like)
                }
            }
            _ => Err(TranslateError::UnsupportedExpr(format!("{expr:?}"))),
        }
    }

    fn translate_literal(&self, lit: &Literal) -> QedExpr {
        match lit {
            Literal::Null => QedExpr::Null,
            _ => QedExpr::Literal {
                value: match lit {
                    Literal::Integer(n) => QedValue::Integer { value: *n },
                    Literal::Float(s) => QedValue::Float {
                        value: s.parse::<f64>().unwrap_or_else(|e| {
                            tracing::warn!("invalid float literal '{s}': {e}");
                            0.0
                        }),
                    },
                    Literal::String(s) => QedValue::String { value: s.clone() },
                    Literal::Boolean(b) => QedValue::Boolean { value: *b },
                    other => QedValue::String {
                        value: format!("{other:?}"),
                    },
                },
            },
        }
    }
}

#[cfg(test)]
mod tests;
