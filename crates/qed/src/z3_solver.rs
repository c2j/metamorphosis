//! Z3-based SQL query equivalence prover.
//!
//! Encodes QED relational algebra as Z3 constraints over uninterpreted table
//! functions. Two queries Q1, Q2 are equivalent iff for all t. Q1(t) iff Q2(t).
//! We check the contrapositive: exists t. Q1(t) xor Q2(t) is UNSAT => Equivalent.

use std::collections::{HashMap, HashSet};
use std::ops::{Add, Mul, Sub};

use z3::ast::{self, Ast, Bool, Dynamic, Int};
use z3::{FuncDecl, SatResult, Solver, Sort};

use crate::ir::{QedExpr, QedInput, QedRelation, QedSchema, QedValue};
use crate::prover::{ProofResult, ProverError};

/// Prove query equivalence using the embedded Z3 SMT solver.
///
/// Models each base table as an uninterpreted function `(Int, …, Int) → Bool`
/// and encodes both queries as membership predicates over the same output
/// tuple variables. If the symmetric difference is empty (UNSAT), the
/// queries are semantically equivalent.
///
/// # Errors
///
/// Returns [`ProverError::Io`] when the QED IR references unknown tables
/// or contains unsupported constructs.
pub fn solve_equivalence(input: &QedInput) -> Result<ProofResult, ProverError> {
    if input.queries[0] == input.queries[1] {
        tracing::debug!("queries structurally identical -> Equivalent");
        return Ok(ProofResult::Equivalent);
    }

    let schema_map = build_schema_map(&input.schemas);
    let table_funcs = create_table_funcs(&input.schemas);

    let arity1 = compute_output_arity(&input.queries[0], &schema_map)?;
    let arity2 = compute_output_arity(&input.queries[1], &schema_map)?;

    if arity1 != arity2 {
        return Ok(ProofResult::NotEquivalent {
            counterexample: None,
        });
    }
    if arity1 == 0 {
        return Ok(ProofResult::Equivalent);
    }

    let output_vars: Vec<Int> = (0..arity1)
        .map(|i| Int::new_const(format!("out_{i}")))
        .collect();

    let result = (|| -> Result<ProofResult, ProverError> {
        let q1 = encode_relation(&input.queries[0], &output_vars, &schema_map, &table_funcs)?;
        let q2 = encode_relation(&input.queries[1], &output_vars, &schema_map, &table_funcs)?;

        let solver = Solver::new();
        solver.assert(q1.eq(&q2).not());

        match solver.check() {
            SatResult::Unsat => Ok(ProofResult::Equivalent),
            SatResult::Sat => {
                let ce = solver.get_model().map(|m| format!("{m}"));
                Ok(ProofResult::NotEquivalent { counterexample: ce })
            }
            SatResult::Unknown => Ok(ProofResult::Unknown {
                reason: "Z3 solver returned Unknown".to_string(),
            }),
        }
    })();

    match result {
        Ok(proof) => Ok(proof),
        Err(ProverError::Io(msg))
            if msg.contains("DISTINCT") || msg.contains("quantified") =>
        {
            Ok(ProofResult::Unknown { reason: msg })
        }
        Err(e) => Err(e),
    }
}

fn build_schema_map(schemas: &[QedSchema]) -> HashMap<String, QedSchema> {
    schemas
        .iter()
        .map(|s| (s.name.clone(), s.clone()))
        .collect()
}

fn create_table_funcs(schemas: &[QedSchema]) -> HashMap<String, FuncDecl> {
    let mut funcs = HashMap::with_capacity(schemas.len());
    for schema in schemas {
        let arity = schema.fields.len();
        if arity == 0 {
            continue;
        }
        let sorts: Vec<Sort> = (0..arity).map(|_| Sort::int()).collect();
        let refs: Vec<&Sort> = sorts.iter().collect();
        let func = FuncDecl::new(
            format!("tbl_{}", schema.name).as_str(),
            &refs,
            &Sort::bool(),
        );
        funcs.insert(schema.name.clone(), func);
    }
    funcs
}

