//! Shared equality predicate analysis for rewrite rules.
//!
//! Provides the [`EqPredicateCollector`] and associated functions used by
//! [`DetectDuplicateEqKeys`](crate::detect_duplicate_eq_keys) and
//! [`ExtractCandidateValues`](crate::extract_candidate_values) to identify
//! parameterized vs. literal equality conditions in WHERE clauses.

use ogsql_parser::ast::{
    Expr, Ident, InsertSource, ObjectName, SelectStatement, Statement, TableRef,
};
use std::collections::HashSet;

/// Collected equality predicate information from a WHERE clause.
pub(crate) struct EqPredicateCollector {
    /// Column references with parameterized/variable equalities (tier-1 candidates).
    /// Stores full `ObjectName` (e.g. `["bs", "is_plan"]`) to preserve table qualifiers
    /// for GROUP BY generation in probe SQL.
    pub tier1: Vec<ObjectName>,
    /// Equality expressions between two known table columns (join conditions).
    pub keep_exprs: Vec<Expr>,
    /// Non-equality expressions to preserve in the WHERE clause.
    /// May include expressions that also contain parameter references;
    /// consumers should use `non_param_exprs()` when building parameter-free probes.
    pub non_eq: Vec<Expr>,
    /// Whether a subquery or EXISTS was found in the WHERE clause.
    pub has_subquery: bool,
    table_aliases: HashSet<String>,
    known_variables: Option<HashSet<String>>,
}

impl EqPredicateCollector {
    pub(crate) fn new(from: &[TableRef], known_variables: Option<HashSet<String>>) -> Self {
        let mut table_aliases = HashSet::new();
        for tr in from {
            collect_table_aliases_recursive(tr, &mut table_aliases);
        }

        if from.len() == 1 {
            if let TableRef::Subquery {
                query, alias: None, ..
            } = &from[0]
            {
                for tr in &query.from {
                    collect_table_aliases_recursive(tr, &mut table_aliases);
                }
            }
        }

        Self {
            tier1: Vec::new(),
            keep_exprs: Vec::new(),
            non_eq: Vec::new(),
            has_subquery: false,
            table_aliases,
            known_variables,
        }
    }

    fn is_known_table(&self, parts: &[Ident]) -> bool {
        parts
            .first()
            .is_some_and(|p| self.table_aliases.contains(p.as_str()))
    }

    fn classify_column_pair(&self, l_parts: &[Ident], r_parts: &[Ident]) -> (bool, bool) {
        if let Some(ref vars) = self.known_variables {
            let l_name = l_parts.last();
            let r_name = r_parts.last();
            let l_is_var = l_name.is_some_and(|n| vars.contains(n.as_str()));
            let r_is_var = r_name.is_some_and(|n| vars.contains(n.as_str()));
            if l_is_var || r_is_var {
                return (!l_is_var, !r_is_var);
            }
        }
        (self.is_known_table(l_parts), self.is_known_table(r_parts))
    }

    pub(crate) fn handle_equality(&mut self, left: &Expr, right: &Expr) {
        match (left, right) {
            (
                Expr::ColumnRef(name),
                Expr::Parameter(_)
                | Expr::MyBatisParam(_)
                | Expr::MyBatisRawExpr(_)
                | Expr::JdbcParam,
            ) => {
                self.tier1.push(name.clone());
            }
            (
                Expr::Parameter(_)
                | Expr::MyBatisParam(_)
                | Expr::MyBatisRawExpr(_)
                | Expr::JdbcParam,
                Expr::ColumnRef(name),
            ) => {
                self.tier1.push(name.clone());
            }
            // Column = Literal → non_eq: hardcoded literal has no selectivity as candidate key
            // (e.g. `sub_src_type = '8'` is a constant filter, not a variable-driven equality)
            (Expr::ColumnRef(_), Expr::Literal(_)) | (Expr::Literal(_), Expr::ColumnRef(_)) => {
                self.non_eq.push(make_binary_eq(left, right));
            }
            (Expr::ColumnRef(l_parts), Expr::ColumnRef(r_parts)) => {
                let (l_is_table, r_is_table) = self.classify_column_pair(l_parts, r_parts);

                match (l_is_table, r_is_table) {
                    (true, false) => {
                        self.tier1.push(l_parts.clone());
                    }
                    (false, true) => {
                        self.tier1.push(r_parts.clone());
                    }
                    _ => {
                        self.keep_exprs.push(make_binary_eq(left, right));
                    }
                }
            }
            // Column = Subquery/Exists → flag, add to non_eq
            (Expr::ColumnRef(_), Expr::Subquery(_) | Expr::Exists(_))
            | (Expr::Subquery(_) | Expr::Exists(_), Expr::ColumnRef(_)) => {
                self.has_subquery = true;
                self.non_eq.push(make_binary_eq(left, right));
            }
            // Column = other expression → non_eq
            (Expr::ColumnRef(_), _) | (_, Expr::ColumnRef(_)) => {
                self.non_eq.push(make_binary_eq(left, right));
            }
            // Neither is ColumnRef → ignore
            _ => {}
        }
    }

