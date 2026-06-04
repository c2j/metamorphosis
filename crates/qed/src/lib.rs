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
//! 3. `translator` — ogsql-parser AST → QED Relation translator *(future)*
//! 4. `prover` — QED prover binary harness *(future)*
//! 5. `verify` — End-to-end verification pipeline *(future)*

pub mod ir;
pub mod schema;

pub use ir::{QedAggArg, QedAggCall, QedExpr, QedInput, QedRelation, QedSchema, QedValue};

pub use schema::{
    CheckConstraint, ColumnInfo, ForeignKeyInfo, ReferentialAction, RichSchema, TableConstraints,
    TableInfo,
};