fn compute_output_arity(
    rel: &QedRelation,
    schemas: &HashMap<String, QedSchema>,
) -> Result<usize, ProverError> {
    match rel {
        QedRelation::Scan { table, fields } => {
            let s = schemas
                .get(table)
                .ok_or_else(|| ProverError::Io(format!("unknown table: {table}")))?;
            Ok(if fields.is_empty() {
                s.fields.len()
            } else {
                fields.len()
            })
        }
        QedRelation::Filter { input, .. } => compute_output_arity(input, schemas),
        QedRelation::Project { exprs, .. } => Ok(exprs.len()),
        QedRelation::Join { left, right, .. } => {
            Ok(compute_output_arity(left, schemas)? + compute_output_arity(right, schemas)?)
        }
        QedRelation::Union { left, .. } | QedRelation::Intersect { left, .. } => {
            compute_output_arity(left, schemas)
        }
        QedRelation::Except { left, .. } => compute_output_arity(left, schemas),
        QedRelation::Distinct { input } => compute_output_arity(input, schemas),
        QedRelation::Values { rows } => Ok(rows.first().map_or(0, |r| r.len())),
        QedRelation::Aggregate { keys, aggs, .. } => Ok(keys.len() + aggs.len()),
        QedRelation::QOp { input, .. } => compute_output_arity(input, schemas),
    }
}

fn encode_relation(
    rel: &QedRelation,
    output_vars: &[Int],
    schemas: &HashMap<String, QedSchema>,
    table_funcs: &HashMap<String, FuncDecl>,
) -> Result<Bool, ProverError> {
    match rel {
        QedRelation::Scan { table, fields } => {
            encode_scan(table, fields, output_vars, schemas, table_funcs)
        }
        QedRelation::Filter { condition, input } => {
            let inp = encode_relation(input, output_vars, schemas, table_funcs)?;
            let c = encode_expr(condition, output_vars)?
                .as_bool()
                .ok_or_else(|| ProverError::Io("filter condition must be bool".into()))?;
            Ok(Bool::and(&[&inp, &c]))
        }
        QedRelation::Project { exprs, input } => {
            encode_project(exprs, input, output_vars, schemas, table_funcs)
        }
        QedRelation::Join {
            left,
            right,
            condition,
        } => encode_join(left, right, condition, output_vars, schemas, table_funcs),
        QedRelation::Union { left, right } => {
            let l = encode_relation(left, output_vars, schemas, table_funcs)?;
            let r = encode_relation(right, output_vars, schemas, table_funcs)?;
            Ok(Bool::or(&[&l, &r]))
        }
        QedRelation::Intersect { left, right } => {
            let l = encode_relation(left, output_vars, schemas, table_funcs)?;
            let r = encode_relation(right, output_vars, schemas, table_funcs)?;
            Ok(Bool::and(&[&l, &r]))
        }
        QedRelation::Except { left, right } => {
            let l = encode_relation(left, output_vars, schemas, table_funcs)?;
            let r = encode_relation(right, output_vars, schemas, table_funcs)?;
            Ok(Bool::and(&[&l, &r.not()]))
        }
        QedRelation::Distinct { input: _ } => {
            tracing::warn!("Distinct relation cannot be soundly encoded in set-based Z3 encoding; returning error");
            return Err(ProverError::Io(
                "DISTINCT cannot be soundly encoded: set-based membership predicates do not track multiplicity".into(),
            ));
        }
        QedRelation::Values { rows } => encode_values(rows, output_vars),
        QedRelation::Aggregate { .. } => encode_uninterpreted("agg", output_vars),
        QedRelation::QOp { input, .. } => encode_relation(input, output_vars, schemas, table_funcs),
    }
}

fn encode_scan(
    table: &str,
    fields: &[usize],
    output_vars: &[Int],
    schemas: &HashMap<String, QedSchema>,
    table_funcs: &HashMap<String, FuncDecl>,
) -> Result<Bool, ProverError> {
    let schema = schemas
        .get(table)
        .ok_or_else(|| ProverError::Io(format!("unknown table: {table}")))?;
    let func = table_funcs
        .get(table)
        .ok_or_else(|| ProverError::Io(format!("no Z3 func for table: {table}")))?;

    let full_arity = schema.fields.len();
    if fields.is_empty() || fields.len() == full_arity {
        let args: Vec<&dyn Ast> = output_vars.iter().map(|v| v as &dyn Ast).collect();
        func.apply(&args)
            .as_bool()
            .ok_or_else(|| ProverError::Io("table func must return bool".into()))
    } else {
        let field_set: HashSet<usize> = fields.iter().copied().collect();
        let mut full_vars: Vec<Int> = Vec::with_capacity(full_arity);
        let mut exist_vars: Vec<Int> = Vec::new();
        let mut out_idx = 0;
        for i in 0..full_arity {
            if field_set.contains(&i) {
                full_vars.push(output_vars[out_idx].clone());
                out_idx += 1;
            } else {
                let v = Int::fresh_const(&format!("sc_{table}_{i}"));
                exist_vars.push(v.clone());
                full_vars.push(v);
            }
        }
        let args: Vec<&dyn Ast> = full_vars.iter().map(|v| v as &dyn Ast).collect();
        let tc = func
            .apply(&args)
            .as_bool()
            .ok_or_else(|| ProverError::Io("table func must return bool".into()))?;
        make_exists(&exist_vars, &tc)
    }
}

