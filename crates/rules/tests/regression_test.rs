//! Data-driven regression tests for built-in rules.
//!
//! Discovers test files under `testcases/regress/<rule_module>/`:
//! - `<case>.input.sql`    — SQL fed into the engine
//! - `<case>.expected.sql` — fragment assertions (must-contain / must-not-contain)
//! - `<case>.full.sql`     — complete expected output for exact normalised comparison
//!
//! Rule directories are auto-resolved via [`builtin_rules`](metamorphosis_rules::builtin_rules)
//! ID matching — adding cases requires **zero** Rust code changes.
//!
//! Filter to a single rule with `REGRESS_RULE=<dir_or_id>`:
//! ```text
//! REGRESS_RULE=nvl_to_case cargo test -p metamorphosis-rules --test regression_test
//! ```
//!
//! Auto-generate `.full.sql` files with `REGEN_FULL=1`:
//! ```text
//! REGEN_FULL=1 cargo test -p metamorphosis-rules --test regression_test
//! ```

use metamorphosis_core::{
    RewriteAction, RewriteConfig, RewriteContext, RewriteEngine, RewriteResult, RuleRegistry,
    SafetyLevel,
};
use metamorphosis_rules::builtin_rules;
use ogsql_parser::analyzer::schema::SchemaMap;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::Parser;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

struct TestCase {
    name: String,
    input: String,
    expected: Option<String>,
    full: Option<String>,
    is_negative: bool,
}

#[test]
fn regression_suite() {
    let root = regress_root();
    assert!(
        root.is_dir(),
        "regress directory not found at {}",
        root.display()
    );

    let mut errors: Vec<String> = Vec::new();
    let mut total: usize = 0;
    let mut rules_tested: Vec<String> = Vec::new();

    let filter = std::env::var("REGRESS_RULE").ok();
    let regen_full = std::env::var("REGEN_FULL").is_ok();

    for entry in fs::read_dir(&root).expect("failed to read regress dir") {
        let Ok(entry) = entry else { continue };
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() { continue }

        let dir_name = entry.file_name().to_str().unwrap().to_string();
        if dir_name.starts_with('_') { continue }

        let rule_id = dir_name.replace('_', "-");

        if let Some(ref f) = filter {
            let f = f.as_str();
            if dir_name != f && rule_id != f {
                continue;
            }
        }

        let Some(rule) = builtin_rules().into_iter().find(|r| r.id() == rule_id) else {
            errors.push(format!(
                "'{dir_name}/' has no matching rule (id '{rule_id}') — register it in builtin_rules()"
            ));
            continue;
        };

        let is_manual = rule.safety_level() == SafetyLevel::Manual;
        let engine = RewriteEngine::new(RuleRegistry::new(vec![rule]));
        let config = RewriteConfig::default();
        let schema = load_schema(&entry.path());
        let known_variables = load_variables(&entry.path());

        let cases = discover_cases(&entry.path());
        if cases.is_empty() {
            errors.push(format!("'{dir_name}/' has no .input.sql cases"));
            continue;
        }

        rules_tested.push(rule_id.clone());
        for case in &cases {
            total += 1;
            let label = format!("{dir_name}/{}", case.name);
            if let Err(msg) = run_case(
                case,
                &engine,
                &config,
                is_manual,
                &entry.path(),
                regen_full,
                schema.as_ref(),
                known_variables.as_ref(),
            ) {
                errors.push(format!("[{label}] {msg}"));
            }
        }
    }

    if !errors.is_empty() {
        panic!(
            "\n{} of {} regression case(s) failed:\n\n{}\n",
            errors.len(),
            total,
            errors.join("\n")
        );
    }

    if let Some(ref f) = filter {
        if rules_tested.is_empty() {
            panic!("REGRESS_RULE='{f}' matched no rule directory");
        }
    }

    eprintln!(
        " {} regression case(s) passed across {} rule(s): {}",
        total,
        rules_tested.len(),
        rules_tested.join(", ")
    );
}

fn regress_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testcases/regress")
}

fn load_schema(dir: &Path) -> Option<SchemaMap> {
    let content = fs::read_to_string(dir.join("_schema.json")).ok()?;
    serde_json::from_str(&content).ok()
}

fn load_variables(dir: &Path) -> Option<HashSet<String>> {
    let content = fs::read_to_string(dir.join("_variables.txt")).ok()?;
    let vars: HashSet<String> = content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();
    if vars.is_empty() { None } else { Some(vars) }
}

#[derive(Default)]
struct CaseFiles {
    input: Option<String>,
    expected: Option<String>,
    full: Option<String>,
}

