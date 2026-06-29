use z3::ast::{Ast, Bool, Dynamic, Int};

use crate::environment::Environment;
use crate::ir::*;

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("unknown column: {0}")]
    UnknownColumn(String),
    #[error("type mismatch in Z3 encoding")]
    TypeMismatch,
    #[error("unsupported relation: {0}")]
    UnsupportedRelation(String),
    #[error("unsupported expression: {0}")]
    UnsupportedExpr(String),
}

/// Encode a relational IR node as a Z3 membership predicate for `output_tuple`.
pub fn encode_relation_for_tuple(
    rel: &Relation,
    output_tuple: &Dynamic,
    env: &Environment,
) -> Result<Bool, EncodeError> {
    match rel {
        Relation::BaseTable { .. } => Ok(Bool::from_bool(true)),
        Relation::Filter { input, predicate } => {
            let inner = encode_relation_for_tuple(input, output_tuple, env)?;
            let cond = encode_expr_bool(predicate, output_tuple, env)?;
            Ok(Bool::and(&[&inner, &cond]))
        }
        Relation::Project {
            input,
            distinct,
            ..
        } => {
            if *distinct {
                return Err(EncodeError::UnsupportedRelation(
                    "Project { distinct: true } not yet supported".into(),
                ));
            }
            encode_relation_for_tuple(input, output_tuple, env)
        }
        Relation::Join {
            left,
            right,
            condition,
            ..
        } => {
            let l = encode_relation_for_tuple(left, output_tuple, env)?;
            let r = encode_relation_for_tuple(right, output_tuple, env)?;
            let mut parts = vec![l, r];
            if let Some(cond) = condition {
                parts.push(encode_expr_bool(cond, output_tuple, env)?);
            }
            Ok(Bool::and(&parts))
        }
        Relation::Union { left, right, .. } => {
            let l = encode_relation_for_tuple(left, output_tuple, env)?;
            let r = encode_relation_for_tuple(right, output_tuple, env)?;
            Ok(Bool::or(&[&l, &r]))
        }
        Relation::Empty => Ok(Bool::from_bool(false)),
        _ => Err(EncodeError::UnsupportedRelation(format!("{:?}", rel))),
    }
}

/// Encode an IR expression as a Z3 integer-typed AST.
pub fn encode_expr_int(
    expr: &Expr,
    tuple: &Dynamic,
    env: &Environment,
) -> Result<Int, EncodeError> {
    match expr {
        Expr::ColumnRef { table, column } => {
            let key = env.attr_key(table.as_deref(), column);
            let func = env
                .attr_funcs
                .get(&key)
                .ok_or_else(|| EncodeError::UnknownColumn(key.clone()))?;
            let args: Vec<&dyn Ast> = vec![tuple];
            func.apply(&args).as_int().ok_or(EncodeError::TypeMismatch)
        }
        Expr::Literal(ExprValue::Integer(v)) => Ok(Int::from_i64(*v)),
        Expr::Literal(ExprValue::Boolean(v)) => Ok(Int::from_i64(if *v { 1 } else { 0 })),
        Expr::BinaryOp { op, left, right } => {
            let l = encode_expr_int(left, tuple, env)?;
            let r = encode_expr_int(right, tuple, env)?;
            match op {
                BinOp::Add => Ok(Int::add(&[&l, &r])),
                BinOp::Sub => Ok(Int::sub(&[&Int::from_i64(0), &l, &r])),
                BinOp::Mul => Ok(Int::mul(&[&l, &r])),
                _ => {
                    let b = encode_expr_bool(expr, tuple, env)?;
                    Ok(Bool::ite(&b, &Int::from_i64(1), &Int::from_i64(0)))
                }
            }
        }
        Expr::SqlNull => Ok(Int::fresh_const("SQL_NULL")),
        Expr::UnaryOp {
            op: UnaryOp::Neg,
            expr: inner,
        } => {
            let v = encode_expr_int(inner, tuple, env)?;
            Ok(Int::sub(&[&Int::from_i64(0), &v]))
        }
        _ => Err(EncodeError::UnsupportedExpr(format!(
            "encode_expr_int: unsupported expression: {:?}",
            expr
        ))),
    }
}

