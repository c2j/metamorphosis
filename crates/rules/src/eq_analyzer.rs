//! Shared equality predicate analysis for rewrite rules.
//!
//! Provides the [`EqPredicateCollector`] and associated functions used by
//! [`DetectDuplicateEqKeys`](crate::detect_duplicate_eq_keys) and
//! [`ExtractCandidateValues`](crate::extract_candidate_values) to identify
//! parameterized vs. literal equality conditions in WHERE clauses.

use ogsql_parser::ast::{
    Expr, Ident, InsertSource, ObjectName, SelectStatement, SelectTarget, Statement, TableRef,
};
use std::collections::{HashMap, HashSet};

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
    /// Single-part ColumnRef names identified as parameters during classification.
    /// Populated by `handle_equality` when the opposing side of a ColumnRef=ColumnRef
    /// equality is classified as a parameter (single-part, not a known table alias).
    /// Stored lowercased for case-insensitive matching — SQL identifiers are
    /// case-insensitive, so `v_gffsrq` in an equality and `V_GFFSRQ` in a
    /// BETWEEN must resolve to the same parameter.
    param_names: HashSet<String>,
    /// True if any classified ColumnRef=ColumnRef equality referenced a column
    /// whose qualifier is multi-part but unknown in the current scope's FROM
    /// (i.e., a correlated reference to an outer query).
    pub(crate) has_correlated_ref: bool,
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
            param_names: HashSet::new(),
            has_correlated_ref: false,
        }
    }

    fn is_known_table(&self, parts: &[Ident]) -> bool {
        parts.first().is_some_and(|p| {
            let lower = p.as_str().to_lowercase();
            self.table_aliases.contains(lower.as_str())
        })
    }

    /// Like `is_known_table`, but also returns true for qualified references
    /// (`outer.col`) whose prefix belongs to an outer query scope. These are
    /// correlated references, not user parameters — only unqualified names
    /// that aren't local table aliases are treated as bind parameters.
    fn is_known_table_or_correlated(&self, parts: &[Ident]) -> bool {
        self.is_known_table(parts) || parts.len() > 1
    }

    fn classify_column_pair(&self, l_parts: &[Ident], r_parts: &[Ident]) -> (bool, bool) {
        if let Some(ref vars) = self.known_variables {
            let l_name = l_parts.last();
            let r_name = r_parts.last();
            let l_is_var = l_name.is_some_and(|n| {
                let lower = n.as_str().to_lowercase();
                vars.contains(lower.as_str())
            });
            let r_is_var = r_name.is_some_and(|n| {
                let lower = n.as_str().to_lowercase();
                vars.contains(lower.as_str())
            });
            if l_is_var || r_is_var {
                return (!l_is_var, !r_is_var);
            }
        }
        (
            self.is_known_table_or_correlated(l_parts),
            self.is_known_table_or_correlated(r_parts),
        )
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
                        if let Some(name) = r_parts.last() {
                            self.param_names.insert(name.as_str().to_lowercase());
                        }
                    }
                    (false, true) => {
                        self.tier1.push(r_parts.clone());
                        if let Some(name) = l_parts.last() {
                            self.param_names.insert(name.as_str().to_lowercase());
                        }
                    }
                    _ => {
                        self.keep_exprs.push(make_binary_eq(left, right));
                        // Detect correlated refs: either side has multi-part
                        // name whose prefix is NOT in current scope's aliases.
                        let l_correlated = l_parts.len() > 1 && !self.is_known_table(l_parts);
                        let r_correlated = r_parts.len() > 1 && !self.is_known_table(r_parts);
                        if l_correlated || r_correlated {
                            self.has_correlated_ref = true;
                        }
                    }
                }
            }
            // Column = Subquery/Exists → flag, add to non_eq
            (Expr::ColumnRef(_), Expr::Subquery(_) | Expr::Exists(_))
            | (Expr::Subquery(_) | Expr::Exists(_), Expr::ColumnRef(_)) => {
                self.has_subquery = true;
                self.non_eq.push(make_binary_eq(left, right));
            }
            // Column = other expression → tier1 if the other side is
            // parameter-driven (incl. function-wrapped vars like to_char(v)),
            // otherwise non_eq.
            (Expr::ColumnRef(name), other) | (other, Expr::ColumnRef(name)) => {
                let (params, _cols) = {
                    let mut params = Vec::new();
                    let mut cols = Vec::new();
                    classify_expr_columns(other, self, &mut params, &mut cols);
                    (params, cols)
                };
                if !params.is_empty() || contains_param(other) {
                    self.tier1.push(name.clone());
                    for p in params {
                        self.param_names.insert(p);
                    }
                } else {
                    self.non_eq.push(make_binary_eq(left, right));
                }
            }
            // Neither side is a bare ColumnRef — e.g. function-wrapped columns
            // like `substr(t.code, 1, 17) = substr(v_param, 1, 17)`. Recursively
            // classify columns/params the same way handle_between does, then
            // push to non_eq so non_param_exprs() can filter it if needed.
            _ => self.handle_expr_equality(left, right),
        }
    }

    /// Classify an equality where neither side is a bare `ColumnRef`.
    ///
    /// Walks both sides with `classify_expr_columns` to find table columns and
    /// parameters. If parameters are present, table columns are pushed to tier1
    /// and parameter names are registered. The full expression is always pushed
    /// to `non_eq` so `non_param_exprs()` can filter it when it references a
    /// classified parameter.
    fn handle_expr_equality(&mut self, left: &Expr, right: &Expr) {
        let (params, cols) = {
            let mut params = Vec::new();
            let mut cols = Vec::new();
            classify_expr_columns(left, self, &mut params, &mut cols);
            classify_expr_columns(right, self, &mut params, &mut cols);
            (params, cols)
        };

        let has_param = !params.is_empty() || contains_param(left) || contains_param(right);

        if has_param {
            for name in params {
                self.param_names.insert(name);
            }
            for col in cols {
                self.tier1.push(col);
            }
        }
        self.non_eq.push(make_binary_eq(left, right));
    }

    /// Tier1-only classifier used by [`extract_eq_from_non_and`].
    ///
    /// Pushes to `tier1` and `param_names` only; never touches `non_eq` or
    /// `keep_exprs`. This honors the contract documented on
    /// [`extract_eq_from_non_and`]: "without modifying the non_eq/keep_exprs
    /// collections".
    ///
    /// Duplicates a subset of [`handle_equality`] but deliberately so — a
    /// single boolean flag on `handle_equality` would couple two contracts.
    fn classify_for_tier1_only(&mut self, left: &Expr, right: &Expr) {
        match (left, right) {
            (
                Expr::ColumnRef(name),
                Expr::Parameter(_)
                | Expr::MyBatisParam(_)
                | Expr::MyBatisRawExpr(_)
                | Expr::JdbcParam,
            )
            | (
                Expr::Parameter(_)
                | Expr::MyBatisParam(_)
                | Expr::MyBatisRawExpr(_)
                | Expr::JdbcParam,
                Expr::ColumnRef(name),
            ) => {
                self.tier1.push(name.clone());
            }
            (Expr::ColumnRef(l_parts), Expr::ColumnRef(r_parts)) => {
                let (l_is_table, r_is_table) = self.classify_column_pair(l_parts, r_parts);
                match (l_is_table, r_is_table) {
                    (true, false) => {
                        self.tier1.push(l_parts.clone());
                        if let Some(n) = r_parts.last() {
                            self.param_names.insert(n.as_str().to_lowercase());
                        }
                    }
                    (false, true) => {
                        self.tier1.push(r_parts.clone());
                        if let Some(n) = l_parts.last() {
                            self.param_names.insert(n.as_str().to_lowercase());
                        }
                    }
                    _ => {} // join condition: do NOT push to keep_exprs here
                }
            }
            _ => {} // deliberately no-op for non-tier1 cases
        }
    }

    /// Classify a BETWEEN expression for tier1 extraction.
    ///
    /// If the BETWEEN references any parameter (explicit param node, unqualified
    /// ColumnRef not in table aliases, or expression containing such), all
    /// known-table ColumnRefs in the BETWEEN are pushed to tier1 so the probe
    /// GROUP BY shows valid ranges/values for the parameter.
    fn handle_between(&mut self, expr: &Expr, low: &Expr, high: &Expr) {
        let (params, cols) = {
            let mut params = Vec::new();
            let mut cols = Vec::new();
            for part in &[expr, low, high] {
                classify_expr_columns(part, self, &mut params, &mut cols);
            }
            (params, cols)
        };

        let has_param = !params.is_empty()
            || contains_param(expr)
            || contains_param(low)
            || contains_param(high);

        if has_param {
            for name in params {
                self.param_names.insert(name);
            }
            for col in cols {
                self.tier1.push(col);
            }
        }
    }

    /// Classify a LIKE expression for tier1 extraction. Same principle as
    /// `handle_between`: if the LIKE is param-bearing, extract the subject
    /// column (and any other known-table columns in the pattern) to tier1.
    fn handle_like(&mut self, subj: &Expr, pattern: &Expr, escape: &Option<Box<Expr>>) {
        let (params, cols) = {
            let mut params = Vec::new();
            let mut cols = Vec::new();
            classify_expr_columns(subj, self, &mut params, &mut cols);
            classify_expr_columns(pattern, self, &mut params, &mut cols);
            if let Some(e) = escape {
                classify_expr_columns(e, self, &mut params, &mut cols);
            }
            (params, cols)
        };

        let has_param = !params.is_empty()
            || contains_param(subj)
            || contains_param(pattern)
            || escape.as_ref().is_some_and(|e| contains_param(e));

        if has_param {
            for name in params {
                self.param_names.insert(name);
            }
            for col in cols {
                self.tier1.push(col);
            }
        }
    }

    /// Pre-scan WHERE for stored-proc variables before equality classification.
    /// An unqualified single-part ColumnRef not in table aliases is treated as
    /// a parameter, UNLESS it appears as the column side of a `col = literal`
    /// equality (which is always a data filter, never a constant condition on
    /// a variable). This catches variables that only appear in LIKE, IN, or
    /// IS NULL — contexts the equality classifier does not cover.
    fn pre_scan_params(&mut self, expr: &Expr) {
        let literal_cols = collect_literal_compared_cols(expr);
        let params = {
            let mut params = Vec::new();
            let mut cols = Vec::new();
            classify_expr_columns(expr, self, &mut params, &mut cols);
            params
        };
        for name in params {
            if !literal_cols.contains(&name) {
                self.param_names.insert(name);
            }
        }
    }

    /// True if `expr` contains any `ColumnRef` whose last identifier matches a name
    /// in `self.param_names`. Used to filter non-equality expressions that
    /// reference stored-proc variables not represented as `Expr::Parameter`.
    fn references_classified_param(&self, expr: &Expr) -> bool {
        let names = &self.param_names;
        if names.is_empty() {
            return false;
        }
        walk_column_refs(expr, &|parts| {
            parts.last().is_some_and(|p| {
                let lower = p.as_str().to_lowercase();
                names.contains(lower.as_str())
            })
        })
    }

    /// True if `expr` contains any parameter-like token (`Parameter`, `JdbcParam`,
    /// etc.) or any classified stored-proc variable reference.
    pub(crate) fn contains_classified_param(&self, expr: &Expr) -> bool {
        contains_param(expr) || self.references_classified_param(expr)
    }

    pub(crate) fn non_param_exprs(&self) -> Vec<Expr> {
        self.non_eq
            .iter()
            .filter(|e| !self.contains_classified_param(e))
            .cloned()
            .collect()
    }
}

