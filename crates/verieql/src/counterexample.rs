use crate::encoder::hash_str;
use crate::environment::Environment;
use crate::types::{Counterexample, CounterexampleTable};

/// Extract a human-readable counterexample from a Z3 model.
pub fn extract_counterexample(
    model: &z3::Model,
    env: &Environment,
    table_names: &[String],
) -> Counterexample {
    let mut tables = Vec::new();

    for name in table_names {
        let upper_name = name.to_uppercase();
        let mut rows = Vec::new();

        for i in 0..env.bound_size {
            let tuple_name = format!("t{}", i + 1);
            let tuple = z3::ast::Dynamic::new_const(tuple_name, &env.tuple_sort);

            let deleted_val = model.eval(
                &env.deleted_func.apply(&[&tuple]).as_bool().unwrap(), true,
            );
            if is_z3_true(&deleted_val) {
                continue;
            }

            let mut row = Vec::new();
            for (key, func) in &env.attr_funcs {
                if !key.starts_with(&format!("{}.", upper_name)) {
                    continue;
                }
                let val = model.eval(&func.apply(&[&tuple]).as_int().unwrap(), false);
                match val {
                    Some(v) => {
                        if let Some(iv) = v.as_i64() {
                            let col_label = hash_str(key.split('.').next_back().unwrap_or(""));
                            let label_sym = z3::Symbol::from(format!("lbl_{col_label}").as_str());
                            let label_dyn = z3::ast::Dynamic::new_const(label_sym, &env.string_label_sort);
                            let is_null_val = model.eval(
                                &env.null_func.apply(&[&tuple, &label_dyn]).as_bool().unwrap(), true,
                            );
                            if is_z3_true(&is_null_val) {
                                row.push("NULL".to_string());
                                continue;
                            }
                            row.push(iv.to_string());
                        } else {
                            row.push("?".to_string());
                        }
                    }
                    None => row.push("?".to_string()),
                }
            }

            if !row.is_empty() {
                rows.push(row);
            }
        }

        tables.push(CounterexampleTable { name: name.clone(), rows });
    }

    Counterexample { tables }
}

fn is_z3_true(opt: &Option<z3::ast::Bool>) -> bool {
    opt.as_ref()
        .and_then(|b| b.as_bool())
        .unwrap_or(false)
}