fn encode_project(
    exprs: &[QedExpr],
    input: &QedRelation,
    output_vars: &[Int],
    schemas: &HashMap<String, QedSchema>,
    table_funcs: &HashMap<String, FuncDecl>,
) -> Result<Bool, ProverError> {
    let input_arity = compute_output_arity(input, schemas)?;
    let iv: Vec<Int> = (0..input_arity)
        .map(|i| Int::fresh_const(&format!("pj_{i}")))
        .collect();
    let input_f = encode_relation(input, &iv, schemas, table_funcs)?;
    let mut parts: Vec<Bool> = vec![input_f];
    for (i, expr) in exprs.iter().enumerate() {
        let val = encode_expr(expr, &iv)?;
        parts.push(Dynamic::from(output_vars[i].clone()).eq(&val));
    }
    make_exists(&iv, &Bool::and(&parts))
}

fn encode_join(
    left: &QedRelation,
    right: &QedRelation,
    condition: &Option<QedExpr>,
    output_vars: &[Int],
    schemas: &HashMap<String, QedSchema>,
    table_funcs: &HashMap<String, FuncDecl>,
) -> Result<Bool, ProverError> {
    let la = compute_output_arity(left, schemas)?;
    let lf = encode_relation(left, &output_vars[..la], schemas, table_funcs)?;
    let rf = encode_relation(right, &output_vars[la..], schemas, table_funcs)?;
    let mut parts = vec![lf, rf];
    if let Some(cond) = condition {
        parts.push(
            encode_expr(cond, output_vars)?
                .as_bool()
                .ok_or_else(|| ProverError::Io("join condition must be bool".into()))?,
        );
    }
    Ok(Bool::and(&parts))
}

fn encode_values(rows: &[Vec<QedExpr>], output_vars: &[Int]) -> Result<Bool, ProverError> {
    if rows.is_empty() {
        return Ok(Bool::from_bool(false));
    }
    let rfs: Result<Vec<Bool>, ProverError> = rows
        .iter()
        .map(|row| {
            let eqs: Result<Vec<Bool>, ProverError> = row
                .iter()
                .enumerate()
                .map(|(i, expr)| {
                    let val = encode_expr(expr, output_vars)?;
                    Ok(Dynamic::from(output_vars[i].clone()).eq(&val))
                })
                .collect();
            Ok(Bool::and(&eqs?))
        })
        .collect();
    Ok(Bool::or(&rfs?))
}

fn encode_uninterpreted(name: &str, output_vars: &[Int]) -> Result<Bool, ProverError> {
    if output_vars.is_empty() {
        return Ok(Bool::from_bool(true));
    }
    let sorts: Vec<Sort> = output_vars.iter().map(|_| Sort::int()).collect();
    let refs: Vec<&Sort> = sorts.iter().collect();
    let func = FuncDecl::new(name, &refs, &Sort::bool());
    let args: Vec<&dyn Ast> = output_vars.iter().map(|v| v as &dyn Ast).collect();
    func.apply(&args)
        .as_bool()
        .ok_or_else(|| ProverError::Io("uninterpreted func must return bool".into()))
}