/// Resolve the effective WHERE clause and FROM for analysis.
///
/// When the outer query wraps a single subquery in FROM, the outer WHERE is
/// merged into the inner scope: outer column references are resolved through
/// the subquery's projection list and AND-combined with the inner WHERE. This
/// preserves real data filters (e.g. `close_date = to_char(v_date, ...)`) that
/// a naive unwrap would discard. Pagination wrappers are unaffected: their
/// predicates resolve to window functions (dropped) or non-parameterized
/// conditions and contribute no tier-1 equalities.
pub(crate) fn resolve_query(select: &SelectStatement) -> (Option<Expr>, &[TableRef]) {
    if select.from.len() == 1 {
        if let TableRef::Subquery { query, alias, .. } = &select.from[0] {
            let merged = merge_outer_where(
                &select.where_clause,
                &query.where_clause,
                &query.targets,
                alias.as_deref(),
                &query.from,
            );
            return (merged, &query.from);
        }
    }
    (select.where_clause.clone(), &select.from)
}

/// Merge an outer query's WHERE into the inner subquery's scope.
fn merge_outer_where(
    outer_where: &Option<Expr>,
    inner_where: &Option<Expr>,
    inner_targets: &[SelectTarget],
    subquery_alias: Option<&str>,
    base_from: &[TableRef],
) -> Option<Expr> {
    let outer = match outer_where {
        Some(e) => e,
        None => return inner_where.clone(),
    };

    let proj_map = build_projection_map(inner_targets);

    let mut base_aliases = HashSet::new();
    for tr in base_from {
        collect_table_aliases_recursive(tr, &mut base_aliases);
    }

    let substituted = substitute_outer_columns(outer, &proj_map, subquery_alias);

    let mut conjunct_refs: Vec<&Expr> = Vec::new();
    split_and_conjuncts_rec(&substituted, &mut conjunct_refs);

    let mut surviving: Vec<Expr> = Vec::new();
    for c in conjunct_refs {
        if conjunct_keepable(c, &base_aliases) {
            surviving.push(c.clone());
        }
    }

    let inner_slice: &[Expr] = match inner_where {
        Some(e) => std::slice::from_ref(e),
        None => &[],
    };
    merge_exprs(inner_slice, &surviving)
}

