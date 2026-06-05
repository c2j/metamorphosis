//! QED-based offline verification for Metamorphosis rewrite rules.
//!
//! Provides tools to verify that SQL rewrites produced by Metamorphosis
//! rules are semantically equivalent to the original queries, using the
//! QED prover (<https://github.com/qed-solver/prover>).
//!
//! # Architecture
//!
//! 1. [`schema`] — Extract rich schema (PK, FK, NOT NULL, CHECK) from DDL
//! 2. [`ir`] — QED intermediate representation types (Rust → JSON)
//! 3. [`translator`] — ogsql-parser AST → QED Relation translator
//! 4. [`prover`] — QED prover binary harness
//! 5. [`verify`] — End-to-end verification pipeline

pub mod ir;
pub mod prover;
pub mod prover_compat;
pub mod schema;
pub mod translator;
pub mod verify;
pub mod z3_solver;

pub use ir::{QedAggArg, QedAggCall, QedExpr, QedInput, QedRelation, QedSchema, QedValue};

pub use prover::{ProofResult, ProverConfig, ProverError};
pub use prover_compat::{
    convert_expr, convert_input, convert_relation, map_data_type, ProverAggCall, ProverDataType,
    ProverExpr, ProverInput, ProverJoinKind, ProverRelation, ProverSchema, VL,
};

pub use schema::{
    CheckConstraint, ColumnInfo, ForeignKeyInfo, ReferentialAction, RichSchema, TableConstraints,
    TableInfo,
};

pub use translator::{AstTranslator, TranslateError};

pub use verify::{VerificationResult, VerifyError};
