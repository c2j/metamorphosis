//! VeriEQL: Bounded equivalence verification for SQL queries.
//!
//! Port of the VeriEQL (OOPSLA 2024 Distinguished Paper) algorithm to Rust.
//! Uses `ogsql-parser` for SQL parsing and `z3` for SMT-based bounded model
//! checking.
//!
//! # Dependencies
//!
//! Only depends on `ogsql-parser`, `z3`, `serde`, `thiserror`, and `tracing`.
//! Zero dependencies on other metamorphosis crates (`core`, `rules`, `qed`,
//! `cli`).

pub mod constraints;
pub mod counterexample;
pub mod encoder;
pub mod environment;
pub mod ir;
pub mod translator;
pub mod types;
pub mod verifier;

use std::time::Instant;

use types::*;

/// Main entry point for bounded SQL equivalence verification.
///
/// Uses Z3 to check whether two SQL queries produce the same result
/// for all databases up to a given tuple bound.
///
/// # Example
///
/// ```ignore
/// use metamorphosis_verieql::{VeriEql, types::*};
///
/// let report = VeriEql::verify(
///     "SELECT DISTINCT ID FROM EMP",
///     "SELECT ID FROM EMP GROUP BY ID",
///     &[TableSchema { name: "EMP".into(), columns: vec![
///         ColumnDef { name: "ID".into(), col_type: ColumnType::Integer },
///     ]}],
///     &serde_json::json!([{"primary": [["EMP__ID"]]}]),
///     Bound(2),
///     Semantics::Bag,
/// ).unwrap();
///
/// assert!(matches!(report.result, ProofResult::Equivalent));
/// ```
pub struct VeriEql;

impl VeriEql {
    /// Verify equivalence of two SQL queries under bounded model checking.
    ///
    /// Creates B symbolic tuples per table, encodes both queries as Z3
    /// membership predicates over a shared output tuple variable, and checks
    /// that the symmetric difference is UNSAT.
    pub fn verify(
        sql1: &str,
        sql2: &str,
        schema: &[TableSchema],
        constraints: &serde_json::Value,
        bound: Bound,
        semantics: Semantics,
    ) -> Result<ProofReport, VeriEqlError> {
        let t0 = Instant::now();

        let stmt1 = Self::parse_sql(sql1)?;
        let stmt2 = Self::parse_sql(sql2)?;

        let ir1 = translator::translate(&stmt1)?;
        let ir2 = translator::translate(&stmt2)?;

        let translate_ms = t0.elapsed().as_millis() as u64;
        let t_solve = Instant::now();

        let mut env = environment::Environment::new(bound.clone(), semantics);

        for table_schema in schema {
            env.create_database(table_schema);
        }

        constraints::apply_constraints(constraints, &mut env)?;

        // Register table aliases so qualified column references resolve correctly.
        register_aliases(&mut env, &ir1);
        register_aliases(&mut env, &ir2);

        let output_tuple = env.declare_tuple();

        let q1_pred = encoder::encode_relation_for_tuple(&ir1, &output_tuple, &env)?;
        let q2_pred = encoder::encode_relation_for_tuple(&ir2, &output_tuple, &env)?;

        for fact in &env.dbms_facts {
            env.solver.assert(fact);
        }

        // ∃output_tuple. Q1(output) XOR Q2(output) — UNSAT => equivalent
        let symmetric_diff = q1_pred.xor(&q2_pred);
        env.solver.assert(&symmetric_diff);

        let result = match env.solver.check() {
            z3::SatResult::Unsat => types::ProofResult::Equivalent,
            z3::SatResult::Sat => {
                let model = env.solver.get_model();
                let table_names: Vec<String> = schema.iter().map(|s| s.name.clone()).collect();
                let ce =
                    model.map(|m| counterexample::extract_counterexample(&m, &env, &table_names));
                types::ProofResult::NotEquivalent {
                    counterexample: ce.unwrap_or_else(|| types::Counterexample { tables: vec![] }),
                }
            }
            z3::SatResult::Unknown => types::ProofResult::Unknown {
                reason: "Z3 returned Unknown".to_string(),
            },
        };

        let solve_ms = t_solve.elapsed().as_millis() as u64;

        Ok(ProofReport {
            result,
            translate_ms,
            solve_ms,
            bound,
        })
    }

    fn parse_sql(sql: &str) -> Result<ogsql_parser::ast::Statement, VeriEqlError> {
        let tokens = ogsql_parser::Tokenizer::new(sql)
            .tokenize()
            .map_err(|e| VeriEqlError::ParseError(e.to_string()))?;
        let mut parser = ogsql_parser::parser::Parser::new(tokens);
        let stmts = parser.parse();
        stmts
            .into_iter()
            .next()
            .ok_or_else(|| VeriEqlError::ParseError("empty SQL input".to_string()))
    }
}

/// Walk the IR tree and register table aliases into the environment.
fn register_aliases(env: &mut environment::Environment, rel: &ir::Relation) {
    match rel {
        ir::Relation::BaseTable { name, alias, .. } => {
            if let Some(a) = alias {
                env.register_alias(a, name);
            }
        }
        ir::Relation::Filter { input, .. }
        | ir::Relation::Project { input, .. }
        | ir::Relation::Distinct { input }
        | ir::Relation::OrderBy { input, .. } => register_aliases(env, input),
        ir::Relation::Join { left, right, .. } => {
            register_aliases(env, left);
            register_aliases(env, right);
        }
        ir::Relation::GroupBy { input, .. } => register_aliases(env, input),
        ir::Relation::Union { left, right, .. }
        | ir::Relation::Intersect { left, right, .. }
        | ir::Relation::Except { left, right, .. } => {
            register_aliases(env, left);
            register_aliases(env, right);
        }
        _ => {}
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VeriEqlError {
    #[error("SQL parse error: {0}")]
    ParseError(String),
    #[error("translation error: {0}")]
    TranslateError(#[from] translator::TranslateError),
    #[error("encoding error: {0}")]
    EncodeError(#[from] encoder::EncodeError),
    #[error("verification error: {0}")]
    VerifyError(#[from] verifier::VerifierError),
    #[error("constraint error: {0}")]
    ConstraintError(#[from] constraints::ConstraintError),
}