fn build_projection_map(targets: &[SelectTarget]) -> HashMap<String, Expr> {
    let mut map = HashMap::new();
    for target in targets {
        if let SelectTarget::Expr(expr, alias) = target {
            let key = match alias {
                Some(a) => Some(a.as_str().to_lowercase()),
                None => match expr {
                    Expr::ColumnRef(parts) => parts.last().map(|p| p.as_str().to_lowercase()),
                    _ => None,
                },
            };
            if let Some(k) = key {
                map.entry(k).or_insert_with(|| expr.clone());
            }
        }
    }
    map
}

/// Recursively rewrite an expression, replacing outer column references with
/// their projection sources. Covers the WHERE-structural variants
/// (ColumnRef/BinaryOp/Parenthesized); other variants are cloned unchanged.
fn substitute_outer_columns(
    expr: &Expr,
    proj_map: &HashMap<String, Expr>,
    subquery_alias: Option<&str>,
) -> Expr {
    match expr {
        Expr::ColumnRef(parts) => {
            let lookup = |ident: &Ident| {
                let lower = ident.as_str().to_lowercase();
                proj_map.get(&lower).cloned()
            };
            match parts.len() {
                1 => lookup(&parts[0]).unwrap_or_else(|| expr.clone()),
                2 if subquery_alias.is_some_and(|a| a.eq_ignore_ascii_case(parts[0].as_str())) => {
                    lookup(&parts[1]).unwrap_or_else(|| expr.clone())
                }
                _ => expr.clone(),
            }
        }
        Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
            left: Box::new(substitute_outer_columns(left, proj_map, subquery_alias)),
            op: op.clone(),
            right: Box::new(substitute_outer_columns(right, proj_map, subquery_alias)),
        },
        Expr::Parenthesized(inner) => Expr::Parenthesized(Box::new(substitute_outer_columns(
            inner,
            proj_map,
            subquery_alias,
        ))),
        _ => expr.clone(),
    }
}

