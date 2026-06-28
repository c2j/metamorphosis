//! DB execution dimension: runs both SQL statements against a real
//! openGauss instance and compares the result sets.

use crate::case_loader::{Case, CompareMode};
use postgres::types::Type;
use postgres::Client;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DbRunnerError {
    #[error("postgres connection failed: {0}")]
    Connect(#[from] postgres::Error),
    #[error("sql execution error in '{phase}': {source}")]
    Exec {
        phase: &'static str,
        #[source]
        source: postgres::Error,
    },
    #[error("result sets differ (mode={mode:?}): {detail}")]
    Mismatch { mode: CompareMode, detail: String },
    #[error("schema.sql is required when db is enabled")]
    MissingSchema,
}

/// Probes connectivity once at harness startup so that all db-enabled cases
/// can be skipped uniformly when no DB is reachable.
pub fn probe_connection(conn_str: &str) -> Result<(), postgres::Error> {
    let _ = Client::connect(conn_str, postgres::NoTls)?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct DbOutcome {
    pub mode: CompareMode,
    pub row_count_original: usize,
    pub row_count_rewritten: usize,
    pub column_count_original: usize,
    pub column_count_rewritten: usize,
    pub mismatch: Option<String>,
}

impl DbOutcome {
    fn equal(mode: CompareMode, orig: &ResultSet, rew: &ResultSet) -> Self {
        Self {
            mode,
            row_count_original: orig.rows.len(),
            row_count_rewritten: rew.rows.len(),
            column_count_original: orig.columns,
            column_count_rewritten: rew.columns,
            mismatch: None,
        }
    }

    fn mismatch(mode: CompareMode, orig: &ResultSet, rew: &ResultSet, detail: String) -> Self {
        Self {
            mode,
            row_count_original: orig.rows.len(),
            row_count_rewritten: rew.rows.len(),
            column_count_original: orig.columns,
            column_count_rewritten: rew.columns,
            mismatch: Some(detail),
        }
    }
}

#[derive(Debug, Default)]
struct ResultSet {
    columns: usize,
    rows: Vec<Vec<String>>,
}

pub fn run(case: &Case, conn_str: &str) -> Result<DbOutcome, DbRunnerError> {
    let schema_sql = case.schema_sql.as_ref().ok_or(DbRunnerError::MissingSchema)?;

    let mut client =
        Client::connect(conn_str, postgres::NoTls).map_err(DbRunnerError::Connect)?;

    let schema_name = isolation_schema_name(&case.meta.name, &case.dir);
    setup_isolation(&mut client, &schema_name)?;

    let result = run_case_inner(&mut client, schema_sql, case);
    let _ = teardown_isolation(&mut client, &schema_name);
    result
}

fn run_case_inner(
    client: &mut Client,
    schema_sql: &str,
    case: &Case,
) -> Result<DbOutcome, DbRunnerError> {
    client
        .batch_execute(schema_sql)
        .map_err(|source| DbRunnerError::Exec {
            phase: "schema_ddl",
            source,
        })?;

    if let Some(data) = &case.data_sql {
        if !data.trim().is_empty() {
            client.batch_execute(data).map_err(|source| DbRunnerError::Exec {
                phase: "data_seed",
                source,
            })?;
        }
    }

    let orig = run_one_query(client, &case.original_sql)?;
    let rew = run_one_query(client, &case.rewritten_sql)?;

    Ok(compare(&orig, &rew, case.meta.db.compare))
}

fn setup_isolation(client: &mut Client, schema: &str) -> Result<(), DbRunnerError> {
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE;
             CREATE SCHEMA {schema};
             SET search_path TO {schema};"
        ))
        .map_err(|source| DbRunnerError::Exec {
            phase: "schema_setup",
            source,
        })?;
    Ok(())
}

fn teardown_isolation(client: &mut Client, schema: &str) -> Result<(), DbRunnerError> {
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE;
             RESET search_path;"
        ))
        .map_err(|source| DbRunnerError::Exec {
            phase: "schema_cleanup",
            source,
        })?;
    Ok(())
}

