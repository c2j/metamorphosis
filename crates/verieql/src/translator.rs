use std::collections::HashMap;

use ogsql_parser::ast::{
    Expr as OExpr, JoinType as OJoinType, SelectStatement, SelectTarget, SetOperation, Statement,
    TableRef, WhenClause, WithClause,
};

use crate::ir::*;

#[derive(Debug, thiserror::Error)]
pub enum TranslateError {
    #[error("unsupported statement type (only SELECT is supported)")]
    UnsupportedStatement,
    #[error("unsupported expression: {0}")]
    UnsupportedExpr(String),
    #[error("unsupported join type")]
    UnsupportedJoin,
    #[error("parse error: {0}")]
    ParseError(String),
}

/// Translate an ogsql-parser Statement into VeriEql IR.
pub fn translate(stmt: &Statement) -> Result<Relation, TranslateError> {
    match stmt {
        Statement::Select(s) => translate_select_with_ctes(s, &CteMap::new()),
        _ => Err(TranslateError::UnsupportedStatement),
    }
}

fn translate_select(s: &SelectStatement) -> Result<Relation, TranslateError> {
    translate_select_with_ctes(s, &CteMap::new())
}

type CteMap = HashMap<String, Relation>;

fn translate_select_with_ctes(
    s: &SelectStatement,
    outer_ctes: &CteMap,
) -> Result<Relation, TranslateError> {
    let cte_map = {
        let mut map = outer_ctes.clone();
        if let Some(ref wc) = s.with {
            for cte in &wc.ctes {
                let rel = translate_select_inner(&cte.query, &map)?;
                map.insert(cte.name.to_lowercase(), rel);
            }
        }
        map
    };

    translate_select_inner(s, &cte_map)
}

fn translate_select_inner(
    s: &SelectStatement,
    cte_map: &CteMap,
) -> Result<Relation, TranslateError> {
    let mut rel = translate_from(&s.from, cte_map)?;

    if let Some(ref cond) = s.where_clause {
        rel = Relation::Filter {
            input: Box::new(rel),
            predicate: translate_expr(cond)?,
        };
    }

    if !s.group_by.is_empty() {
        rel = translate_group_by(&s.group_by, &s.targets, s.having.as_ref(), &rel)?;
    }

    rel = translate_targets(&s.targets, rel, s.distinct)?;

    if !s.order_by.is_empty() {
        let items: Result<Vec<_>, _> = s
            .order_by
            .iter()
            .map(|o| {
                Ok(OrderByItem {
                    expr: translate_expr(&o.expr)?,
                    asc: o.asc.unwrap_or(true),
                    nulls_first: o.nulls_first,
                })
            })
            .collect();
        rel = Relation::OrderBy {
            input: Box::new(rel),
            items: items?,
            limit: s.limit.as_ref().map(translate_expr).transpose()?,
            offset: s.offset.as_ref().map(translate_expr).transpose()?,
        };
    }

    if let Some(ref set_op) = s.set_operation {
        rel = translate_set_op(set_op, rel, cte_map)?;
    }

    Ok(rel)
}

fn translate_from(from: &[TableRef], cte_map: &CteMap) -> Result<Relation, TranslateError> {
    if from.is_empty() {
        return Ok(Relation::Empty);
    }

    let mut rels: Vec<Relation> = Vec::new();
    for tref in from {
        rels.push(translate_table_ref(tref, cte_map)?);
    }

    if rels.len() == 1 {
        return Ok(rels.pop().unwrap());
    }

    let mut result = rels.remove(0);
    for r in rels {
        result = Relation::Join {
            left: Box::new(result),
            right: Box::new(r),
            join_type: JoinType::Cross,
            condition: None,
        };
    }
    Ok(result)
}