fn split_and_conjuncts_rec<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    if let Expr::BinaryOp { left, op, right } = expr {
        if op.eq_ignore_ascii_case("AND") {
            split_and_conjuncts_rec(left, out);
            split_and_conjuncts_rec(right, out);
            return;
        }
    }
    out.push(expr);
}

/// A merged conjunct is keepable iff it references a base-table column and
/// contains no window function (which is invalid in WHERE).
fn conjunct_keepable(expr: &Expr, base_aliases: &HashSet<String>) -> bool {
    !expr_has_window_func(expr) && expr_has_base_column(expr, base_aliases)
}

fn expr_has_window_func(expr: &Expr) -> bool {
    match expr {
        Expr::FunctionCall { args, over, .. } => {
            over.is_some() || args.iter().any(expr_has_window_func)
        }
        Expr::BinaryOp { left, right, .. } => {
            expr_has_window_func(left) || expr_has_window_func(right)
        }
        Expr::Parenthesized(inner) => expr_has_window_func(inner),
        _ => false,
    }
}

fn expr_has_base_column(expr: &Expr, base_aliases: &HashSet<String>) -> bool {
    match expr {
        Expr::ColumnRef(parts) => parts.first().is_some_and(|p| {
            let lower = p.as_str().to_lowercase();
            base_aliases.contains(&lower)
        }),
        Expr::BinaryOp { left, right, .. } => {
            expr_has_base_column(left, base_aliases) || expr_has_base_column(right, base_aliases)
        }
        Expr::Parenthesized(inner) => expr_has_base_column(inner, base_aliases),
        Expr::FunctionCall { args, .. } => {
            args.iter().any(|a| expr_has_base_column(a, base_aliases))
        }
        _ => false,
    }
}