fn encode_expr(expr: &QedExpr, vars: &[Int]) -> Result<Dynamic, ProverError> {
    match expr {
        QedExpr::ColumnRef { index } => vars
            .get(*index)
            .map(|v| Dynamic::from(v.clone()))
            .ok_or_else(|| ProverError::Io(format!("column index out of range: {index}"))),
        QedExpr::Literal { value } => match value {
            QedValue::Integer { value: v } => Ok(Dynamic::from(Int::from_i64(*v))),
            QedValue::Boolean { value: v } => Ok(Dynamic::from(Bool::from_bool(*v))),
            QedValue::String { .. } | QedValue::Float { .. } => {
                Ok(Dynamic::from(Int::fresh_const("lit")))
            }
        },
        QedExpr::BinOp { op, left, right } => {
            let l = encode_expr(left, vars)?;
            let r = encode_expr(right, vars)?;
            encode_binop(op, &l, &r)
        }
        QedExpr::UnOp { op, expr: inner } => {
            let v = encode_expr(inner, vars)?;
            match op.as_str() {
                "not" => v
                    .as_bool()
                    .map(|b| Dynamic::from(b.not()))
                    .ok_or_else(|| ProverError::Io("not: expected bool".into())),
                "neg" => {
                    let i = v
                        .as_int()
                        .ok_or_else(|| ProverError::Io("neg: expected int".into()))?;
                    Ok(Dynamic::from(Int::from_i64(0).sub(&i)))
                }
                _ => Err(ProverError::Io(format!("unknown unary op: {op}"))),
            }
        }
        QedExpr::Null => Ok(Dynamic::from(Int::new_const("SQL_NULL"))),
        QedExpr::Function { name, .. } => {
            tracing::warn!("uninterpreted function '{name}' -> fresh variable");
            Ok(Dynamic::from(Int::fresh_const("fn")))
        }
        QedExpr::Quantified { .. } => {
            tracing::warn!("quantified expression cannot be soundly encoded");
            Err(ProverError::Io(
                "quantified expression (IN/EXISTS subquery) not supported in Z3 encoding; \
                 decorrelation should have handled this".into(),
            ))
        }
    }
}

fn encode_binop(op: &str, left: &Dynamic, right: &Dynamic) -> Result<Dynamic, ProverError> {
    match op {
        "eq" => Ok(Dynamic::from(left.eq(right))),
        "neq" => Ok(Dynamic::from(left.eq(right).not())),
        "and" => {
            let l = left
                .as_bool()
                .ok_or_else(|| ProverError::Io("and: expected bool lhs".into()))?;
            let r = right
                .as_bool()
                .ok_or_else(|| ProverError::Io("and: expected bool rhs".into()))?;
            Ok(Dynamic::from(Bool::and(&[&l, &r])))
        }
        "or" => {
            let l = left
                .as_bool()
                .ok_or_else(|| ProverError::Io("or: expected bool lhs".into()))?;
            let r = right
                .as_bool()
                .ok_or_else(|| ProverError::Io("or: expected bool rhs".into()))?;
            Ok(Dynamic::from(Bool::or(&[&l, &r])))
        }
        "gt" | "lt" | "gte" | "lte" => encode_cmp(op, left, right),
        "add" | "sub" | "mul" => encode_arith(op, left, right),
        _ => {
            tracing::warn!("unknown binary op '{op}' -> fresh Bool");
            Ok(Dynamic::from(Bool::fresh_const("bop")))
        }
    }
}

fn encode_cmp(op: &str, left: &Dynamic, right: &Dynamic) -> Result<Dynamic, ProverError> {
    let l = left
        .as_int()
        .ok_or_else(|| ProverError::Io(format!("{op}: expected int lhs")))?;
    let r = right
        .as_int()
        .ok_or_else(|| ProverError::Io(format!("{op}: expected int rhs")))?;
    let b = match op {
        "gt" => l.gt(&r),
        "lt" => l.lt(&r),
        "gte" => l.ge(&r),
        "lte" => l.le(&r),
        _ => unreachable!(),
    };
    Ok(Dynamic::from(b))
}

fn encode_arith(op: &str, left: &Dynamic, right: &Dynamic) -> Result<Dynamic, ProverError> {
    let l = left
        .as_int()
        .ok_or_else(|| ProverError::Io(format!("{op}: expected int lhs")))?;
    let r = right
        .as_int()
        .ok_or_else(|| ProverError::Io(format!("{op}: expected int rhs")))?;
    let result = match op {
        "add" => l.add(&r),
        "sub" => l.sub(&r),
        "mul" => l.mul(&r),
        _ => unreachable!(),
    };
    Ok(Dynamic::from(result))
}

fn make_exists(vars: &[Int], body: &Bool) -> Result<Bool, ProverError> {
    if vars.is_empty() {
        return Ok(body.clone());
    }
    let bounds: Vec<&dyn Ast> = vars.iter().map(|v| v as &dyn Ast).collect();
    Ok(ast::exists_const(&bounds, &[], body))
}