fn translate_table_ref(tref: &TableRef, cte_map: &CteMap) -> Result<Relation, TranslateError> {
    match tref {
        TableRef::Table { name, .. } => {
            let table_name = name.join(".");
            if let Some(cte_rel) = cte_map.get(&table_name.to_lowercase()) {
                return Ok(cte_rel.clone());
            }
            Ok(Relation::BaseTable {
                name: table_name.clone(),
                columns: Vec::new(),
                tuple_count: 0,
            })
        }
        TableRef::Subquery { query, .. } => translate_select_with_ctes(query, cte_map),
        TableRef::Join {
            left,
            right,
            join_type,
            condition,
            natural,
            using_columns,
            ..
        } => {
            let l = translate_table_ref(left, cte_map)?;
            let r = translate_table_ref(right, cte_map)?;
            let jt = match join_type {
                OJoinType::Inner => JoinType::Inner,
                OJoinType::Left => JoinType::Left,
                OJoinType::Right => JoinType::Right,
                OJoinType::Full => JoinType::Full,
                OJoinType::Cross => JoinType::Cross,
            };

            let cond = if *natural && !using_columns.is_empty() {
                Some(build_using_condition(using_columns)?)
            } else if *natural {
                condition.as_ref().map(translate_expr).transpose()?
            } else if !using_columns.is_empty() {
                Some(build_using_condition(using_columns)?)
            } else {
                condition.as_ref().map(translate_expr).transpose()?
            };

            Ok(Relation::Join {
                left: Box::new(l),
                right: Box::new(r),
                join_type: jt,
                condition: cond,
            })
        }
        TableRef::Values { .. } => Ok(Relation::Values { rows: Vec::new() }),
        _ => Err(TranslateError::UnsupportedJoin),
    }
}

fn build_using_condition(columns: &[String]) -> Result<Expr, TranslateError> {
    let mut parts: Vec<Expr> = Vec::new();
    for col in columns {
        parts.push(Expr::BinaryOp {
            op: BinOp::Eq,
            left: Box::new(Expr::ColumnRef {
                table: None,
                column: col.clone(),
            }),
            right: Box::new(Expr::ColumnRef {
                table: None,
                column: col.clone(),
            }),
        });
    }
    if parts.is_empty() {
        return Err(TranslateError::UnsupportedExpr(
            "NATURAL/USING join with no columns".into(),
        ));
    }
    if parts.len() == 1 {
        return Ok(parts.pop().unwrap());
    }
    let mut result = parts.remove(0);
    for p in parts {
        result = Expr::BinaryOp {
            op: BinOp::And,
            left: Box::new(result),
            right: Box::new(p),
        };
    }
    Ok(result)
}

fn translate_targets(
    targets: &[SelectTarget],
    input: Relation,
    distinct: bool,
) -> Result<Relation, TranslateError> {
    let exprs: Result<Vec<ProjectExpr>, _> = targets
        .iter()
        .map(|t| match t {
            SelectTarget::Expr(e, _alias) => {
                let ir_expr = translate_expr(e)?;
                if is_aggregate_expr(e) {
                    Ok(ProjectExpr::Aggregate(AggregateExpr {
                        func: extract_agg_func(e),
                        arg: extract_agg_arg(e),
                        distinct: false,
                        alias: None,
                    }))
                } else {
                    Ok(ProjectExpr::Column(ir_expr))
                }
            }
            SelectTarget::Star(_) => Ok(ProjectExpr::Column(Expr::Star)),
        })
        .collect();

    Ok(Relation::Project {
        input: Box::new(input),
        exprs: exprs?,
        distinct,
    })
}

fn translate_group_by(
    group_by: &[ogsql_parser::ast::GroupByItem],
    _targets: &[SelectTarget],
    having: Option<&OExpr>,
    input: &Relation,
) -> Result<Relation, TranslateError> {
    let keys: Result<Vec<Expr>, _> = group_by
        .iter()
        .map(|g| match g {
            ogsql_parser::ast::GroupByItem::Expr(e) => translate_expr(e),
            _ => Err(TranslateError::UnsupportedExpr(
                "GROUPING SETS/ROLLUP/CUBE".into(),
            )),
        })
        .collect();

    Ok(Relation::GroupBy {
        input: Box::new(input.clone()),
        keys: keys?,
        aggregates: Vec::new(),
        having: having.map(translate_expr).transpose()?,
    })
}