/// Walk the WHERE clause and collect equality predicates.
pub(crate) fn collect_eq_predicates(
    where_clause: &Option<Expr>,
    from: &[TableRef],
    known_variables: Option<&HashSet<String>>,
) -> EqPredicateCollector {
    let mut collector = EqPredicateCollector::new(from, known_variables.cloned());
    if let Some(expr) = where_clause {
        collector.pre_scan_params(expr);
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
        Expr::Between {
            expr: subj,
            low,
            high,
            ..
        } => {
            col.handle_between(subj, low, high);
            col.non_eq.push(expr.clone());
        }
        Expr::Like {
            expr: subj,
            pattern,
            escape,
            ..
        } => {
            col.handle_like(subj, pattern, escape);
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
pub(crate) fn extract_eq_from_non_and(expr: &Expr, col: &mut EqPredicateCollector) {
    match expr {
        Expr::BinaryOp { left, op, right } => {
            let op_upper = op.to_uppercase();
            match op_upper.as_str() {
                "=" => col.classify_for_tier1_only(left, right),
                _ => {
                    extract_eq_from_non_and(left, col);
                    extract_eq_from_non_and(right, col);
                }
            }
        }
        Expr::Parenthesized(inner) => {
            extract_eq_from_non_and(inner, col);
        }
        Expr::Between {
            expr: subj,
            low,
            high,
            ..
        } => col.handle_between(subj, low, high),
        Expr::Like {
            expr: subj,
            pattern,
            escape,
            ..
        } => col.handle_like(subj, pattern, escape),
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
                aliases.insert(a.to_lowercase());
            }
            if let Some(bare) = name.last() {
                aliases.insert(bare.as_str().to_lowercase());
            }
        }
        TableRef::Subquery { alias, .. }
        | TableRef::FunctionCall { alias, .. }
        | TableRef::Values { alias, .. } => {
            if let Some(a) = alias {
                aliases.insert(a.to_lowercase());
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

            scopes.extend(extract_query_scopes(&where_clause, from, known_variables));

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

/// Walk an expression tree and call `f` on each [`Expr::ColumnRef`] found.
///
/// Returns `true` if any call to `f` returns `true`. Mirrors the variant
/// coverage of [`contains_param`] to ensure consistent parameter detection
/// for both AST-level parameters (`Parameter`, `JdbcParam`, etc.) and
/// stored-proc variables that appear as unqualified `ColumnRef` nodes.
pub(crate) fn walk_column_refs(expr: &Expr, f: &dyn Fn(&[Ident]) -> bool) -> bool {
    match expr {
        Expr::ColumnRef(parts) => f(parts),
        Expr::BinaryOp { left, right, .. } => {
            walk_column_refs(left, f) || walk_column_refs(right, f)
        }
        Expr::UnaryOp { expr, .. } => walk_column_refs(expr, f),
        Expr::Parenthesized(inner) => walk_column_refs(inner, f),
        Expr::IsNull { expr, .. } => walk_column_refs(expr, f),
        Expr::FunctionCall { args, filter, .. } => {
            args.iter().any(|a| walk_column_refs(a, f))
                || filter
                    .as_ref()
                    .is_some_and(|filt| walk_column_refs(filt, f))
        }
        Expr::SpecialFunction { args, .. } => args.iter().any(|a| walk_column_refs(a, f)),
        Expr::Case {
            operand,
            whens,
            else_expr,
        } => {
            operand.as_ref().is_some_and(|e| walk_column_refs(e, f))
                || whens
                    .iter()
                    .any(|w| walk_column_refs(&w.condition, f) || walk_column_refs(&w.result, f))
                || else_expr.as_ref().is_some_and(|e| walk_column_refs(e, f))
        }
        Expr::Between {
            expr, low, high, ..
        } => walk_column_refs(expr, f) || walk_column_refs(low, f) || walk_column_refs(high, f),
        Expr::InList { list, .. } => list.iter().any(|e| walk_column_refs(e, f)),
        Expr::InSubquery { .. } | Expr::Exists(_) | Expr::Subquery(_) => false,
        Expr::TypeCast { expr, .. } => walk_column_refs(expr, f),
        Expr::Treat { expr, .. } => walk_column_refs(expr, f),
        Expr::Array(exprs) => exprs.iter().any(|e| walk_column_refs(e, f)),
        Expr::Subscript {
            object,
            lower,
            upper,
            ..
        } => {
            walk_column_refs(object, f)
                || lower.as_ref().is_some_and(|e| walk_column_refs(e, f))
                || upper.as_ref().is_some_and(|e| walk_column_refs(e, f))
        }
        _ => false,
    }
}

/// Recursively walk `expr` and classify every `ColumnRef` into either a
/// parameter name (single-part, not a known table alias) or a known-table
/// column (multi-part or in aliases). Used by `handle_between` to extract
/// range-bound columns into tier1 when the BETWEEN is parameter-bearing.
fn classify_expr_columns(
    expr: &Expr,
    collector: &EqPredicateCollector,
    params: &mut Vec<String>,
    cols: &mut Vec<ObjectName>,
) {
    match expr {
        Expr::ColumnRef(parts) => {
            if parts.len() == 1 && !collector.is_known_table(parts) {
                if let Some(n) = parts.last() {
                    params.push(n.as_str().to_lowercase());
                }
            } else if collector.is_known_table_or_correlated(parts) {
                cols.push(parts.clone());
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            classify_expr_columns(left, collector, params, cols);
            classify_expr_columns(right, collector, params, cols);
        }
        Expr::UnaryOp { expr, .. } => classify_expr_columns(expr, collector, params, cols),
        Expr::Parenthesized(inner) => classify_expr_columns(inner, collector, params, cols),
        Expr::IsNull { expr, .. } => classify_expr_columns(expr, collector, params, cols),
        Expr::FunctionCall { args, filter, .. } => {
            for a in args {
                classify_expr_columns(a, collector, params, cols);
            }
            if let Some(f) = filter {
                classify_expr_columns(f, collector, params, cols);
            }
        }
        Expr::SpecialFunction { args, .. } => {
            for a in args {
                classify_expr_columns(a, collector, params, cols);
            }
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            classify_expr_columns(expr, collector, params, cols);
            classify_expr_columns(low, collector, params, cols);
            classify_expr_columns(high, collector, params, cols);
        }
        Expr::TypeCast { expr, .. } => classify_expr_columns(expr, collector, params, cols),
        Expr::Treat { expr, .. } => classify_expr_columns(expr, collector, params, cols),
        Expr::Array(exprs) => {
            for e in exprs {
                classify_expr_columns(e, collector, params, cols);
            }
        }
        Expr::Like {
            expr,
            pattern,
            escape,
            ..
        } => {
            classify_expr_columns(expr, collector, params, cols);
            classify_expr_columns(pattern, collector, params, cols);
            if let Some(e) = escape {
                classify_expr_columns(e, collector, params, cols);
            }
        }
        Expr::InList { expr, list, .. } => {
            classify_expr_columns(expr, collector, params, cols);
            for item in list {
                classify_expr_columns(item, collector, params, cols);
            }
        }
        Expr::Case {
            operand,
            whens,
            else_expr,
        } => {
            if let Some(o) = operand {
                classify_expr_columns(o, collector, params, cols);
            }
            for w in whens {
                classify_expr_columns(&w.condition, collector, params, cols);
                classify_expr_columns(&w.result, collector, params, cols);
            }
            if let Some(e) = else_expr {
                classify_expr_columns(e, collector, params, cols);
            }
        }
        Expr::Subscript {
            object,
            lower,
            upper,
            ..
        } => {
            classify_expr_columns(object, collector, params, cols);
            if let Some(l) = lower {
                classify_expr_columns(l, collector, params, cols);
            }
            if let Some(u) = upper {
                classify_expr_columns(u, collector, params, cols);
            }
        }
        _ => {}
    }
}

/// Collect last-identifier names of ColumnRefs that appear in `col = literal`
/// or `literal = col` equalities. In WHERE clauses, these are always data
/// filters on columns, never constant conditions on stored-proc variables.
fn collect_literal_compared_cols(expr: &Expr) -> HashSet<String> {
    let mut cols = HashSet::new();
    fn walk(expr: &Expr, cols: &mut HashSet<String>) {
        match expr {
            Expr::BinaryOp { left, op, right } => match op.to_uppercase().as_str() {
                "=" => match (left.as_ref(), right.as_ref()) {
                    (Expr::ColumnRef(name), Expr::Literal(_))
                    | (Expr::Literal(_), Expr::ColumnRef(name)) => {
                        if let Some(n) = name.last() {
                            cols.insert(n.as_str().to_lowercase());
                        }
                    }
                    _ => {}
                },
                "AND" | "OR" => {
                    walk(left, cols);
                    walk(right, cols);
                }
                _ => {}
            },
            Expr::Parenthesized(inner) => walk(inner, cols),
            _ => {}
        }
    }
    walk(expr, &mut cols);
    cols
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
        Expr::SpecialFunction { args, .. } => args.iter().any(contains_param),
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
