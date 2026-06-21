//! Rule: convert `NVL(a, b)` to `CASE WHEN a IS NULL THEN b ELSE a END`.
//!
//! Safe level: the rewrite is semantically equivalent. NVL is an Oracle /
//! openGauss-specific function; CASE WHEN is standard SQL and allows the
//! optimizer to use index access paths on `a`.

use metamorphosis_core::types::{MatchResult, RewriteAction, RuleCategory, SafetyLevel};
use metamorphosis_core::{RewriteContext, RewriteRule};
use ogsql_parser::ast::{Expr, SelectTarget, Spanned, Statement, WhenClause};
use tracing::debug;

/// Rule: rewrite NVL(a, b) to CASE WHEN a IS NULL THEN b ELSE a END.
///
/// Safe level: semantically equivalent.
#[derive(Debug)]
pub struct NvlToCase;

impl RewriteRule for NvlToCase {
    fn id(&self) -> &'static str {
        "nvl-to-case"
    }

    fn description(&self) -> &'static str {
        "Rewrite NVL(a, b) to CASE WHEN a IS NULL THEN b ELSE a END"
    }

    fn category(&self) -> RuleCategory {
        RuleCategory::Semantic
    }

    fn safety_level(&self) -> SafetyLevel {
        SafetyLevel::Safe
    }

    fn matches(&self, _ctx: &RewriteContext, stmt: &Statement) -> MatchResult {
        let select = match stmt {
            Statement::Select(s) => &s.node,
            _ => {
                return MatchResult::NotMatched {
                    reason: "Not a SELECT statement".to_string(),
                };
            }
        };

        for target in &select.targets {
            if let SelectTarget::Expr(expr, _) = target {
                if find_nvl(expr) {
                    return MatchResult::Matched;
                }
            }
        }

        if let Some(ref where_clause) = select.where_clause {
            if find_nvl(where_clause) {
                return MatchResult::Matched;
            }
        }

        MatchResult::NotMatched {
            reason: "No NVL function call found in SELECT targets or WHERE clause".to_string(),
        }
    }

    fn apply(&self, _ctx: &RewriteContext, stmt: &Statement) -> Vec<RewriteAction> {
        let spanned = match stmt {
            Statement::Select(s) => s,
            _ => return vec![],
        };

        let mut new_select = spanned.node.clone();
        let mut replaced = false;

        for target in &mut new_select.targets {
            if let SelectTarget::Expr(expr, _) = target {
                if let Some(new_expr) = replace_first_nvl(expr) {
                    *expr = new_expr;
                    replaced = true;
                    break;
                }
            }
        }

        if !replaced {
            if let Some(ref where_expr) = new_select.where_clause {
                if let Some(new_where) = replace_first_nvl(where_expr) {
                    new_select.where_clause = Some(new_where);
                    replaced = true;
                }
            }
        }

        if !replaced {
            return vec![];
        }

        debug!("Replaced NVL(a, b) with CASE WHEN a IS NULL THEN b ELSE a END");

        vec![RewriteAction::Replace(Box::new(Statement::Select(
            Spanned::without_span(new_select),
        )))]
    }
}

fn is_nvl_call(expr: &Expr) -> bool {
    if let Expr::FunctionCall { name, args, .. } = expr {
        args.len() == 2
            && name
                .first()
                .map(|i| i.value.eq_ignore_ascii_case("nvl"))
                .unwrap_or(false)
    } else {
        false
    }
}

fn find_nvl(expr: &Expr) -> bool {
    if is_nvl_call(expr) {
        return true;
    }
    match expr {
        Expr::BinaryOp { left, right, .. } => find_nvl(left) || find_nvl(right),
        Expr::UnaryOp { expr: inner, .. } => find_nvl(inner),
        Expr::Parenthesized(inner) => find_nvl(inner),
        Expr::IsNull { expr: inner, .. } => find_nvl(inner),
        Expr::FunctionCall { args, .. } => args.iter().any(find_nvl),
        Expr::Case {
            operand,
            whens,
            else_expr,
        } => {
            operand.as_ref().map(|o| find_nvl(o)).unwrap_or(false)
                || whens.iter().any(|w| find_nvl(&w.condition) || find_nvl(&w.result))
                || else_expr.as_ref().map(|e| find_nvl(e)).unwrap_or(false)
        }
        Expr::Between {
            expr, low, high, ..
        } => find_nvl(expr) || find_nvl(low) || find_nvl(high),
        Expr::InList { expr, list, .. } => find_nvl(expr) || list.iter().any(find_nvl),
        Expr::Like { expr, pattern, .. } => find_nvl(expr) || find_nvl(pattern),
        Expr::TypeCast { expr, .. } => find_nvl(expr),
        _ => false,
    }
}