fn discover_cases(dir: &Path) -> Vec<TestCase> {
    let mut files: BTreeMap<String, CaseFiles> = BTreeMap::new();

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let os_name = entry.file_name();
            let Some(filename) = os_name.to_str() else { continue };
            let content = fs::read_to_string(entry.path()).ok();

            if let Some(cn) = filename.strip_suffix(".input.sql") {
                files.entry(cn.to_string()).or_default().input = content;
            } else if let Some(cn) = filename.strip_suffix(".expected.sql") {
                files.entry(cn.to_string()).or_default().expected = content;
            } else if let Some(cn) = filename.strip_suffix(".full.sql") {
                files.entry(cn.to_string()).or_default().full = content;
            }
        }
    }

    files
        .into_iter()
        .filter_map(|(name, cf)| {
            Some(TestCase {
                is_negative: name.starts_with("neg-"),
                name,
                input: cf.input?,
                expected: cf.expected,
                full: cf.full,
            })
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn run_case(
    case: &TestCase,
    engine: &RewriteEngine,
    config: &RewriteConfig,
    is_manual: bool,
    rule_dir: &Path,
    regen_full: bool,
    schema: Option<&SchemaMap>,
    known_variables: Option<&HashSet<String>>,
) -> Result<(), String> {
    let ctx = RewriteContext {
        version: None,
        schema,
        config,
        source_file: None,
        known_variables,
    };

    let (stmts, parse_errors) = Parser::parse_sql(&case.input);
    if !parse_errors.is_empty() {
        return Err(format!("input.sql has parse errors: {parse_errors:?}"));
    }
    let statements: Vec<Statement> = stmts.into_iter().map(|si| si.statement).collect();
    if statements.is_empty() {
        return Err("input.sql produced no parseable statements".to_string());
    }

    let result = engine.rewrite(&ctx, statements);
    let output = format_output(&result, is_manual);

    let skip_full = is_manual && case.is_negative;

    if regen_full && !skip_full {
        let full_path = rule_dir.join(format!("{}.full.sql", case.name));
        fs::write(&full_path, &output)
            .map_err(|e| format!("failed to write {}: {e}", full_path.display()))?;
    }

    if case.is_negative {
        verify_negative(case, &result, &output, is_manual)?;
    } else {
        verify_positive(case, &result, &output, is_manual)?;
    }

    if let Some(full) = &case.full {
        if !skip_full && !regen_full {
            check_full(&output, full)?;
        }
    }

    Ok(())
}

fn verify_positive(
    case: &TestCase,
    result: &RewriteResult,
    output: &str,
    is_manual: bool,
) -> Result<(), String> {
    if is_manual {
        if result.suggestions.is_empty() {
            return Err("expected probe suggestion(s) but none generated".to_string());
        }
    } else if !result.changed {
        return Err("expected rewrite but statement was not changed".to_string());
    }

    let expected = case
        .expected
        .as_ref()
        .ok_or("positive case is missing .expected.sql")?;

    check_fragments(output, expected)
}

fn verify_negative(
    case: &TestCase,
    result: &RewriteResult,
    output: &str,
    is_manual: bool,
) -> Result<(), String> {
    if is_manual {
        if !result.suggestions.is_empty() {
            return Err(format!(
                "expected no suggestions but got {}: {output}",
                result.suggestions.len()
            ));
        }
    } else if result.changed {
        return Err(format!("expected no rewrite but statement was changed: {output}"));
    }

    if let Some(expected) = &case.expected {
        check_fragments(output, expected)?;
    }
    Ok(())
}

fn format_output(result: &RewriteResult, is_manual: bool) -> String {
    let fmt = SqlFormatter::new();
    if is_manual {
        result
            .suggestions
            .iter()
            .filter_map(|s| match &s.action {
                RewriteAction::Generate { stmt, .. } | RewriteAction::Replace(stmt) => {
                    Some(fmt.format_statement(stmt))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        result
            .statements
            .iter()
            .map(|s| fmt.format_statement(s))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn check_fragments(output: &str, expected: &str) -> Result<(), String> {
    let normalized_output = normalize(output);

    for line in expected.lines() {
        if let Some(forbidden) = line.trim().strip_prefix('!') {
            let forbidden = normalize(forbidden);
            if normalized_output.contains(&forbidden) {
                return Err(format!(
                    "output must NOT contain '{forbidden}':\n  {output}"
                ));
            }
        }
    }

    let meaningful: Vec<&str> = expected
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with("--") && !l.starts_with('!'))
        .collect();

    let exact_mode = expected
        .lines()
        .any(|l| l.trim().eq_ignore_ascii_case("-- @exact"));

    if exact_mode {
        let normalized_expected = normalize(&meaningful.join(" "));
        if normalized_output != normalized_expected {
            return Err(format!(
                "exact match failed:\n  expected: {normalized_expected}\n  actual:   {normalized_output}"
            ));
        }
    } else {
        for fragment in &meaningful {
            let normalized_fragment = normalize(fragment);
            if !normalized_output.contains(&normalized_fragment) {
                return Err(format!(
                    "output must contain '{normalized_fragment}':\n  {output}"
                ));
            }
        }
    }

    Ok(())
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn check_full(output: &str, full: &str) -> Result<(), String> {
    let normalized_output = normalize(output);
    let normalized_full = normalize(full);
    if normalized_output != normalized_full {
        return Err(format!(
            "full match failed:\n  expected: {normalized_full}\n  actual:   {normalized_output}"
        ));
    }
    Ok(())
}