    pub(crate) fn non_param_exprs(&self) -> Vec<Expr> {
        self.non_eq
            .iter()
            .filter(|e| !contains_param(e))
            .cloned()
            .collect()
    }
}

/// Resolve the effective WHERE clause and FROM for analysis.
///
/// When the outer query is a wrapper pattern (`SELECT ... FROM (subquery) WHERE pagination`)
/// with a single Subquery, unwrap to analyze the inner query's real WHERE clause.
/// This handles patterns like:
/// ```sql
/// SELECT ... FROM (SELECT ... FROM t1 WHERE ... AND ...) WHERE rn BETWEEN ...
/// ```
pub(crate) fn resolve_query(select: &SelectStatement) -> (&Option<Expr>, &[TableRef]) {
    if select.from.len() == 1 {
        if let TableRef::Subquery { query, .. } = &select.from[0] {
            return (&query.where_clause, &query.from);
        }
    }
    (&select.where_clause, &select.from)
}

/// Walk the WHERE clause and collect equality predicates.
pub(crate) fn collect_eq_predicates(
    where_clause: &Option<Expr>,
    from: &[TableRef],
    known_variables: Option<&HashSet<String>>,
) -> EqPredicateCollector {
    let mut collector = EqPredicateCollector::new(from, known_variables.cloned());
    if let Some(expr) = where_clause {
        collect_from(expr, &mut collector);
    }
    collector
}

/// Recursively walk an expression, collecting equality predicates.
pub(crate) fn collect_from(expr: &Expr, col: &mut EqPredicateCollector) {
    match expr {
        Expr::BinaryOp { left, op, right } => {
            let op_upper = op.to_uppercase();
            match op_upper.as_str() {
                "=" => {
                    col.handle_equality(left, right);
                }
                "AND" => {
                    collect_from(left, col);
                    collect_from(right, col);
                }
                _ => {
                    // For non-flattenable operators (OR, LIKE, BETWEEN, etc.):
                    // Preserve the full expression in WHERE without individually
                    // recursing children — otherwise bare IsNull sub-expressions
                    // get added separately in addition to the containing OR.
                    // Extract equality sub-expressions for tier1 analysis.
                    extract_eq_from_non_and(left, col);
                    extract_eq_from_non_and(right, col);
                    col.non_eq.push(expr.clone());
                }
            }
        }
        Expr::Exists(_) | Expr::Subquery(_) => {
            col.has_subquery = true;
            col.non_eq.push(expr.clone());
        }
        Expr::Parenthesized(inner) => match inner.as_ref() {
            Expr::BinaryOp { left, op, right } if op.to_uppercase() == "=" => {
                col.handle_equality(left, right);
            }
            _ => {
                extract_eq_from_non_and(inner, col);
                col.non_eq.push(expr.clone());
            }
        },
        _ => {
            col.non_eq.push(expr.clone());
        }
    }
}

