//! Rule: detect duplicate candidate keys from equality conditions and generate
//! a GROUP BY probe SQL to verify uniqueness.
//!
//! Manual level: only generates suggestions (probe SQL), never replaces.

use metamorphosis_core::types::{Confidence, RewriteAction, RuleCategory, SafetyLevel};
use metamorphosis_core::{RewriteContext, RewriteRule};
use ogsql_parser::ast::{
    Expr, GroupByItem, Literal, OrderByItem, SelectStatement, SelectTarget, Spanned, Statement,
    TableRef,
};
use std::collections::HashSet;
use tracing::debug;

/// Rule: detect duplicate candidate keys from equality conditions and generate
/// a GROUP BY probe SQL to verify uniqueness.
///
/// Manual level: only generates suggestions (probe SQL), never replaces.
#[derive(Debug)]
pub struct DetectDuplicateEqKeys;

impl DetectDuplicateEqKeys {
    /// Resolve the effective WHERE clause and FROM for analysis.
    ///
    /// When the outer query is a wrapper pattern (`SELECT ... FROM (subquery) WHERE pagination`)
    /// with a single Subquery(alias=None), unwrap to analyze the inner query's real WHERE clause.
    /// This handles patterns like:
    /// ```sql
    /// SELECT ... FROM (SELECT ... FROM t1 WHERE ... AND ...) WHERE rn BETWEEN ...
    /// ```
    fn resolve_query(select: &SelectStatement) -> (&Option<Expr>, &[TableRef]) {
        if select.from.len() == 1 {
            if let TableRef::Subquery {
                query, alias: None, ..
            } = &select.from[0]
            {
                return (&query.where_clause, &query.from);
            }
        }
        (&select.where_clause, &select.from)
    }
}

impl RewriteRule for DetectDuplicateEqKeys {
    fn id(&self) -> &'static str {
        "detect-duplicate-eq-keys"
    }

    fn description(&self) -> &'static str {
        "Detect candidate keys from equality conditions and generate uniqueness probe"
    }

    fn category(&self) -> RuleCategory {
        RuleCategory::DataQuality
    }

    fn safety_level(&self) -> SafetyLevel {
        SafetyLevel::Manual
    }

    fn matches(&self, ctx: &RewriteContext, stmt: &Statement) -> bool {
        let select = match stmt {
            Statement::Select(s) => &s.node,
            _ => return false,
        };

        let (where_clause, from) = Self::resolve_query(select);
        let collector = collect_eq_predicates(where_clause, from, ctx.known_variables);
        collector.tier1.len() >= 2
    }

    fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Option<RewriteAction> {
        let select = match stmt {
            Statement::Select(s) => &s.node,
            _ => return None,
        };

        let (where_clause, from) = Self::resolve_query(select);
        let collector = collect_eq_predicates(where_clause, from, ctx.known_variables);

        let mut seen = HashSet::new();
        let mut group_cols: Vec<String> = Vec::new();
        for col in collector.tier1.iter() {
            if seen.insert(col.clone()) {
                group_cols.push(col.clone());
            }
        }

        if group_cols.is_empty() {
            return None;
        }

        let limit = ctx.config.probe_default_limit;
        let probe = build_probe_statement(
            from,
            &collector.keep_exprs,
            &collector.non_eq,
            &group_cols,
            limit,
        );

        debug!(
            rule_id = self.id(),
            group_cols = ?group_cols,
            "Generated duplicate key probe"
        );

        Some(RewriteAction::Generate {
            stmt: Box::new(Statement::Select(probe)),
            purpose:
                "Candidate key duplicate detection: verify uniqueness of equality-condition columns"
                    .to_string(),
            confidence: if collector.has_subquery {
                Confidence::Medium
            } else {
                Confidence::High
            },
        })
    }
}

/// Collected equality predicate information from a WHERE clause.
struct EqPredicateCollector {
    pub tier1: Vec<String>,
    pub keep_exprs: Vec<Expr>,
    pub non_eq: Vec<Expr>,
    pub has_subquery: bool,
    table_aliases: HashSet<String>,
    known_variables: Option<HashSet<String>>,
}

