//! Harness configuration loaded from `config.toml`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub database: DbConfig,
    #[serde(default)]
    pub runner: RunnerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DbConfig {
    /// Full connection URL, e.g. `postgres://user:pwd@host:5432/db`.
    /// When set, takes precedence over the individual fields below.
    pub url: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerConfig {
    /// Directory scanned for case folders. Defaults to `cases/`.
    #[serde(default = "default_cases_dir")]
    pub cases_dir: PathBuf,
    /// When true, abort after the first failing case (default: false).
    #[serde(default)]
    pub fail_fast: bool,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            cases_dir: default_cases_dir(),
            fail_fast: false,
        }
    }
}

fn default_cases_dir() -> PathBuf {
    PathBuf::from("cases")
}

impl DbConfig {
    /// Resolve the libpq-style connection string.
    ///
    /// Precedence: `DATABASE_URL` env var > `url` field > assembled fields.
    /// Returns `None` when no usable configuration is present.
    pub fn connection_string(&self) -> Option<String> {
        if let Ok(url) = std::env::var("DATABASE_URL") {
            if !url.trim().is_empty() {
                return Some(url);
            }
        }
        if let Some(url) = &self.url {
            if !url.trim().is_empty() {
                return Some(url.clone());
            }
        }
        let host = self
            .host
            .clone()
            .or_else(|| std::env::var("PGHOST").ok().filter(|s| !s.is_empty()));
        let user = self
            .user
            .clone()
            .or_else(|| std::env::var("PGUSER").ok().filter(|s| !s.is_empty()));
        let password = self
            .password
            .clone()
            .or_else(|| std::env::var("PGPASSWORD").ok().filter(|s| !s.is_empty()));
        let port = self
            .port
            .or_else(|| std::env::var("PGPORT").ok().and_then(|s| s.parse().ok()));
        let database = self
            .database
            .clone()
            .or_else(|| std::env::var("PGDATABASE").ok().filter(|s| !s.is_empty()));

        let (host, user) = match (host, user) {
            (Some(h), Some(u)) => (h, u),
            _ => return None,
        };

        let mut s = String::from("postgres://");
        s.push_str(&user);
        if let Some(pwd) = password {
            s.push(':');
            s.push_str(&urlencode(&pwd));
        }
        s.push('@');
        s.push_str(&host);
        if let Some(p) = port {
            s.push(':');
            s.push_str(&p.to_string());
        }
        if let Some(db) = database {
            s.push('/');
            s.push_str(&db);
        }
        Some(s)
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

impl Config {
    /// Load config from a TOML file. Returns `Default` if the file is missing
    /// so the harness can still run with env-only configuration.
    pub fn load_or_default(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(content) => toml::from_str(&content).unwrap_or_else(|e| {
                eprintln!(
                    "warn: failed to parse '{}' ({}); using defaults",
                    path.display(),
                    e
                );
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }
}
