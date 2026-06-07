use z3::ast::{Bool, Dynamic, Int};

use crate::encoder::hash_str;
use crate::environment::Environment;
use crate::types::Semantics;

#[derive(Debug, thiserror::Error)]
pub enum VerifierError {
    #[error("Z3 encoding error: {0}")]
    EncodingError(String),
    #[error("column mismatch: left has {left} columns, right has {right}")]
    ColumnMismatch { left: usize, right: usize },
}

/// Build the equivalence formula between two query result sets.
pub fn build_equivalence_formula(
    l_tuples: &[Dynamic],
    r_tuples: &[Dynamic],
    l_attr_keys: &[String],
    r_attr_keys: &[String],
    env: &Environment,
) -> Result<Bool, VerifierError> {
    if l_attr_keys.len() != r_attr_keys.len() {
        return Err(VerifierError::ColumnMismatch {
            left: l_attr_keys.len(),
            right: r_attr_keys.len(),
        });
    }

    match env.semantics {
        Semantics::Bag => build_bag_equivalence(l_tuples, r_tuples, l_attr_keys, r_attr_keys, env),
        Semantics::List => build_list_equivalence(l_tuples, r_tuples, l_attr_keys, r_attr_keys, env),
    }
}

fn build_bag_equivalence(
    l_tuples: &[Dynamic],
    r_tuples: &[Dynamic],
    l_attr_keys: &[String],
    r_attr_keys: &[String],
    env: &Environment,
) -> Result<Bool, VerifierError> {
    let mut formulas: Vec<Bool> = Vec::new();

    let l_size = count_non_deleted(l_tuples, env);
    let r_size = count_non_deleted(r_tuples, env);
    formulas.push(l_size.eq(&r_size));

    for lt in l_tuples {
        let count_l = count_equals(lt, l_tuples, l_attr_keys, l_attr_keys, env)?;
        let count_r = count_equals(lt, r_tuples, l_attr_keys, r_attr_keys, env)?;
        let not_del = env.deleted_func.apply(&[lt]).as_bool().unwrap().not();
        formulas.push(not_del.implies(count_l.eq(&count_r)));
    }

    for rt in r_tuples {
        let count_l = count_equals(rt, l_tuples, r_attr_keys, l_attr_keys, env)?;
        let count_r = count_equals(rt, r_tuples, r_attr_keys, r_attr_keys, env)?;
        let not_del = env.deleted_func.apply(&[rt]).as_bool().unwrap().not();
        formulas.push(not_del.implies(count_l.eq(&count_r)));
    }

    Ok(Bool::and(&formulas))
}

fn build_list_equivalence(
    l_tuples: &[Dynamic],
    r_tuples: &[Dynamic],
    l_attr_keys: &[String],
    r_attr_keys: &[String],
    env: &Environment,
) -> Result<Bool, VerifierError> {
    let mut formulas: Vec<Bool> = Vec::new();

    let l_size = count_non_deleted(l_tuples, env);
    let r_size = count_non_deleted(r_tuples, env);
    formulas.push(l_size.eq(&r_size));

    for (lt, rt) in l_tuples.iter().zip(r_tuples.iter()) {
        let eq = tuple_equals(lt, rt, l_attr_keys, r_attr_keys, env)?;
        let l_not_del = env.deleted_func.apply(&[lt]).as_bool().unwrap().not();
        let r_not_del = env.deleted_func.apply(&[rt]).as_bool().unwrap().not();
        formulas.push(Bool::and(&[&l_not_del, &r_not_del]).implies(eq));
    }

    Ok(Bool::and(&formulas))
}

fn tuple_equals(
    t1: &Dynamic,
    t2: &Dynamic,
    attrs1: &[String],
    attrs2: &[String],
    env: &Environment,
) -> Result<Bool, VerifierError> {
    let both_deleted = Bool::and(&[
        &env.deleted_func.apply(&[t1]).as_bool().unwrap(),
        &env.deleted_func.apply(&[t2]).as_bool().unwrap(),
    ]);

    let mut eqs: Vec<Bool> = Vec::new();

    for (a1, a2) in attrs1.iter().zip(attrs2.iter()) {
        let f1 = env.attr_funcs.get(a1)
            .ok_or_else(|| VerifierError::EncodingError(format!("unknown attr: {a1}")))?;
        let f2 = env.attr_funcs.get(a2)
            .ok_or_else(|| VerifierError::EncodingError(format!("unknown attr: {a2}")))?;

        let v1 = f1.apply(&[t1]).as_int().unwrap();
        let v2 = f2.apply(&[t2]).as_int().unwrap();

        let label1 = Dynamic::new_const(
            z3::Symbol::from(format!("lbl_{}", hash_str(a1)).as_str()),
            &env.string_label_sort,
        );
        let label2 = Dynamic::new_const(
            z3::Symbol::from(format!("lbl_{}", hash_str(a2)).as_str()),
            &env.string_label_sort,
        );

        let null1 = env.null_func.apply(&[t1, &label1]).as_bool().unwrap();
        let null2 = env.null_func.apply(&[t2, &label2]).as_bool().unwrap();

        let both_null = Bool::and(&[&null1, &null2]);
        let both_present_and_eq = Bool::and(&[&null1.not(), &null2.not(), &v1.eq(&v2)]);
        eqs.push(Bool::or(&[&both_null, &both_present_and_eq]));
    }

    let values_match = Bool::and(&eqs);

    let t1_not_del = env.deleted_func.apply(&[t1]).as_bool().unwrap().not();
    let t2_not_del = env.deleted_func.apply(&[t2]).as_bool().unwrap().not();
    let both_alive = Bool::and(&[&t1_not_del, &t2_not_del, &values_match]);

    Ok(Bool::or(&[&both_deleted, &both_alive]))
}

fn count_non_deleted(tuples: &[Dynamic], env: &Environment) -> Int {
    let one = Int::from_i64(1);
    let zero = Int::from_i64(0);
    let terms: Vec<Int> = tuples.iter().map(|t| {
        let del = env.deleted_func.apply(&[t]).as_bool().unwrap();
        Bool::ite(&del.not(), &one, &zero)
    }).collect();
    sum_ints(&terms)
}

fn count_equals(
    target: &Dynamic,
    tuples: &[Dynamic],
    target_attrs: &[String],
    tuple_attrs: &[String],
    env: &Environment,
) -> Result<Int, VerifierError> {
    let one = Int::from_i64(1);
    let zero = Int::from_i64(0);
    let mut terms = Vec::with_capacity(tuples.len());
    for t in tuples {
        let eq = tuple_equals(target, t, target_attrs, tuple_attrs, env)?;
        terms.push(Bool::ite(&eq, &one, &zero));
    }
    Ok(sum_ints(&terms))
}

fn sum_ints(terms: &[Int]) -> Int {
    if terms.is_empty() {
        return Int::from_i64(0);
    }
    if terms.len() == 1 {
        return terms[0].clone();
    }
    let refs: Vec<&Int> = terms.iter().collect();
    Int::add(&refs)
}
