//! Core types for VeriEQL bounded equivalence verification.

use serde::{Deserialize, Serialize};

/// Tuple bound for bounded model checking.
///
/// A bound of N means each table is modeled with N symbolic tuples.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bound(pub usize);

impl Default for Bound {
    fn default() -> Self {
        Bound(2)
    }
}

/// Column type for schema definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ColumnType {
    Integer,
    Varchar,
    Boolean,
    Date,
    Float,
}

/// Schema for a single table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<ColumnDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnDef {
    pub name: String,
    pub col_type: ColumnType,
}

/// Result of bounded equivalence check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProofResult {
    /// Queries are equivalent up to the given bound.
    Equivalent,
    /// Queries are NOT equivalent; counterexample provided.
    NotEquivalent { counterexample: Counterexample },
    /// Z3 returned Unknown.
    Unknown { reason: String },
}

/// Human-readable counterexample database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Counterexample {
    pub tables: Vec<CounterexampleTable>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CounterexampleTable {
    pub name: String,
    pub rows: Vec<Vec<String>>,
}

/// Full verification report with timing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofReport {
    pub result: ProofResult,
    pub translate_ms: u64,
    pub solve_ms: u64,
    pub bound: Bound,
}

/// Verification semantics.
#[derive(Debug, Clone, PartialEq)]
pub enum Semantics {
    /// Bag semantics (default): duplicates matter.
    Bag,
    /// List semantics: ORDER BY position matters.
    List,
}