/// Extract equality sub-expressions from non-AND, non-= operators (like OR)
/// for tier1 analysis, without modifying the non_eq/keep_exprs collections.
/// This complements `collect_from` which preserves the full expression but
/// doesn't recurse into children of non-flattenable operators.
pub(crate) fn extract_eq_from_non_and(expr: &Expr, col: &mut EqPredicateCollector) {
    match expr {
        Expr::BinaryOp { left, op, right } => {
            let op_upper = op.to_uppercase();
            match op_upper.as_str() {
                "=" => {
                    col.handle_equality(left, right);
                }
                _ => {
                    extract_eq_from_non_and(left, col);
                    extract_eq_from_non_and(right, col);
                }
            }
        }
        Expr::Parenthesized(inner) => {
            extract_eq_from_non_and(inner, col);
        }
        _ => {}
    }
}

/// Reconstruct a `BinaryOp { op: "=" }` expression.
pub(crate) fn make_binary_eq(left: &Expr, right: &Expr) -> Expr {
    Expr::BinaryOp {
        left: Box::new(left.clone()),
        op: "=".to_string(),
        right: Box::new(right.clone()),
    }
}

/// Merge two slices of expressions with AND. Returns `None` if both empty.
pub(crate) fn merge_exprs(a: &[Expr], b: &[Expr]) -> Option<Expr> {
    let combined: Vec<&Expr> = a.iter().chain(b.iter()).collect();
    match combined.len() {
        0 => None,
        1 => Some(combined[0].clone()),
        _ => {
            let mut iter = combined.into_iter();
            let first = iter.next().unwrap().clone();
            Some(iter.fold(first, |acc, expr| Expr::BinaryOp {
                left: Box::new(acc),
                op: "AND".to_string(),
                right: Box::new(expr.clone()),
            }))
        }
    }
}

/// Recursively collect table/subquery aliases from any TableRef tree.
fn collect_table_aliases_recursive(tr: &TableRef, aliases: &mut HashSet<String>) {
    match tr {
        TableRef::Table { name, alias, .. } => {
            if let Some(a) = alias {
                aliases.insert(a.clone());
            }
            if let Some(bare) = name.last() {
                aliases.insert(bare.as_str().to_string());
            }
        }
        TableRef::Subquery { alias, .. }
        | TableRef::FunctionCall { alias, .. }
        | TableRef::Values { alias, .. } => {
            if let Some(a) = alias {
                aliases.insert(a.clone());
            }
        }
        TableRef::Join { left, right, .. } => {
            collect_table_aliases_recursive(left, aliases);
            collect_table_aliases_recursive(right, aliases);
        }
        TableRef::Pivot { source, .. } | TableRef::Unpivot { source, .. } => {
            collect_table_aliases_recursive(source, aliases);
        }
    }
}

// ── QueryScope ──

/// A sub-query scope extracted from a WHERE clause expression tree.
///
/// Each scope captures the context (`from` and `where_clause`) needed to
/// build a data quality probe for that particular sub-expression. Scopes
/// are collected by [`extract_query_scopes`] which walks the expression
/// tree looking for `EXISTS`, `IN (subquery)`, `Subquery`, and
/// `ScalarSublink` nodes, then recursing into each subquery's own WHERE.
#[derive(Debug, Clone)]
pub(crate) struct QueryScope {
    /// Human-readable label for the scope.
    pub label: String,
    /// FROM tables from the containing query.
    pub from: Vec<TableRef>,
    /// WHERE clause of this scope (the subquery's WHERE or a CTE's WHERE).
    pub where_clause: Option<Expr>,
    /// Whether this scope came from a CTE (vs an inline subquery).
    #[allow(dead_code)]
    pub is_cte: bool,
}

/// Walk a WHERE clause expression tree and collect all subquery scopes.
///
/// Recursively finds `EXISTS`, `Subquery`, `InSubquery`, and `ScalarSublink`
/// nodes, creating a [`QueryScope`] for each. Also recurses into each found
/// subquery's own WHERE to find nested subqueries.
pub(crate) fn extract_query_scopes(
    where_clause: &Option<Expr>,
    from: &[TableRef],
    known_variables: Option<&HashSet<String>>,
) -> Vec<QueryScope> {
    let mut scopes = Vec::new();
    let mut counter = 0u32;
    if let Some(expr) = where_clause {
        walk_subquery_scopes(expr, from, known_variables, &mut counter, &mut scopes);
    }
    scopes
}

