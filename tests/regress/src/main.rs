//! CLI entry point for the regression harness.

use std::path::PathBuf;
use std::process::ExitCode;

use metamorphosis_regress::case_loader::LoadedCases;
use metamorphosis_regress::config::Config;
use metamorphosis_regress::db_runner;
use metamorphosis_regress::reporter::{CaseReport, Report};
use metamorphosis_regress::verify_runner;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    let config_path = std::env::var("REGRESS_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.toml"));

    let config = Config::load_or_default(&config_path);
    let cases_root = if config.runner.cases_dir.is_absolute() {
        config.runner.cases_dir.clone()
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(&config.runner.cases_dir)
    };

    let loaded = match LoadedCases::discover(&cases_root) {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "error: failed to load cases from {}: {e}",
                cases_root.display()
            );
            return ExitCode::from(2);
        }
    };
    if loaded.cases.is_empty() {
        eprintln!("warn: no cases found under {}", cases_root.display());
    }

    let conn_str = config.database.connection_string();
    let db_reachable = match &conn_str {
        Some(s) => match db_runner::probe_connection(s) {
            Ok(()) => {
                println!("info: database reachable");
                true
            }
            Err(e) => {
                eprintln!("warn: database not reachable — db cases will be skipped: {e}");
                false
            }
        },
        None => {
            eprintln!("warn: no database configured — db cases will be skipped");
            false
        }
    };

    let mut report = Report::default();
    for case in &loaded.cases {
        let case_report = run_one(case, &conn_str, db_reachable);
        let is_pass = case_report.is_pass();
        report.record(&case_report);
        println!("\n{}", Report::render_detail(case, &case_report));
        println!("  → {}", case_report.status_label());
        if !is_pass && config.runner.fail_fast {
            eprintln!("fail-fast set, aborting.");
            break;
        }
    }

    println!("{}", report.render_summary());
    if report.failed > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_one(
    case: &metamorphosis_regress::Case,
    conn_str: &Option<String>,
    db_reachable: bool,
) -> CaseReport {
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
        } else if let Some(s) = conn_str {
            match db_runner::run(case, s) {
                Ok(o) => cr.db = Some(o),
                Err(e) => cr.db_error = Some(e.to_string()),
            }
        } else {
            cr.db_skipped = true;
        }
    }

    cr
}