fn translate_set_op(
    op: &SetOperation,
    left: Relation,
    cte_map: &CteMap,
) -> Result<Relation, TranslateError> {
    match op {
        SetOperation::Union { all, right } => {
            let r = translate_select_inner(right, cte_map)?;
            Ok(Relation::Union {
                left: Box::new(left),
                right: Box::new(r),
                all: *all,
            })
        }
        SetOperation::Intersect { all, right } => {
            let r = translate_select_inner(right, cte_map)?;
            Ok(Relation::Intersect {
                left: Box::new(left),
                right: Box::new(r),
                all: *all,
            })
        }
        SetOperation::Except { all, right } => {
            let r = translate_select_inner(right, cte_map)?;
            Ok(Relation::Except {
                left: Box::new(left),
                right: Box::new(r),
                all: *all,
            })
        }
    }
}

fn translate_expr(expr: &OExpr) -> Result<Expr, TranslateError> {
    match expr {
        OExpr::Literal(lit) => {
            use ogsql_parser::ast::Literal;
            match lit {
                Literal::Integer(v) => Ok(Expr::Literal(ExprValue::Integer(*v))),
                Literal::Float(v) => Ok(v
                    .parse::<f64>()
                    .map(|f| Expr::Literal(ExprValue::Float(f)))
                    .unwrap_or_else(|_| Expr::Literal(ExprValue::Integer(0)))),
                Literal::String(v) => Ok(Expr::Literal(ExprValue::String(v.clone()))),
                Literal::Boolean(v) => Ok(Expr::Literal(ExprValue::Boolean(*v))),
                Literal::Null => Ok(Expr::SqlNull),
                _ => Ok(Expr::Literal(ExprValue::Integer(0))),
            }
        }
        OExpr::ColumnRef(name) => {
            if name.len() == 2 {
                Ok(Expr::ColumnRef {
                    table: Some(name[0].clone()),
                    column: name[1].clone(),
                })
            } else if name.len() == 1 {
                Ok(Expr::ColumnRef {
                    table: None,
                    column: name[0].clone(),
                })
            } else {
                Ok(Expr::ColumnRef {
                    table: None,
                    column: name.join("."),
                })
            }
        }
        OExpr::BinaryOp { left, op, right } => {
            let l = translate_expr(left)?;
            let r = translate_expr(right)?;
            let binop = match op.to_uppercase().as_str() {
                "+" => BinOp::Add,
                "-" => BinOp::Sub,
                "*" => BinOp::Mul,
                "/" => BinOp::Div,
                "%" => BinOp::Mod,
                "=" => BinOp::Eq,
                "!=" | "<>" => BinOp::Neq,
                "<" => BinOp::Lt,
                ">" => BinOp::Gt,
                "<=" => BinOp::Lte,
                ">=" => BinOp::Gte,
                "AND" => BinOp::And,
                "OR" => BinOp::Or,
                "||" => BinOp::Concat,
                _ => return Err(TranslateError::UnsupportedExpr(format!("binary op: {op}"))),
            };
            Ok(Expr::BinaryOp {
                op: binop,
                left: Box::new(l),
                right: Box::new(r),
            })
        }
        OExpr::UnaryOp { op, expr: inner } => {
            let e = translate_expr(inner)?;
            let uop = match op.to_uppercase().as_str() {
                "NOT" | "!" => UnaryOp::Not,
                "-" => UnaryOp::Neg,
                _ => return Err(TranslateError::UnsupportedExpr(format!("unary op: {op}"))),
            };
            Ok(Expr::UnaryOp {
                op: uop,
                expr: Box::new(e),
            })
        }
        OExpr::Case {
            operand,
            whens,
            else_expr,
        } => {
            let op = operand
                .as_ref()
                .map(|e| translate_expr(e).map(Box::new))
                .transpose()?;
            let ws: Result<Vec<(Expr, Expr)>, _> = whens
                .iter()
                .map(|w: &WhenClause| {
                    let cond = translate_expr(&w.condition)?;
                    let result = translate_expr(&w.result)?;
                    Ok((cond, result))
                })
                .collect();
            let el = else_expr
                .as_ref()
                .map(|e| translate_expr(e).map(Box::new))
                .transpose()?;
            Ok(Expr::Case {
                operand: op,
                whens: ws?,
                else_expr: el,
            })
        }
        OExpr::IsNull {
            expr: inner,
            negated,
        } => Ok(Expr::IsNull {
            expr: Box::new(translate_expr(inner)?),
            negated: *negated,
        }),
        OExpr::InList {
            expr: inner,
            list,
            negated,
        } => {
            let e = translate_expr(inner)?;
            let ls: Result<Vec<_>, _> = list.iter().map(translate_expr).collect();
            Ok(Expr::InList {
                expr: Box::new(e),
                list: ls?,
                negated: *negated,
            })
        }
        OExpr::InSubquery {
            expr: inner,
            subquery,
            negated,
        } => {
            let e = translate_expr(inner)?;
            let sq = translate_select(subquery)?;
            Ok(Expr::InSubquery {
                expr: Box::new(e),
                subquery: Box::new(sq),
                negated: *negated,
            })
        }
        OExpr::Exists(sq) => Ok(Expr::Exists(Box::new(translate_select(sq)?))),
        OExpr::Subquery(sq) => Ok(Expr::ScalarSubquery(Box::new(translate_select(sq)?))),
        OExpr::Between {
            expr: inner,
            low,
            high,
            negated,
        } => Ok(Expr::Between {
            expr: Box::new(translate_expr(inner)?),
            low: Box::new(translate_expr(low)?),
            high: Box::new(translate_expr(high)?),
            negated: *negated,
        }),
        OExpr::Like {
            expr: inner,
            pattern,
            negated,
            ..
        } => Ok(Expr::Like {
            expr: Box::new(translate_expr(inner)?),
            pattern: Box::new(translate_expr(pattern)?),
            negated: *negated,
        }),
        OExpr::FunctionCall { name, args, .. } => {
            let fn_name = name.join(".");
            let translated_args: Result<Vec<_>, _> = args.iter().map(translate_expr).collect();
            Ok(Expr::FunctionCall {
                name: fn_name,
                args: translated_args?,
            })
        }
        OExpr::Parenthesized(inner) => translate_expr(inner),
        OExpr::TypeCast { expr: inner, .. } => translate_expr(inner),
        _ => Err(TranslateError::UnsupportedExpr(format!("{:?}", expr))),
    }
}