/// Recursively walk an expression tree looking for subquery nodes.
#[allow(clippy::only_used_in_recursion)]
fn walk_subquery_scopes(
    expr: &Expr,
    from: &[TableRef],
    known_variables: Option<&HashSet<String>>,
    counter: &mut u32,
    scopes: &mut Vec<QueryScope>,
) {
    match expr {
        Expr::Exists(subquery)
        | Expr::Subquery(subquery)
        | Expr::InSubquery { subquery, .. }
        | Expr::ScalarSublink { subquery, .. } => {
            *counter += 1;
            let label = format!("subquery_{}", counter);
            // Use the subquery's own FROM for the scope so probes reference
            // the correct tables.
            push_subquery_scope(
                &label,
                &subquery.from,
                &subquery.where_clause,
                false,
                scopes,
            );
            // Recurse into the subquery's own WHERE for nested subqueries.
            if let Some(ref inner_where) = subquery.where_clause {
                walk_subquery_scopes(
                    inner_where,
                    &subquery.from,
                    known_variables,
                    counter,
                    scopes,
                );
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            walk_subquery_scopes(left, from, known_variables, counter, scopes);
            walk_subquery_scopes(right, from, known_variables, counter, scopes);
        }
        Expr::UnaryOp { expr: inner, .. } => {
            walk_subquery_scopes(inner, from, known_variables, counter, scopes);
        }
        Expr::Parenthesized(inner) => {
            walk_subquery_scopes(inner, from, known_variables, counter, scopes);
        }
        _ => {}
    }
}

/// Create a [`QueryScope`] and push it onto the scopes vector.
fn push_subquery_scope(
    label: &str,
    from: &[TableRef],
    where_clause: &Option<Expr>,
    is_cte: bool,
    scopes: &mut Vec<QueryScope>,
) {
    scopes.push(QueryScope {
        label: label.to_string(),
        from: from.to_vec(),
        where_clause: where_clause.clone(),
        is_cte,
    });
}

/// Extract all query scopes from a statement for equality analysis.
///
/// For SELECT, uses `resolve_query` to unwrap common pagination wrapper patterns.
/// For UPDATE/DELETE, uses the statement's FROM/USING and WHERE directly.
/// For INSERT, uses the source SELECT's FROM/WHERE if the source is a SELECT.
/// For MERGE, uses the source table and ON condition.
/// For all DML types, also extracts subquery scopes from WHERE clauses and
/// CTE definitions from WITH clauses.
pub(crate) fn extract_statement_scopes(
    stmt: &Statement,
    known_variables: Option<&std::collections::HashSet<String>>,
) -> Vec<QueryScope> {
    let mut scopes = Vec::new();
    let mut counter = 0u32;

    match stmt {
        Statement::Select(s) => {
            let (where_clause, from) = resolve_query(&s.node);
            counter += 1;
            scopes.push(QueryScope {
                label: format!("main_{}", counter),
                from: from.to_vec(),
                where_clause: where_clause.clone(),
                is_cte: false,
            });

            scopes.extend(extract_query_scopes(where_clause, from, known_variables));

            extract_cte_scopes(&s.node.with, known_variables, &mut counter, &mut scopes);
        }
        Statement::Update(s) => {
            counter += 1;
            scopes.push(QueryScope {
                label: format!("main_{}", counter),
                from: s.node.from.clone(),
                where_clause: s.node.where_clause.clone(),
                is_cte: false,
            });

            scopes.extend(extract_query_scopes(
                &s.node.where_clause,
                &s.node.from,
                known_variables,
            ));

            extract_cte_scopes(&s.node.with, known_variables, &mut counter, &mut scopes);
        }
        Statement::Delete(s) => {
            counter += 1;
            scopes.push(QueryScope {
                label: format!("main_{}", counter),
                from: s.node.using.clone(),
                where_clause: s.node.where_clause.clone(),
                is_cte: false,
            });

            scopes.extend(extract_query_scopes(
                &s.node.where_clause,
                &s.node.using,
                known_variables,
            ));

            extract_cte_scopes(&s.node.with, known_variables, &mut counter, &mut scopes);
        }
        Statement::Insert(s) => {
            if let InsertSource::Select(ref select) = s.node.source {
                counter += 1;
                scopes.push(QueryScope {
                    label: format!("insert_select_{}", counter),
                    from: select.from.clone(),
                    where_clause: select.where_clause.clone(),
                    is_cte: false,
                });

                scopes.extend(extract_query_scopes(
                    &select.where_clause,
                    &select.from,
                    known_variables,
                ));
            }

            extract_cte_scopes(&s.node.with, known_variables, &mut counter, &mut scopes);
        }
        Statement::Merge(s) => {
            let merge_from = vec![s.node.source.clone()];
            let merge_where = Some(s.node.on_condition.clone());

            counter += 1;
            scopes.push(QueryScope {
                label: format!("main_{}", counter),
                from: merge_from.clone(),
                where_clause: merge_where.clone(),
                is_cte: false,
            });

            scopes.extend(extract_query_scopes(
                &merge_where,
                &merge_from,
                known_variables,
            ));
        }
        _ => {}
    }

    scopes
}

/// Extract query scopes from CTE definitions.
pub(crate) fn extract_cte_scopes(
    with: &Option<ogsql_parser::ast::WithClause>,
    known_variables: Option<&std::collections::HashSet<String>>,
    counter: &mut u32,
    scopes: &mut Vec<QueryScope>,
) {
    if let Some(ref with_clause) = with {
        for cte in &with_clause.ctes {
            *counter += 1;
            scopes.push(QueryScope {
                label: format!("cte:{}", cte.name),
                from: cte.query.from.clone(),
                where_clause: cte.query.where_clause.clone(),
                is_cte: true,
            });

            scopes.extend(extract_query_scopes(
                &cte.query.where_clause,
                &cte.query.from,
                known_variables,
            ));
        }
    }
}

/// Check whether any table in `from` references the given CTE name.
#[allow(dead_code)]
pub(crate) fn references_cte(from: &[TableRef], cte_name: &str) -> bool {
    from.iter().any(|tr| match tr {
        TableRef::Table { name, .. } => name.last().is_some_and(|i| i.as_str() == cte_name),
        _ => false,
    })
}

pub(crate) fn contains_param(expr: &Expr) -> bool {
    match expr {
        Expr::Parameter(_) | Expr::MyBatisParam(_) | Expr::MyBatisRawExpr(_) | Expr::JdbcParam => {
            true
        }
        Expr::BinaryOp { left, right, .. } => contains_param(left) || contains_param(right),
        Expr::UnaryOp { expr, .. } => contains_param(expr),
        Expr::Parenthesized(inner) => contains_param(inner),
        Expr::IsNull { expr, .. } => contains_param(expr),
        Expr::FunctionCall { args, filter, .. } => {
            args.iter().any(contains_param) || filter.as_ref().is_some_and(|f| contains_param(f))
        }
        Expr::Case {
            operand,
            whens,
            else_expr,
        } => {
            operand.as_ref().is_some_and(|e| contains_param(e))
                || whens
                    .iter()
                    .any(|w| contains_param(&w.condition) || contains_param(&w.result))
                || else_expr.as_ref().is_some_and(|e| contains_param(e))
        }
        Expr::Between {
            expr, low, high, ..
        } => contains_param(expr) || contains_param(low) || contains_param(high),
        Expr::InList { list, .. } => list.iter().any(contains_param),
        Expr::InSubquery { .. } | Expr::Exists(_) | Expr::Subquery(_) => false,
        Expr::TypeCast { expr, .. } => contains_param(expr),
        Expr::Treat { expr, .. } => contains_param(expr),
        Expr::Array(exprs) => exprs.iter().any(contains_param),
        Expr::Subscript {
            object,
            lower,
            upper,
            ..
        } => {
            contains_param(object)
                || lower.as_ref().is_some_and(|e| contains_param(e))
                || upper.as_ref().is_some_and(|e| contains_param(e))
        }
        _ => false,
    }
}