fn run_one_query(client: &mut Client, sql: &str) -> Result<ResultSet, DbRunnerError> {
    let mut tx = client
        .transaction()
        .map_err(|source| DbRunnerError::Exec {
            phase: "begin",
            source,
        })?;
    let stmt = tx.prepare(sql).map_err(|source| DbRunnerError::Exec {
        phase: "prepare_query",
        source,
    })?;
    let cols = stmt.columns().len();
    let rows = tx
        .query(&stmt, &[])
        .map_err(|source| DbRunnerError::Exec {
            phase: "exec_query",
            source,
        })?;
    let rendered = render_rows(&rows, cols);
    tx.rollback().map_err(|source| DbRunnerError::Exec {
        phase: "rollback",
        source,
    })?;
    Ok(ResultSet {
        columns: cols,
        rows: rendered,
    })
}

fn compare(orig: &ResultSet, rew: &ResultSet, mode: CompareMode) -> DbOutcome {
    if orig.columns != rew.columns {
        return DbOutcome::mismatch(
            mode,
            orig,
            rew,
            format!("column count: {} vs {}", orig.columns, rew.columns),
        );
    }

    let mut o = orig.rows.clone();
    let mut r = rew.rows.clone();

    match mode {
        CompareMode::Ordered => {
            if o == r {
                DbOutcome::equal(mode, orig, rew)
            } else {
                let pos = o
                    .iter()
                    .zip(r.iter())
                    .position(|(a, b)| a != b)
                    .unwrap_or(usize::min(o.len(), r.len()));
                DbOutcome::mismatch(
                    mode,
                    orig,
                    rew,
                    format!("first differing row index {pos}"),
                )
            }
        }
        CompareMode::Unordered => {
            o.sort();
            r.sort();
            if o == r {
                DbOutcome::equal(mode, orig, rew)
            } else {
                DbOutcome::mismatch(
                    mode,
                    orig,
                    rew,
                    format!("sorted rows differ ({} vs {})", o.len(), r.len()),
                )
            }
        }
        CompareMode::Set => {
            o.sort();
            o.dedup();
            r.sort();
            r.dedup();
            if o == r {
                DbOutcome::equal(mode, orig, rew)
            } else {
                let only_orig: Vec<_> = o.iter().filter(|x| !r.contains(x)).collect();
                let only_rew: Vec<_> = r.iter().filter(|x| !o.contains(x)).collect();
                DbOutcome::mismatch(
                    mode,
                    orig,
                    rew,
                    format!(
                        "set diff: only_original={}, only_rewritten={}",
                        only_orig.len(),
                        only_rew.len()
                    ),
                )
            }
        }
    }
}

fn render_rows(rows: &[postgres::Row], cols: usize) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| (0..cols).map(|i| cell_to_string(row, i)).collect::<Vec<_>>())
        .collect()
}

fn cell_to_string(row: &postgres::Row, idx: usize) -> String {
    let col_type = row.columns()[idx].type_();
    macro_rules! try_get {
        ($t:ty) => {
            match row.try_get::<_, Option<$t>>(idx) {
                Ok(Some(v)) => return v.to_string(),
                Ok(None) => return "NULL".to_string(),
                Err(_) => {}
            }
        };
    }
    if col_type == &Type::BOOL {
        try_get!(bool);
    }
    if matches!(
        col_type,
        &Type::INT2 | &Type::INT4 | &Type::INT8 | &Type::OID
    ) {
        try_get!(i64);
    }
    if matches!(col_type, &Type::FLOAT4 | &Type::FLOAT8 | &Type::NUMERIC) {
        try_get!(f64);
    }
    try_get!(String);
    match row.try_get::<_, Option<Vec<u8>>>(idx) {
        Ok(Some(bytes)) => return String::from_utf8_lossy(&bytes).into_owned(),
        Ok(None) => return "NULL".to_string(),
        Err(_) => {}
    }
    "<unsupported>".to_string()
}

fn isolation_schema_name(case_name: &str, dir: &std::path::Path) -> String {
    let dir_name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(case_name);
    let safe: String = dir_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let prefix = &safe[..safe.len().min(40)];
    let hash = fxhash(dir.to_string_lossy().as_bytes());
    format!("regress_{prefix}_{hash:08x}")
}

fn fxhash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0x517cc1b727220a95;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
