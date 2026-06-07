//! Shared equality predicate analysis for rewrite rules.
//!
//! Provides the [`EqPredicateCollector`] and associated functions used by
//! [`DetectDuplicateEqKeys`](crate::detect_duplicate_eq_keys) and
//! [`ExtractCandidateValues`](crate::extract_candidate_values) to identify
//! parameterized vs. literal equality conditions in WHERE clauses.

use ogsql_parser::ast::{Expr, SelectStatement, TableRef};
use std::collections::HashSet;

/// Collected equality predicate information from a WHERE clause.
pub(crate) struct EqPredicateCollector {
    /// Column references with parameterized/variable equalities (tier-1 candidates).
    /// Stores full `ObjectName` (e.g. `["bs", "is_plan"]`) to preserve table qualifiers
    /// for GROUP BY generation in probe SQL.
    pub tier1: Vec<Vec<String>>,
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

    fn is_known_table(&self, parts: &[String]) -> bool {
        parts
            .first()
            .is_some_and(|p| self.table_aliases.contains(p))
    }

    fn classify_column_pair(&self, l_parts: &[String], r_parts: &[String]) -> (bool, bool) {
        if let Some(ref vars) = self.known_variables {
            let l_name = l_parts.last();
            let r_name = r_parts.last();
            let l_is_var = l_name.is_some_and(|n| vars.contains(n));
            let r_is_var = r_name.is_some_and(|n| vars.contains(n));
            if l_is_var || r_is_var {
                return (!l_is_var, !r_is_var);
            }
        }
        (self.is_known_table(l_parts), self.is_known_table(r_parts))
    }

    pub(crate) fn handle_equality(&mut self, left: &Expr, right: &Expr) {
        match (left, right) {
            (Expr::ColumnRef(name), Expr::Parameter(_) | Expr::MyBatisParam(_) | Expr::JdbcParam) => {
                self.tier1.push(name.clone());
            }
            (Expr::Parameter(_) | Expr::MyBatisParam(_) | Expr::JdbcParam, Expr::ColumnRef(name)) => {
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
        Expr::Parenthesized(inner) => {
            match inner.as_ref() {
                Expr::BinaryOp { left, op, right }
                    if op.to_uppercase() == "=" =>
                {
                    col.handle_equality(left, right);
                }
                _ => {
                    extract_eq_from_non_and(inner, col);
                    col.non_eq.push(expr.clone());
                }
            }
        }
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
                aliases.insert(bare.clone());
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

pub(crate) fn contains_param(expr: &Expr) -> bool {
    match expr {
        Expr::Parameter(_) | Expr::MyBatisParam(_) | Expr::JdbcParam => true,
        Expr::BinaryOp { left, right, .. } => contains_param(left) || contains_param(right),
        Expr::UnaryOp { expr, .. } => contains_param(expr),
        Expr::Parenthesized(inner) => contains_param(inner),
        Expr::IsNull { expr, .. } => contains_param(expr),
        Expr::FunctionCall { args, filter, .. } => {
            args.iter().any(contains_param)
                || filter.as_ref().is_some_and(|f| contains_param(f))
        }
        Expr::Case { operand, whens, else_expr } => {
            operand.as_ref().is_some_and(|e| contains_param(e))
                || whens
                    .iter()
                    .any(|w| contains_param(&w.condition) || contains_param(&w.result))
                || else_expr.as_ref().is_some_and(|e| contains_param(e))
        }
        Expr::Between { expr, low, high, .. } => {
            contains_param(expr) || contains_param(low) || contains_param(high)
        }
        Expr::InList { list, .. } => list.iter().any(contains_param),
        Expr::InSubquery { .. } | Expr::Exists(_) | Expr::Subquery(_) => false,
        Expr::TypeCast { expr, .. } => contains_param(expr),
        Expr::Treat { expr, .. } => contains_param(expr),
        Expr::Array(exprs) => exprs.iter().any(contains_param),
        Expr::Subscript { object, lower, upper, .. } => {
            contains_param(object)
                || lower.as_ref().is_some_and(|e| contains_param(e))
                || upper.as_ref().is_some_and(|e| contains_param(e))
        }
        _ => false,
    }
}