fn is_aggregate_expr(expr: &OExpr) -> bool {
    match expr {
        OExpr::FunctionCall { name, .. } => {
            let fn_name = name.last().map(|s| s.to_uppercase()).unwrap_or_default();
            matches!(fn_name.as_str(), "COUNT" | "SUM" | "AVG" | "MIN" | "MAX")
        }
        _ => false,
    }
}

fn extract_agg_func(expr: &OExpr) -> AggFunc {
    if let OExpr::FunctionCall { name, .. } = expr {
        match name
            .last()
            .map(|s| s.to_uppercase())
            .unwrap_or_default()
            .as_str()
        {
            "COUNT" => AggFunc::Count,
            "SUM" => AggFunc::Sum,
            "AVG" => AggFunc::Avg,
            "MIN" => AggFunc::Min,
            "MAX" => AggFunc::Max,
            _ => AggFunc::Count,
        }
    } else {
        AggFunc::Count
    }
}

fn extract_agg_arg(expr: &OExpr) -> Option<Expr> {
    if let OExpr::FunctionCall { args, name, .. } = expr {
        let fn_name = name.last().map(|s| s.to_uppercase()).unwrap_or_default();
        if fn_name == "COUNT" && args.is_empty() {
            return None;
        }
        args.first().and_then(|a| translate_expr(a).ok())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_and_translate(sql: &str) -> Result<Relation, TranslateError> {
        let tokens = ogsql_parser::Tokenizer::new(sql).tokenize().unwrap();
        let stmts = ogsql_parser::parser::Parser::new(tokens).parse();
        let stmt = stmts.into_iter().next().unwrap();
        translate(&stmt)
    }

    #[test]
    fn test_natural_join() {
        let sql = "SELECT a FROM t1 NATURAL JOIN t2";
        let result = parse_and_translate(sql);
        assert!(
            result.is_ok(),
            "NATURAL JOIN should parse+translate: {:?}",
            result
        );
    }

    #[test]
    fn test_natural_join_chain() {
        let sql = "SELECT x FROM a NATURAL JOIN b NATURAL JOIN c NATURAL JOIN d";
        let result = parse_and_translate(sql);
        assert!(
            result.is_ok(),
            "chained NATURAL JOIN should parse+translate: {:?}",
            result
        );
    }

    #[test]
    fn test_cte() {
        let sql = "WITH cte AS (SELECT id FROM users) SELECT * FROM cte";
        let result = parse_and_translate(sql);
        assert!(result.is_ok(), "CTE should parse+translate: {:?}", result);
    }

    #[test]
    fn test_cte_with_natural_join() {
        let sql = "WITH s AS (SELECT id FROM takes NATURAL JOIN section) SELECT DISTINCT id FROM s NATURAL JOIN student";
        let result = parse_and_translate(sql);
        assert!(
            result.is_ok(),
            "CTE + NATURAL JOIN should parse+translate: {:?}",
            result
        );
    }

    #[test]
    fn test_cast() {
        let sql = "SELECT CAST(x AS INTEGER) FROM t";
        let result = parse_and_translate(sql);
        assert!(result.is_ok(), "CAST should parse+translate: {:?}", result);
    }

    #[test]
    fn test_coalesce() {
        let sql = "SELECT COALESCE(a, b, 0) FROM t";
        let result = parse_and_translate(sql);
        assert!(
            result.is_ok(),
            "COALESCE should parse+translate: {:?}",
            result
        );
    }

    #[test]
    fn test_float_literal() {
        let sql = "SELECT a FROM t WHERE x > 3.14";
        let result = parse_and_translate(sql);
        assert!(
            result.is_ok(),
            "float literal should parse+translate: {:?}",
            result
        );
    }

    #[test]
    fn test_cast_in_where() {
        let sql = "SELECT a FROM t WHERE CAST(x AS INT) > 0";
        let result = parse_and_translate(sql);
        assert!(
            result.is_ok(),
            "CAST in WHERE should parse+translate: {:?}",
            result
        );
    }

    #[test]
    fn test_coalesce_in_where() {
        let sql = "SELECT a FROM t WHERE COALESCE(x, 0) = 1";
        let result = parse_and_translate(sql);
        assert!(
            result.is_ok(),
            "COALESCE in WHERE should parse+translate: {:?}",
            result
        );
    }

    #[test]
    fn test_benchmark_ca6() {
        let sql = "SELECT STUDENT.ID FROM COURSE NATURAL JOIN DEPARTMENT NATURAL JOIN STUDENT NATURAL JOIN TAKES NATURAL JOIN SECTION GROUP BY STUDENT.ID,STUDENT.DEPT_NAME HAVING COUNT(DEPT_NAME) > 1";
        let result = parse_and_translate(sql);
        assert!(
            result.is_ok(),
            "CA6 benchmark should parse+translate: {:?}",
            result
        );
    }

    #[test]
    fn test_benchmark_cq9() {
        let sql = "WITH S AS (SELECT ID, TIME_SLOT_ID, YEAR FROM TAKES NATURAL JOIN SECTION GROUP BY ID, TIME_SLOT_ID, YEAR HAVING COUNT(TIME_SLOT_ID)>4) SELECT DISTINCT ID,NAME FROM S NATURAL JOIN STUDENT";
        let result = parse_and_translate(sql);
        assert!(
            result.is_ok(),
            "CQ9 benchmark should parse+translate: {:?}",
            result
        );
    }
}
