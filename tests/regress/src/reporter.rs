//! Aggregates per-case outcomes into a final report with PASS/FAIL/SKIP tallies.

use crate::case_loader::{Case, DbExpect, VerifyExpect};
use crate::db_runner::DbOutcome;
use crate::verify_runner::{VerifyOutcome, VerifyVerdict};

#[derive(Debug, Clone)]
pub struct CaseReport {
    pub case_name: String,
    pub verify_expect: VerifyExpect,
    pub db_expect: DbExpect,
    pub verify: Option<VerifyOutcome>,
    pub verify_error: Option<String>,
    pub db: Option<DbOutcome>,
    pub db_skipped: bool,
    pub db_error: Option<String>,
}

impl CaseReport {
    pub fn new(case: &Case) -> Self {
        Self {
            case_name: case.meta.name.clone(),
            verify_expect: case.meta.verify.expect,
            db_expect: case.meta.db.expect,
            verify: None,
            verify_error: None,
            db: None,
            db_skipped: false,
            db_error: None,
        }
    }

    fn verify_match(&self) -> bool {
        if self.verify_expect == VerifyExpect::Any {
            return true;
        }
        let Some(o) = &self.verify else {
            return self.verify_error.is_none();
        };
        matches!(
            (self.verify_expect, o.verdict),
            (VerifyExpect::Equivalent, VerifyVerdict::Equivalent)
                | (VerifyExpect::NotEquivalent, VerifyVerdict::NotEquivalent)
                | (VerifyExpect::Unknown, VerifyVerdict::Unknown)
        )
    }

    fn db_match(&self) -> bool {
        if self.db_expect == DbExpect::Any {
            return true;
        }
        if self.db_skipped {
            return true;
        }
        let Some(o) = &self.db else {
            return self.db_error.is_none();
        };
        matches!(
            (self.db_expect, o.mismatch.is_some()),
            (DbExpect::Equal, false) | (DbExpect::Mismatch, true)
        )
    }

    pub fn is_pass(&self) -> bool {
        self.verify_match()
            && self.db_match()
            && self.verify_error.is_none()
            && self.db_error.is_none()
    }

    pub fn is_unknown(&self) -> bool {
        !self.is_pass()
            && self.verify_error.is_none()
            && self.db_error.is_none()
            && matches!(
                self.verify.as_ref().map(|o| &o.verdict),
                Some(VerifyVerdict::Unknown)
            )
            && !matches!(self.verify_expect, VerifyExpect::Unknown)
    }

    pub fn status_label(&self) -> &'static str {
        if self.is_pass() {
            if self.db_skipped {
                "PASS (db skipped)"
            } else {
                "PASS"
            }
        } else if self.is_unknown() {
            "UNKNOWN"
        } else {
            "FAIL"
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Report {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub unknown: usize,
    pub failures: Vec<CaseReport>,
}

impl Report {
    pub fn record(&mut self, case: &CaseReport) {
        if case.is_pass() {
            self.passed += 1;
        } else if case.is_unknown() {
            self.unknown += 1;
        } else {
            self.failed += 1;
            self.failures.push(case.clone());
        }
        if case.db_skipped {
            self.skipped += 1;
        }
    }

    pub fn render_detail(case: &Case, report: &CaseReport) -> String {
        let mut lines = Vec::new();
        lines.push(format!("── {} ──", case.meta.name));
        if !case.meta.description.is_empty() {
            lines.push(format!("  {}", case.meta.description));
        }
        if case.meta.verify.enabled {
            let expect_label = format!("{:?}", case.meta.verify.expect);
            match (&report.verify, &report.verify_error) {
                (Some(o), _) => {
                    let verdict = match o.verdict {
                        VerifyVerdict::Equivalent => "✓ Equivalent",
                        VerifyVerdict::NotEquivalent => "✗ NotEquivalent",
                        VerifyVerdict::Unknown => "? Unknown",
                    };
                    let mark = if matches!(
                        (case.meta.verify.expect, o.verdict),
                        (VerifyExpect::Equivalent, VerifyVerdict::Equivalent)
                            | (VerifyExpect::NotEquivalent, VerifyVerdict::NotEquivalent)
                            | (VerifyExpect::Unknown, VerifyVerdict::Unknown)
                            | (VerifyExpect::Any, _)
                    ) {
                        "✓"
                    } else {
                        "✗"
                    };
                    lines.push(format!(
                        "  verify [{:?}] expect={}: {} {} ({}ms)",
                        o.engine, expect_label, mark, verdict, o.elapsed_ms
                    ));
                    if let Some(ce) = &o.counterexample {
                        for line in ce.lines() {
                            lines.push(format!("    {line}"));
                        }
                    }
                }
                (None, Some(e)) => lines.push(format!("  verify: error — {e}")),
                (None, None) => lines.push("  verify: not run".to_string()),
            }
        }
        if case.meta.db.enabled {
            let expect_label = format!("{:?}", case.meta.db.expect);
            if report.db_skipped {
                lines.push(format!(
                    "  db:     skipped (no database reachable) [expect={expect_label}]"
                ));
            }
            match (&report.db, &report.db_error) {
                (Some(o), _) => {
                    let mismatched = o.mismatch.is_some();
                    let actual = if mismatched { "mismatch" } else { "equal" };
                    let mark = match (case.meta.db.expect, mismatched) {
                        (DbExpect::Equal, false)
                        | (DbExpect::Mismatch, true)
                        | (DbExpect::Any, _) => "✓",
                        _ => "✗",
                    };
                    lines.push(format!(
                        "  db [{:?}] expect={}: {} {} (orig={}rows × {}cols, rew={}rows × {}cols)",
                        o.mode,
                        expect_label,
                        mark,
                        actual,
                        o.row_count_original,
                        o.column_count_original,
                        o.row_count_rewritten,
                        o.column_count_rewritten
                    ));
                    if let Some(m) = &o.mismatch {
                        lines.push(format!("    {m}"));
                    }
                }
                (None, Some(e)) => lines.push(format!("  db:     error — {e}")),
                _ => {}
            }
        }
        lines.join("\n")
    }

    pub fn render_summary(&self) -> String {
        format!(
            "\n═══ Summary: {} passed, {} unknown, {} failed, {} db-skipped ═══",
            self.passed, self.unknown, self.failed, self.skipped
        )
    }
}