impl EqPredicateCollector {
    fn new(from: &[TableRef], known_variables: Option<HashSet<String>>) -> Self {
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

    fn handle_equality(&mut self, left: &Expr, right: &Expr) {
        match (left, right) {
            // Column = Parameter/MyBatisParam → tier1: parameterized filter (variable input)
            (Expr::ColumnRef(name), Expr::Parameter(_) | Expr::MyBatisParam(_)) => {
                if let Some(col) = name.last() {
                    self.tier1.push(col.clone());
                }
            }
            // Parameter/MyBatisParam = Column → tier1
            (Expr::Parameter(_) | Expr::MyBatisParam(_), Expr::ColumnRef(name)) => {
                if let Some(col) = name.last() {
                    self.tier1.push(col.clone());
                }
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
                        if let Some(col) = l_parts.last() {
                            self.tier1.push(col.clone());
                        }
                    }
                    (false, true) => {
                        if let Some(col) = r_parts.last() {
                            self.tier1.push(col.clone());
                        }
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

/// Walk the WHERE clause and collect equality predicates.
fn collect_eq_predicates(
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
fn collect_from(expr: &Expr, col: &mut EqPredicateCollector) {
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
            // Extract equality sub-expressions for tier1 analysis,
            // but preserve the PARENTHESIZED expression in non_eq
            // to maintain correct AND/OR precedence in the probe SQL.
            extract_eq_from_non_and(inner, col);
            col.non_eq.push(expr.clone());
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
fn extract_eq_from_non_and(expr: &Expr, col: &mut EqPredicateCollector) {
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
fn make_binary_eq(left: &Expr, right: &Expr) -> Expr {
    Expr::BinaryOp {
        left: Box::new(left.clone()),
        op: "=".to_string(),
        right: Box::new(right.clone()),
    }
}

/// Build probe SQL preserving FROM and JOIN conditions (tier1 equalities excluded):
/// `SELECT col1, col2, ..., count(1) AS cnt FROM tables WHERE join_conds AND non_eq GROUP BY col1, col2, ... HAVING count(1) > 1 ORDER BY cnt DESC LIMIT N`
fn build_probe_statement(
    from: &[TableRef],
    keep_exprs: &[Expr],
    non_eq: &[Expr],
    group_cols: &[String],
    limit: usize,
) -> Spanned<SelectStatement> {
    let mut targets: Vec<SelectTarget> = group_cols
        .iter()
        .map(|col| SelectTarget::Expr(Expr::ColumnRef(vec![col.clone()]), None))
        .collect();

    targets.push(SelectTarget::Expr(
        Expr::FunctionCall {
            name: vec!["count".to_string()],
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
        Some("cnt".to_string()),
    ));

    let group_by: Vec<GroupByItem> = group_cols
        .iter()
        .map(|col| GroupByItem::Expr(Expr::ColumnRef(vec![col.clone()])))
        .collect();

    let having = Some(Expr::BinaryOp {
        left: Box::new(Expr::FunctionCall {
            name: vec!["count".to_string()],
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
        }),
        op: ">".to_string(),
        right: Box::new(Expr::Literal(Literal::Integer(1))),
    });

    let order_by = vec![OrderByItem {
        expr: Expr::ColumnRef(vec!["cnt".to_string()]),
        asc: Some(false),
        nulls_first: None,
        using: None,
    }];

    let limit_expr = Some(Expr::Literal(Literal::Integer(limit as i64)));

    let where_clause = merge_exprs(keep_exprs, non_eq);

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
        where_clause,
        connect_by: None,
        group_by,
        having,
        order_by,
        order_siblings: false,
        limit: limit_expr,
        offset: None,
        fetch: None,
        lock_clause: None,
        window_clause: vec![],
        set_operation: None,
        raw_body: None,
    })
}

/// Merge two slices of expressions with AND. Returns `None` if both empty.
fn merge_exprs(a: &[Expr], b: &[Expr]) -> Option<Expr> {
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
