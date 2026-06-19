use metamorphosis_core::types::{MatchResult, RewriteAction, RuleCategory, SafetyLevel};
use metamorphosis_core::{RewriteContext, RewriteRule};
use ogsql_parser::ast::{Ident, SelectTarget, Spanned};
use ogsql_parser::{Expr, ObjectName, Statement, TableRef};
use tracing::debug;

/// Rule: replace `SELECT *` (or `SELECT t.*`) with explicit column names
/// from the schema map.
///
/// Safety: Safe (semantically equivalent when schema is accurate).
#[derive(Debug)]
pub struct EliminateSelectStar;

impl RewriteRule for EliminateSelectStar {
    fn id(&self) -> &'static str {
        "eliminate-select-star"
    }

    fn description(&self) -> &'static str {
        "Replace SELECT * with explicit column names using schema metadata"
    }

    fn category(&self) -> RuleCategory {
        RuleCategory::Semantic
    }

    fn safety_level(&self) -> SafetyLevel {
        SafetyLevel::Safe
    }

    fn matches(&self, ctx: &RewriteContext, stmt: &Statement) -> MatchResult {
        if ctx.schema.is_none() {
            return MatchResult::NotMatched {
                reason: "No schema map provided; cannot resolve column names for expansion"
                    .to_string(),
            };
        }

        match stmt {
            Statement::Select(spanned) => {
                if has_wildcard_target(&spanned.targets) {
                    MatchResult::Matched
                } else {
                    MatchResult::NotMatched {
                        reason: "No wildcard target (SELECT *) found in target list".to_string(),
                    }
                }
            }
            _ => MatchResult::NotMatched {
                reason: format!("Statement is {} (not SELECT)", stmt_type_label(stmt)),
            },
        }
    }

    fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Vec<RewriteAction> {
        let schema = match ctx.schema {
            Some(ref s) => s,
            None => return vec![],
        };
        let spanned = match stmt {
            Statement::Select(s) => s,
            _ => return vec![],
        };

        let select = &spanned.node;
        if !has_wildcard_target(&select.targets) {
            return vec![];
        }

        let (table_name, _alias) = match resolve_base_table(&select.from) {
            Some(v) => v,
            None => return vec![],
        };
        let table_key = table_name.join(".").to_lowercase();
        let columns = match schema.get(&table_key) {
            Some(c) => c,
            None => return vec![],
        };

        debug!(
            table = %table_key,
            column_count = columns.len(),
            "Expanding SELECT *"
        );

        let mut new_targets: Vec<SelectTarget> = Vec::with_capacity(select.targets.len());
        for target in &select.targets {
            match target {
                SelectTarget::Star(prefix) => {
                    for col_name in columns.keys() {
                        let column_ref = if let Some(p) = prefix {
                            Expr::ColumnRef(vec![p.clone(), col_name.clone().into()])
                        } else {
                            Expr::ColumnRef(vec![col_name.clone().into()])
                        };
                        new_targets.push(SelectTarget::Expr(column_ref, None));
                    }
                }
                other => new_targets.push(other.clone()),
            }
        }

        let mut new_select = select.clone();
        new_select.targets = new_targets;

        vec![RewriteAction::Replace(Box::new(Statement::Select(
            Spanned::without_span(new_select),
        )))]
    }
}

/// Check if any target is a wildcard (including qualified wildcards like `t.*`).
fn has_wildcard_target(targets: &[SelectTarget]) -> bool {
    targets.iter().any(|t| matches!(t, SelectTarget::Star(_)))
}

fn stmt_type_label(stmt: &Statement) -> &'static str {
    match stmt {
        Statement::Select(_) => "SELECT",
        Statement::Insert(_) => "INSERT",
        Statement::Update(_) => "UPDATE",
        Statement::Delete(_) => "DELETE",
        Statement::CreateTable(_) => "CREATE TABLE",
        Statement::Drop(_) => "DROP",
        Statement::AlterTable(_) => "ALTER TABLE",
        _ => "non-SELECT",
    }
}

/// Resolve the first base table from the FROM clause, skipping subqueries/joins.
fn resolve_base_table(from: &[TableRef]) -> Option<(&ObjectName, &Option<Ident>)> {
    from.iter().find_map(|tr| match tr {
        TableRef::Table { name, alias, .. } => Some((name, alias)),
        _ => None,
    })
}
