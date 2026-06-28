//! Discovers and parses regression cases from `cases/<name>/` directories.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CaseLoadError {
    #[error("io error reading '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse case.toml at '{path}': {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("case '{name}' is missing required file: {file}")]
    MissingFile { name: String, file: &'static str },
    #[error("case '{name}' has no SQL in original.sql / rewritten.sql")]
    EmptySql { name: String },
    #[error("case '{name}' has verify.bound={bound}, must be ≥ 1 (Bound(0) trivially proves equivalence)")]
    Bound { name: String, bound: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum VerifyExpect {
    #[default]
    Equivalent,
    NotEquivalent,
    Unknown,
    /// Skip verdict check (only useful for debugging the harness itself).
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DbExpect {
    #[default]
    Equal,
    Mismatch,
    /// Skip db outcome check.
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VerifyEngine {
    #[default]
    Qed,
    Verieql,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CompareMode {
    /// Rows must appear in the same order on both sides (require ORDER BY).
    Ordered,
    /// Rows may appear in any order; sort both sides before comparing.
    #[default]
    Unordered,
    /// Compare as a set (dedup + sort).
    Set,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CaseVerifyConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub engine: VerifyEngine,
    /// VeriEQL bound (ignored for Qed).
    #[serde(default = "default_bound")]
    pub bound: usize,
    #[serde(default)]
    pub expect: VerifyExpect,
}

fn default_true() -> bool {
    true
}

fn default_bound() -> usize {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CaseDbConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub compare: CompareMode,
    #[serde(default)]
    pub expect: DbExpect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseMeta {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub rule: Option<String>,
    #[serde(default)]
    pub verify: CaseVerifyConfig,
    #[serde(default)]
    pub db: CaseDbConfig,
}

/// A fully loaded regression case ready to be executed by the runners.
#[derive(Debug, Clone)]
pub struct Case {
    pub meta: CaseMeta,
    pub dir: PathBuf,
    pub original_sql: String,
    pub rewritten_sql: String,
    /// DDL used both for QED schema extraction and DB setup.
    pub schema_sql: Option<String>,
    /// Seed `INSERT` statements executed inside the case schema before queries.
    pub data_sql: Option<String>,
}

#[derive(Debug, Default)]
pub struct LoadedCases {
    pub cases: Vec<Case>,
    pub root: PathBuf,
}

impl LoadedCases {
    /// Scan `root` for immediate child directories that contain a `case.toml`.
    pub fn discover(root: &Path) -> Result<Self, CaseLoadError> {
        let mut cases = Vec::new();
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(source) => {
                return Err(CaseLoadError::Io {
                    path: root.to_path_buf(),
                    source,
                });
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let case_toml = path.join("case.toml");
            if !case_toml.exists() {
                continue;
            }
            match Case::load(&path) {
                Ok(c) => cases.push(c),
                Err(CaseLoadError::Io { path, source }) if source.kind() == std::io::ErrorKind::NotFound => {
                    eprintln!("warn: skipping '{}': missing case files", path.display());
                }
                Err(e) => return Err(e),
            }
        }
        cases.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));
        Ok(Self {
            cases,
            root: root.to_path_buf(),
        })
    }
}

impl Case {
    pub fn load(dir: &Path) -> Result<Self, CaseLoadError> {
        let case_toml_path = dir.join("case.toml");
        let toml_str = read_to_string(&case_toml_path)?;
        let meta: CaseMeta = toml::from_str(&toml_str).map_err(|source| CaseLoadError::Toml {
            path: case_toml_path.clone(),
            source,
        })?;

        if meta.verify.enabled && meta.verify.bound == 0 {
            return Err(CaseLoadError::Bound {
                name: meta.name.clone(),
                bound: meta.verify.bound,
            });
        }

        let original_sql = read_optional(dir, "original.sql")?.unwrap_or_default();
        let rewritten_sql = read_optional(dir, "rewritten.sql")?.unwrap_or_default();

        if original_sql.trim().is_empty() {
            return Err(CaseLoadError::EmptySql { name: meta.name.clone() });
        }
        if rewritten_sql.trim().is_empty() {
            return Err(CaseLoadError::EmptySql { name: meta.name.clone() });
        }

        let schema_sql = read_optional(dir, "schema.sql")?;
        let data_sql = read_optional(dir, "data.sql")?;

        Ok(Self {
            meta,
            dir: dir.to_path_buf(),
            original_sql,
            rewritten_sql,
            schema_sql,
            data_sql,
        })
    }
}

fn read_optional(dir: &Path, file: &str) -> Result<Option<String>, CaseLoadError> {
    let path = dir.join(file);
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CaseLoadError::Io {
            path,
            source,
        }),
    }
}

fn read_to_string(path: &Path) -> Result<String, CaseLoadError> {
    std::fs::read_to_string(path).map_err(|source| CaseLoadError::Io {
        path: path.to_path_buf(),
        source,
    })
}
