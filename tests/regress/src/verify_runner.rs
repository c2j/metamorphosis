//! Formal verification dimension: invokes QED or VeriEQL engines.

use crate::case_loader::{Case, VerifyEngine};
use metamorphosis_qed::prover::{ProofResult, ProverConfig};
use metamorphosis_qed::schema::extract_rich_schema;
use metamorphosis_qed::verify::verify_rewrite;
use metamorphosis_verieql::types::{
    Bound, ColumnDef, ColumnType, ProofReport, ProofResult as VeriProofResult, Semantics,
    TableSchema,
};
use metamorphosis_verieql::VeriEql;
use ogsql_parser::Parser;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum VerifyRunnerError {
    #[error("verify requires schema.sql in the case directory")]
    MissingSchema,
    #[error("failed to parse schema.sql: {0}")]
    SchemaParse(String),
    #[error("failed to parse original SQL: {0}")]
    OriginalParse(String),
    #[error("failed to parse rewritten SQL: {0}")]
    RewrittenParse(String),
    #[error("qed verify returned error: {0}")]
    Qed(#[from] metamorphosis_qed::VerifyError),
    #[error("verieql verify returned error: {0}")]
    Verieql(#[from] metamorphosis_verieql::VeriEqlError),
}

#[derive(Debug, Clone)]
pub struct VerifyOutcome {
    pub engine: VerifyEngine,
    /// Z3-equivalent proof outcome.
    pub verdict: VerifyVerdict,
    pub elapsed_ms: u64,
    /// Counterexample text when NotEquivalent, else None.
    pub counterexample: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyVerdict {
    Equivalent,
    NotEquivalent,
    Unknown,
}

pub fn run(case: &Case) -> Result<VerifyOutcome, VerifyRunnerError> {
    match case.meta.verify.engine {
        VerifyEngine::Qed => run_qed(case),
        VerifyEngine::Verieql => run_verieql(case),
    }
}

fn run_qed(case: &Case) -> Result<VerifyOutcome, VerifyRunnerError> {
    let schema_sql = case.schema_sql.as_ref().ok_or(VerifyRunnerError::MissingSchema)?;
    let (stmt_infos, errors) = Parser::parse_sql(schema_sql);
    if !errors.is_empty() {
        return Err(VerifyRunnerError::SchemaParse(format!("{:?}", errors)));
    }
    let ddl_stmts: Vec<_> = stmt_infos.into_iter().map(|si| si.statement).collect();
    let rich_schema = extract_rich_schema(&ddl_stmts);

    let orig_stmt = parse_one(&case.original_sql, VerifyRunnerError::OriginalParse)?;
    let rew_stmt = parse_one(&case.rewritten_sql, VerifyRunnerError::RewrittenParse)?;

    let rule_id = case.meta.rule.as_deref().unwrap_or("regress");
    let config = ProverConfig::default();
    let result = verify_rewrite(rule_id, &orig_stmt, &rew_stmt, &rich_schema, &config)?;

    let (verdict, counterexample) = match &result.proof {
        ProofResult::Equivalent => (VerifyVerdict::Equivalent, None),
        ProofResult::NotEquivalent { counterexample: ce } => {
            (VerifyVerdict::NotEquivalent, ce.as_ref().map(|c| c.to_string()))
        }
        ProofResult::Unknown { reason } => {
            (VerifyVerdict::Unknown, Some(reason.clone()))
        }
        ProofResult::Timeout { seconds } => {
            (VerifyVerdict::Unknown, Some(format!("timeout after {seconds}s")))
        }
        _ => (VerifyVerdict::Unknown, Some(format!("{:?}", result.proof))),
    };

    Ok(VerifyOutcome {
        engine: VerifyEngine::Qed,
        verdict,
        elapsed_ms: result.elapsed_ms,
        counterexample,
    })
}

fn run_verieql(case: &Case) -> Result<VerifyOutcome, VerifyRunnerError> {
    let schema_sql = case.schema_sql.as_ref().ok_or(VerifyRunnerError::MissingSchema)?;
    let (stmt_infos, errors) = Parser::parse_sql(schema_sql);
    if !errors.is_empty() {
        return Err(VerifyRunnerError::SchemaParse(format!("{:?}", errors)));
    }
    let ddl_stmts: Vec<_> = stmt_infos.into_iter().map(|si| si.statement).collect();
    let rich_schema = extract_rich_schema(&ddl_stmts);
    let tables = rich_to_verieql_schema(&rich_schema);

    let constraints = serde_json::Value::Null;
    let report: ProofReport = VeriEql::verify(
        &case.original_sql,
        &case.rewritten_sql,
        &tables,
        &constraints,
        Bound(case.meta.verify.bound),
        Semantics::Bag,
    )?;

    let (verdict, counterexample) = match &report.result {
        VeriProofResult::Equivalent => (VerifyVerdict::Equivalent, None),
        VeriProofResult::NotEquivalent { counterexample: ce } => {
            let text = if ce.tables.is_empty() {
                None
            } else {
                let mut s = String::new();
                for t in &ce.tables {
                    s.push_str(&format!("{}:\n", t.name));
                    for row in &t.rows {
                        s.push_str(&format!("  [{}]\n", row.join(", ")));
                    }
                }
                Some(s)
            };
            (VerifyVerdict::NotEquivalent, text)
        }
        VeriProofResult::Unknown { reason } => (VerifyVerdict::Unknown, Some(reason.clone())),
    };

    Ok(VerifyOutcome {
        engine: VerifyEngine::Verieql,
        verdict,
        elapsed_ms: report.translate_ms + report.solve_ms,
        counterexample,
    })
}

fn parse_one(
    sql: &str,
    wrap: impl FnOnce(String) -> VerifyRunnerError,
) -> Result<ogsql_parser::ast::Statement, VerifyRunnerError> {
    let out = Parser::parse_sql(sql);
    if !out.1.is_empty() {
        return Err(wrap(format!("{:?}", out.1)));
    }
    out.0
        .into_iter()
        .next()
        .map(|si| si.statement)
        .ok_or_else(|| wrap("empty input".to_string()))
}

fn rich_to_verieql_schema(schema: &metamorphosis_qed::RichSchema) -> Vec<TableSchema> {
    schema
        .tables
        .iter()
        .map(|(name, info)| TableSchema {
            name: name.clone(),
            columns: info
                .columns
                .iter()
                .map(|c| ColumnDef {
                    name: c.name.clone(),
                    col_type: sql_type_to_verieql(&c.data_type),
                })
                .collect(),
        })
        .collect()
}

fn sql_type_to_verieql(ty: &str) -> ColumnType {
    let upper = ty.to_uppercase();
    if upper.starts_with("INT")
        || upper.starts_with("BIGINT")
        || upper.starts_with("SMALLINT")
    {
        ColumnType::Integer
    } else if upper.starts_with("VARCHAR")
        || upper.starts_with("CHAR")
        || upper.starts_with("TEXT")
        || upper.starts_with("CLOB")
    {
        ColumnType::Varchar
    } else if upper.starts_with("BOOL") {
        ColumnType::Boolean
    } else if upper.starts_with("DATE")
        || upper.starts_with("TIMESTAMP")
        || upper.starts_with("TIME")
    {
        ColumnType::Date
    } else if upper.starts_with("FLOAT")
        || upper.starts_with("DOUBLE")
        || upper.starts_with("NUMERIC")
        || upper.starts_with("DECIMAL")
        || upper.starts_with("REAL")
    {
        ColumnType::Float
    } else {
        ColumnType::Integer
    }
}
