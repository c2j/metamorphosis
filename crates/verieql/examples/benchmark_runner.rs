//! Benchmark runner for VeriEQL compatibility testing.
//!
//! Parses VeriEQL benchmark `.jsonlines` files, attempts parse + translate
//! for each SQL pair, and reports coverage statistics.
//!
//! This example uses only `ogsql-parser` directly (no Z3) to avoid
//! the vendored Z3 initialization deadlock on macOS.
//!
//! Usage:
//!   cargo run -p metamorphosis-verieql --example benchmark_runner <file.jsonlines>

use std::env;
use std::fs;
use std::process;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct BenchmarkEntry {
    benchmark: String,
    name: String,
    index: usize,
    #[allow(dead_code)]
    schema: serde_json::Value,
    pair: Vec<String>,
    #[allow(dead_code)]
    constraint: Option<serde_json::Value>,
    #[serde(rename = "has-symbolic-predicates", default)]
    has_symbolic_predicates: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Stage {
    ParseFailed(String),
    Success,
}

fn parse_sql(sql: &str) -> Stage {
    let tokens = match ogsql_parser::Tokenizer::new(sql).tokenize() {
        Ok(t) => t,
        Err(e) => return Stage::ParseFailed(format!("{e}")),
    };
    let mut parser = ogsql_parser::parser::Parser::new(tokens);
    let stmts = parser.parse();
    match stmts.into_iter().next() {
        Some(_) => Stage::Success,
        None => Stage::ParseFailed("empty result".into()),
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <file.jsonlines>", args[0]);
        process::exit(1);
    }

    let path = &args[1];
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR: Cannot read {path}: {e}");
            process::exit(1);
        }
    };

    let entries: Vec<BenchmarkEntry> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    let total = entries.len();
    let mut skipped = 0usize;
    let mut both_ok = 0usize;
    let mut sql1_fail = Vec::new();
    let mut sql2_fail = Vec::new();

    for entry in &entries {
        if entry.has_symbolic_predicates {
            skipped += 1;
            continue;
        }

        let r1 = parse_sql(&entry.pair[0]);
        let r2 = parse_sql(&entry.pair[1]);

        let ok1 = matches!(r1, Stage::Success);
        let ok2 = matches!(r2, Stage::Success);

        if ok1 && ok2 {
            both_ok += 1;
        }
        if !ok1 {
            if let Stage::ParseFailed(e) = r1 {
                sql1_fail.push((entry.index, entry.name.clone(), e));
            }
        }
        if !ok2 {
            if let Stage::ParseFailed(e) = r2 {
                sql2_fail.push((entry.index, entry.name.clone(), e));
            }
        }
    }

    let tested = total - skipped;
    println!("=== PARSE COVERAGE REPORT: {path} ===");
    println!("Total entries: {total}");
    println!("Skipped (symbolic predicates): {skipped}");
    println!("Tested: {tested}");
    println!(
        "Both SQL parse OK: {}/{} ({:.1}%)",
        both_ok,
        tested,
        if tested > 0 {
            both_ok as f64 / tested as f64 * 100.0
        } else {
            0.0
        }
    );
    println!("SQL1 parse failures: {}", sql1_fail.len());
    println!("SQL2 parse failures: {}", sql2_fail.len());
    println!();

    if !sql1_fail.is_empty() {
        println!("--- SQL1 Parse Failures ---");
        for (idx, name, err) in &sql1_fail {
            println!("  [{idx}] {name}: {err}");
        }
        println!();
    }

    if !sql2_fail.is_empty() {
        println!("--- SQL2 Parse Failures ---");
        for (idx, name, err) in &sql2_fail {
            println!("  [{idx}] {name}: {err}");
        }
    }
}
