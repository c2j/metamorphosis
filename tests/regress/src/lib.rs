//! Regression harness for metamorphosis rewrite rules.
//!
//! Each case lives in `cases/<name>/` with `case.toml` declaring the two
//! verification dimensions:
//!
//! - **formal verify** — invokes `metamorphosis-qed` or `metamorphosis-verieql`
//!   to prove semantic equivalence offline (Z3 SMT).
//! - **db execute** — runs the original and rewritten SQL against a real
//!   openGauss instance and compares result sets.
//!
//! The harness auto-discovers cases and runs both dimensions, skipping the
//! DB dimension gracefully when no database is reachable.

pub mod case_loader;
pub mod config;
pub mod db_runner;
pub mod reporter;
pub mod verify_runner;

pub use case_loader::{
    Case, CaseDbConfig, CaseVerifyConfig, CompareMode, DbExpect, LoadedCases, VerifyEngine,
    VerifyExpect,
};
pub use config::{Config, DbConfig, RunnerConfig};
pub use db_runner::DbOutcome;
pub use reporter::{CaseReport, Report};
pub use verify_runner::{VerifyOutcome, VerifyVerdict};