/// Encode an IR expression as a Z3 boolean-typed AST.
pub fn encode_expr_bool(
    expr: &Expr,
    tuple: &Dynamic,
    env: &Environment,
) -> Result<Bool, EncodeError> {
    match expr {
        Expr::BinaryOp { op, left, right } => match op {
            BinOp::Eq => {
                let l = encode_expr_int(left, tuple, env)?;
                let r = encode_expr_int(right, tuple, env)?;
                Ok(l.eq(&r))
            }
            BinOp::Neq => {
                let l = encode_expr_int(left, tuple, env)?;
                let r = encode_expr_int(right, tuple, env)?;
                Ok(l.eq(&r).not())
            }
            BinOp::Lt => {
                let l = encode_expr_int(left, tuple, env)?;
                let r = encode_expr_int(right, tuple, env)?;
                Ok(l.lt(&r))
            }
            BinOp::Gt => {
                let l = encode_expr_int(left, tuple, env)?;
                let r = encode_expr_int(right, tuple, env)?;
                Ok(l.gt(&r))
            }
            BinOp::Lte => {
                let l = encode_expr_int(left, tuple, env)?;
                let r = encode_expr_int(right, tuple, env)?;
                Ok(l.le(&r))
            }
            BinOp::Gte => {
                let l = encode_expr_int(left, tuple, env)?;
                let r = encode_expr_int(right, tuple, env)?;
                Ok(l.ge(&r))
            }
            BinOp::And => {
                let l = encode_expr_bool(left, tuple, env)?;
                let r = encode_expr_bool(right, tuple, env)?;
                Ok(Bool::and(&[&l, &r]))
            }
            BinOp::Or => {
                let l = encode_expr_bool(left, tuple, env)?;
                let r = encode_expr_bool(right, tuple, env)?;
                Ok(Bool::or(&[&l, &r]))
            }
            _ => Err(EncodeError::UnsupportedExpr(format!("{:?}", op))),
        },
        Expr::UnaryOp {
            op: UnaryOp::Not,
            expr: inner,
        } => Ok(encode_expr_bool(inner, tuple, env)?.not()),
        Expr::IsNull {
            expr: inner,
            negated,
        } => {
            let col_key = match inner.as_ref() {
                Expr::ColumnRef { table, column } => env.attr_key(table.as_deref(), column),
                _ => "unknown".to_string(),
            };
            let label = Dynamic::new_const(
                z3::Symbol::from(format!("lbl_{}", hash_str(&col_key)).as_str()),
                &env.string_label_sort,
            );
            let args: Vec<&dyn Ast> = vec![tuple, &label];
            let is_null = env
                .null_func
                .apply(&args)
                .as_bool()
                .ok_or(EncodeError::TypeMismatch)?;
            if *negated {
                Ok(is_null.not())
            } else {
                Ok(is_null)
            }
        }
        Expr::Exists(subquery) => {
            // EXISTS(subquery) is true if at least one concrete tuple
            // satisfies the subquery relation.
            let tuples = env.all_table_tuples();
            let mut any = Bool::from_bool(false);
            for sub_tuple in tuples {
                let pred = encode_relation_for_tuple(subquery, sub_tuple, env)?;
                any = Bool::or(&[&any, &pred]);
            }
            Ok(any)
        }
        Expr::Literal(ExprValue::Boolean(v)) => Ok(Bool::from_bool(*v)),
        _ => Err(EncodeError::UnsupportedExpr(format!(
            "encode_expr_bool: unsupported expression: {:?}",
            expr
        ))),
    }
}

/// Deterministic hash for string-to-integer mapping (column labels in Z3).
pub(crate) fn hash_str(s: &str) -> i64 {
    s.bytes()
        .fold(0i64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as i64))
}