fn replace_first_nvl(expr: &Expr) -> Option<Expr> {
    if is_nvl_call(expr) {
        if let Expr::FunctionCall { args, .. } = expr {
            let a = args[0].clone();
            let b = args[1].clone();
            return Some(Expr::Case {
                operand: None,
                whens: vec![WhenClause {
                    condition: Expr::IsNull {
                        expr: Box::new(a.clone()),
                        negated: false,
                    },
                    result: b,
                }],
                else_expr: Some(Box::new(a)),
            });
        }
    }

    match expr {
        Expr::BinaryOp {
            left,
            op,
            right,
        } => {
            if let Some(new_left) = replace_first_nvl(left) {
                Some(Expr::BinaryOp {
                    left: Box::new(new_left),
                    op: op.clone(),
                    right: right.clone(),
                })
            } else if let Some(new_right) = replace_first_nvl(right) {
                Some(Expr::BinaryOp {
                    left: left.clone(),
                    op: op.clone(),
                    right: Box::new(new_right),
                })
            } else {
                None
            }
        }
        Expr::UnaryOp { op, expr: inner } => {
            replace_first_nvl(inner).map(|r| Expr::UnaryOp {
                op: op.clone(),
                expr: Box::new(r),
            })
        }
        Expr::Parenthesized(inner) => {
            replace_first_nvl(inner).map(|r| Expr::Parenthesized(Box::new(r)))
        }
        Expr::IsNull { expr: inner, negated } => {
            replace_first_nvl(inner).map(|r| Expr::IsNull {
                expr: Box::new(r),
                negated: *negated,
            })
        }
        Expr::FunctionCall {
            name,
            args,
            distinct,
            over,
            filter,
            within_group,
            separator,
            default,
            conversion_format,
            agg_from,
            builtin,
        } => {
            for (i, arg) in args.iter().enumerate() {
                if let Some(new_arg) = replace_first_nvl(arg) {
                    let mut new_args = args.clone();
                    new_args[i] = new_arg;
                    return Some(Expr::FunctionCall {
                        name: name.clone(),
                        args: new_args,
                        distinct: *distinct,
                        over: over.clone(),
                        filter: filter.clone(),
                        within_group: within_group.clone(),
                        separator: separator.clone(),
                        default: default.clone(),
                        conversion_format: conversion_format.clone(),
                        agg_from: agg_from.clone(),
                        builtin: builtin.clone(),
                    });
                }
            }
            None
        }
        Expr::TypeCast {
            expr: inner,
            type_name,
            default,
            format,
        } => replace_first_nvl(inner).map(|r| Expr::TypeCast {
            expr: Box::new(r),
            type_name: type_name.clone(),
            default: default.clone(),
            format: format.clone(),
        }),
        Expr::Case {
            operand,
            whens,
            else_expr,
        } => {
            if let Some(op) = operand {
                if let Some(new_op) = replace_first_nvl(op) {
                    return Some(Expr::Case {
                        operand: Some(Box::new(new_op)),
                        whens: whens.clone(),
                        else_expr: else_expr.clone(),
                    });
                }
            }
            for (i, when) in whens.iter().enumerate() {
                if let Some(new_cond) = replace_first_nvl(&when.condition) {
                    let mut new_whens = whens.clone();
                    new_whens[i].condition = new_cond;
                    return Some(Expr::Case {
                        operand: operand.clone(),
                        whens: new_whens,
                        else_expr: else_expr.clone(),
                    });
                }
                if let Some(new_result) = replace_first_nvl(&when.result) {
                    let mut new_whens = whens.clone();
                    new_whens[i].result = new_result;
                    return Some(Expr::Case {
                        operand: operand.clone(),
                        whens: new_whens,
                        else_expr: else_expr.clone(),
                    });
                }
            }
            if let Some(ee) = else_expr {
                if let Some(new_ee) = replace_first_nvl(ee) {
                    return Some(Expr::Case {
                        operand: operand.clone(),
                        whens: whens.clone(),
                        else_expr: Some(Box::new(new_ee)),
                    });
                }
            }
            None
        }
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => {
            if let Some(new_expr) = replace_first_nvl(expr) {
                Some(Expr::Between {
                    expr: Box::new(new_expr),
                    low: low.clone(),
                    high: high.clone(),
                    negated: *negated,
                })
            } else if let Some(new_low) = replace_first_nvl(low) {
                Some(Expr::Between {
                    expr: expr.clone(),
                    low: Box::new(new_low),
                    high: high.clone(),
                    negated: *negated,
                })
            } else {
                replace_first_nvl(high).map(|new_high| Expr::Between {
                    expr: expr.clone(),
                    low: low.clone(),
                    high: Box::new(new_high),
                    negated: *negated,
                })
            }
        }
        Expr::InList { expr, list, negated } => {
            if let Some(new_expr) = replace_first_nvl(expr) {
                Some(Expr::InList {
                    expr: Box::new(new_expr),
                    list: list.clone(),
                    negated: *negated,
                })
            } else {
                for (i, item) in list.iter().enumerate() {
                    if let Some(new_item) = replace_first_nvl(item) {
                        let mut new_list = list.clone();
                        new_list[i] = new_item;
                        return Some(Expr::InList {
                            expr: expr.clone(),
                            list: new_list,
                            negated: *negated,
                        });
                    }
                }
                None
            }
        }
        Expr::Like {
            expr,
            pattern,
            escape,
            negated,
            case_insensitive,
        } => {
            if let Some(new_expr) = replace_first_nvl(expr) {
                Some(Expr::Like {
                    expr: Box::new(new_expr),
                    pattern: pattern.clone(),
                    escape: escape.clone(),
                    negated: *negated,
                    case_insensitive: *case_insensitive,
                })
            } else {
                replace_first_nvl(pattern).map(|new_pattern| Expr::Like {
                    expr: expr.clone(),
                    pattern: Box::new(new_pattern),
                    escape: escape.clone(),
                    negated: *negated,
                    case_insensitive: *case_insensitive,
                })
            }
        }
        _ => None,
    }
}
