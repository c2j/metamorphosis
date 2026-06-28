//! Integration entry: `cargo test -p metamorphosis-regress`.
//!
//! A single sequential test is used instead of one-test-per-case because DB
//! isolation schemas are not safe under cargo's parallel test runner, and
//! formal verification is CPU-bound — sequential keeps timings predictable.

use std::path::PathBuf;
use std::sync::Mutex;

use metamorphosis_regress::case_loader::LoadedCases;
use metamorphosis_regress::config::Config;
use metamorphosis_regress::db_runner;
use metamorphosis_regress::reporter::{CaseReport, Report};
use metamorphosis_regress::verify_runner;

/// Serialises test execution across multiple test binaries in the same process.
static SEQUENTIAL: Mutex<()> = Mutex::new(());

#[test]
fn run_all_regress_cases() {
    let _guard = SEQUENTIAL.lock().unwrap();

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config_path = manifest_dir.join("config.toml");
    let config = Config::load_or_default(&config_path);

    let cases_root = if config.runner.cases_dir.is_absolute() {
        config.runner.cases_dir.clone()
    } else {
        manifest_dir.join(&config.runner.cases_dir)
    };

    let loaded = LoadedCases::discover(&cases_root).expect("failed to discover cases");
    if loaded.cases.is_empty() {
        eprintln!("no cases found under {}", cases_root.display());
        return;
    }

    let conn_str = config.database.connection_string();
    let db_reachable = match &conn_str {
        Some(s) => match db_runner::probe_connection(s) {
            Ok(()) => {
                eprintln!("info: database reachable — db dimensions will execute");
                true
            }
            Err(e) => {
                eprintln!("warning: database not reachable — db dimensions will be skipped: {e}");
                false
            }
        },
        None => {
            eprintln!(
                "warning: no database configured — db dimensions will be skipped. \
                 Set DATABASE_URL or create config.toml to enable."
            );
            false
        }
    };

    let mut report = Report::default();
    let mut failures: Vec<String> = Vec::new();
    let mut unknowns: Vec<String> = Vec::new();

    for case in &loaded.cases {
        let mut cr = CaseReport::new(case);

        if case.meta.verify.enabled {
            match verify_runner::run(case) {
                Ok(o) => cr.verify = Some(o),
                Err(e) => cr.verify_error = Some(e.to_string()),
            }
        }

        if case.meta.db.enabled {
            if !db_reachable {
                cr.db_skipped = true;
            } else if let Some(s) = &conn_str {
                match db_runner::run(case, s) {
                    Ok(o) => cr.db = Some(o),
                    Err(e) => cr.db_error = Some(e.to_string()),
                }
            } else {
                cr.db_skipped = true;
            }
        }

        let detail = Report::render_detail(case, &cr);
        let label = cr.status_label();
        report.record(&cr);
        eprintln!("\n{detail}\n  → {label}");

        if !cr.is_pass() {
            if cr.is_unknown() {
                unknowns.push(case.meta.name.clone());
            } else {
                failures.push(case.meta.name.clone());
            }
        }
    }

    eprintln!("{}", report.render_summary());

    if !failures.is_empty() {
        panic!("{} case(s) failed: {}", failures.len(), failures.join(", "));
    }
    if !unknowns.is_empty() {
        eprintln!(
            "warning: {} case(s) returned Unknown verdict (treat as soft-fail): {}",
            unknowns.len(),
            unknowns.join(", ")
        );
    }
}
