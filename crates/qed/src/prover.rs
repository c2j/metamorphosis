//! QED prover binary harness.
//!
//! Wraps the external `qed-prover` binary as a subprocess: serializes [`QedInput`]
//! to a temp JSON file, invokes the prover, and parses its output into a
//! [`ProofResult`].
//!
//! Tests mock `std::process::Output` directly — no `qed-prover` binary required.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use crate::ir::QedInput;
use crate::prover_compat;

/// Result of a QED equivalence proof attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProofResult {
    /// QED proved the two queries are semantically equivalent.
    Equivalent,
    /// QED found a counterexample (queries are NOT equivalent).
    NotEquivalent { counterexample: Option<String> },
    /// QED could not determine equivalence within the timeout.
    Unknown { reason: String },
    /// The prover process timed out.
    Timeout { seconds: u64 },
}

/// Configuration for the QED prover invocation.
#[derive(Debug, Clone)]
pub struct ProverConfig {
    /// Path to the `qed-prover` binary.
    pub binary_path: PathBuf,
    /// Timeout in seconds for the prover process.
    pub timeout_secs: u64,
    /// Working directory for the prover (optional).
    pub workdir: Option<PathBuf>,
}

impl Default for ProverConfig {
    fn default() -> Self {
        Self {
            binary_path: PathBuf::from("qed-prover"),
            timeout_secs: 60,
            workdir: None,
        }
    }
}

/// Errors from prover invocation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProverError {
    /// Failed to serialize [`QedInput`] to JSON.
    #[error("serialization failed: {0}")]
    Serialization(String),
    /// IO error (temp file creation, process spawn, etc.).
    #[error("IO error: {0}")]
    Io(String),
    /// The prover process returned a non-zero exit or could not be started.
    #[error("prover process error: {0}")]
    Process(String),
    /// The prover did not finish within the configured timeout.
    #[error("prover timed out after {0}s")]
    Timeout(u64),
    /// The prover produced output that could not be classified.
    #[error("unexpected prover output:\n  stdout: {stdout}\n  stderr: {stderr}")]
    UnexpectedOutput {
        /// Captured standard output.
        stdout: String,
        /// Captured standard error.
        stderr: String,
    },
}

/// Run the QED prover on the given [`QedInput`].
///
/// Converts `input` to the prover's native JSON format via [`prover_compat`],
/// writes to a temporary file, spawns the `qed-prover` binary with a timeout,
/// and returns the parsed [`ProofResult`].
///
/// If `schema_name_map` is `None`, table names are used as-is for schema
/// qualification. Provide a map (e.g., `"users"` → `"PUBLIC.users"`) for
/// qualified names matching the prover's convention.
pub fn run_prover(
    input: &QedInput,
    config: &ProverConfig,
    schema_name_map: Option<&HashMap<String, String>>,
) -> Result<ProofResult, ProverError> {
    // Primary: embedded Z3 solver (no external binary required)
    match crate::z3_solver::solve_equivalence(input) {
        Ok(result) => {
            tracing::debug!("Z3 solver returned: {result:?}");
            return Ok(result);
        }
        Err(e) => {
            tracing::warn!("Z3 solver failed: {e}; falling back to binary prover");
        }
    }

    // Fallback: external qed-prover binary
    let name_map = schema_name_map.cloned().unwrap_or_default();
    let prover_input = prover_compat::convert_input(input, &name_map);
    let json = serde_json::to_string_pretty(&prover_input)
        .map_err(|e| ProverError::Serialization(e.to_string()))?;

    let temp_dir = tempfile::tempdir().map_err(|e| ProverError::Io(e.to_string()))?;
    let input_path = temp_dir.path().join("input.json");
    std::fs::write(&input_path, &json).map_err(|e| ProverError::Io(e.to_string()))?;

    let (tx, rx) = mpsc::channel();
    let binary = config.binary_path.clone();
    let input_path_clone = input_path.clone();

    // Child handle shared with timeout path for process killing.
    let child_arc: std::sync::Arc<std::sync::Mutex<Option<std::process::Child>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let child_for_thread = child_arc.clone();

    let handle = std::thread::spawn(move || {
        match std::process::Command::new(&binary)
            .arg(&input_path_clone)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(child) => {
                *child_for_thread.lock().unwrap() = Some(child);
                // Take back ownership — wait_with_output consumes self
                let taken = child_for_thread.lock().unwrap().take();
                match taken {
                    Some(c) => {
                        let _ = tx.send(c.wait_with_output().map_err(|e| e.to_string()));
                    }
                    None => {
                        let _ = tx.send(Err("child handle lost".to_string()));
                    }
                }
            }
            Err(e) => {
                let _ = tx.send(Err(e.to_string()));
            }
        }
    });

    match rx.recv_timeout(Duration::from_secs(config.timeout_secs)) {
        Ok(Ok(output)) => {
            let result_path = input_path.with_extension("result");
            if result_path.exists() {
                match parse_result_file(&result_path) {
                    Ok(file_result) if file_result.provable => {
                        return Ok(ProofResult::Equivalent);
                    }
                    Ok(file_result) if file_result.smt_timed_out => {
                        return Ok(ProofResult::Unknown {
                            reason: "SMT solver timed out".to_string(),
                        });
                    }
                    Ok(file_result) if file_result.panicked => {
                        return Ok(ProofResult::Unknown {
                            reason: "Prover panicked during execution".to_string(),
                        });
                    }
                    Ok(_) => {
                        return Ok(ProofResult::NotEquivalent {
                            counterexample: None,
                        });
                    }
                    Err(_) => {
                        tracing::warn!(
                            "Failed to parse .result file, falling back to stdout parsing"
                        );
                    }
                }
            }
            parse_prover_output(&output)
        }
        Ok(Err(e)) => Err(ProverError::Process(e)),
        Err(_) => {
            if let Ok(mut guard) = child_arc.lock() {
                if let Some(ref mut child) = *guard {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            let _ = handle.join();
            Ok(ProofResult::Timeout {
                seconds: config.timeout_secs,
            })
        }
    }
}

/// Write a [`QedInput`] to a JSON file at the given path.
///
/// Useful for debugging and for CI integration where the prover binary
/// is invoked separately.
pub fn write_qed_input_to_file(input: &QedInput, path: &PathBuf) -> Result<(), ProverError> {
    let json = serde_json::to_string_pretty(input)
        .map_err(|e| ProverError::Serialization(e.to_string()))?;
    std::fs::write(path, json).map_err(|e| ProverError::Io(e.to_string()))
}

/// Parse raw prover process output into a [`ProofResult`].
///
/// Recognises keywords in stdout (case-insensitive).
///
/// **Real qed-prover format** (from `github.com/qed-solver/prover`):
/// - `"Equivalence is provable for ..."` → [`ProofResult::Equivalent`]
/// - `"Equivalence is not provable for ..."` → [`ProofResult::NotEquivalent`]
/// - `"Trivially true!"` → [`ProofResult::Equivalent`]
///
/// **Legacy format** (our original mock output):
/// - `"equivalent"` (but not `"not equivalent"`) → [`ProofResult::Equivalent`]
/// - `"notequivalent"` or `"not equivalent"` → [`ProofResult::NotEquivalent`]
/// - `"unknown"` → [`ProofResult::Unknown`]
///
/// Falls back to exit code 0 = Equivalent if no keywords matched.
fn parse_prover_output(output: &std::process::Output) -> Result<ProofResult, ProverError> {
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    let stdout_lower = stdout.to_lowercase();

    // Real prover format: "provable" / "not provable"
    let is_provable = stdout_lower.contains("provable") && !stdout_lower.contains("not provable");
    let is_not_provable = stdout_lower.contains("not provable");

    // Legacy format: "equivalent" / "not equivalent"
    let is_equivalent = stdout_lower.contains("equivalent")
        && !stdout_lower.contains("not equivalent")
        && !stdout_lower.contains("notequivalent");
    let is_not_equivalent =
        stdout_lower.contains("notequivalent") || stdout_lower.contains("not equivalent");

    // Both formats: "trivially true", "unknown", "timed out"
    let is_trivially_true = stdout_lower.contains("trivially true");
    let is_unknown = stdout_lower.contains("unknown") || stdout_lower.contains("timed out");

    if is_provable || is_equivalent || is_trivially_true {
        Ok(ProofResult::Equivalent)
    } else if is_not_provable || is_not_equivalent {
        Ok(ProofResult::NotEquivalent {
            counterexample: extract_counterexample(&stdout),
        })
    } else if is_unknown {
        Ok(ProofResult::Unknown {
            reason: extract_reason(&stdout, &stderr),
        })
    } else if output.status.success() {
        // Exit code 0, no keyword matched — assume equivalent.
        Ok(ProofResult::Equivalent)
    } else {
        Err(ProverError::UnexpectedOutput { stdout, stderr })
    }
}

/// Structured result parsed from the qed-prover's `.result` JSON file.
///
/// The prover creates a `.result` file next to each input JSON containing
/// detailed timing and outcome information.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[non_exhaustive]
pub struct ProverFileResult {
    /// Whether the equivalence was proven.
    pub provable: bool,
    /// Whether the prover panicked during execution.
    pub panicked: bool,
    /// Whether the input used only complete (fully supported) SQL features.
    pub complete_fragment: bool,
    /// Whether the SMT solver timed out.
    pub smt_timed_out: bool,
}

/// Parse a prover `.result` JSON file for structured output.
///
/// The qed-prover creates a `.result` file adjacent to the input JSON file
/// after processing. This function reads and parses that file.
pub fn parse_result_file(path: &std::path::Path) -> Result<ProverFileResult, ProverError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| ProverError::Io(format!("failed to read .result file: {e}")))?;
    serde_json::from_str(&content)
        .map_err(|e| ProverError::Serialization(format!("failed to parse .result JSON: {e}")))
}

/// Extract a counterexample snippet from prover stdout.
fn extract_counterexample(stdout: &str) -> Option<String> {
    let lines: Vec<&str> = stdout.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let lower = line.to_lowercase();
        if lower.contains("counter") || lower.contains("example") {
            let end = std::cmp::min(i + 5, lines.len());
            return Some(lines[i..end].join("\n"));
        }
    }
    None
}

/// Build a reason string from stdout and stderr.
fn extract_reason(stdout: &str, stderr: &str) -> String {
    if !stderr.is_empty() {
        format!("{}\n{}", stdout.trim(), stderr.trim())
    } else {
        stdout.trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn make_output(stdout: &str, stderr: &str, exit_code: i32) -> std::process::Output {
        std::process::Output {
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
            status: std::process::ExitStatus::from_raw(exit_code),
        }
    }

    #[test]
    fn test_parse_equivalent() {
        let output = make_output("Equivalent\n", "", 0);
        let result = parse_prover_output(&output).unwrap();
        assert_eq!(result, ProofResult::Equivalent);
    }

    #[test]
    fn test_parse_not_equivalent() {
        let output = make_output("NotEquivalent\nCounterexample: { x = 1 }\n", "", 0);
        let result = parse_prover_output(&output).unwrap();
        assert!(matches!(result, ProofResult::NotEquivalent { .. }));
        if let ProofResult::NotEquivalent { counterexample } = &result {
            assert!(counterexample.is_some());
            assert!(counterexample.as_ref().unwrap().contains("Counterexample"));
        }
    }

    #[test]
    fn test_parse_not_equivalent_with_space() {
        let output = make_output("Not Equivalent\n", "", 0);
        let result = parse_prover_output(&output).unwrap();
        assert!(matches!(result, ProofResult::NotEquivalent { .. }));
    }

    #[test]
    fn test_parse_unknown() {
        let output = make_output("Unknown\nTimeout reached\n", "", 0);
        let result = parse_prover_output(&output).unwrap();
        assert!(matches!(result, ProofResult::Unknown { .. }));
        if let ProofResult::Unknown { reason } = &result {
            assert!(reason.contains("Timeout reached"));
        }
    }

    #[test]
    fn test_parse_unknown_with_stderr() {
        let output = make_output("Unknown\n", "resource limit exceeded\n", 0);
        let result = parse_prover_output(&output).unwrap();
        if let ProofResult::Unknown { reason } = &result {
            assert!(reason.contains("resource limit exceeded"));
        }
    }

    #[test]
    fn test_parse_unexpected_output() {
        let output = make_output("some garbage\n", "error happened\n", 1);
        let result = parse_prover_output(&output);
        assert!(matches!(result, Err(ProverError::UnexpectedOutput { .. })));
    }

    #[test]
    fn test_parse_empty_success_is_equivalent() {
        let output = make_output("", "", 0);
        let result = parse_prover_output(&output).unwrap();
        assert_eq!(result, ProofResult::Equivalent);
    }

    #[test]
    fn test_config_default() {
        let config = ProverConfig::default();
        assert_eq!(config.binary_path, PathBuf::from("qed-prover"));
        assert_eq!(config.timeout_secs, 60);
        assert!(config.workdir.is_none());
    }

    #[test]
    fn test_write_qed_input_to_file() {
        let input = QedInput {
            schemas: vec![],
            queries: [
                crate::ir::QedRelation::Scan {
                    table: "t".to_string(),
                    fields: vec![],
                },
                crate::ir::QedRelation::Scan {
                    table: "t".to_string(),
                    fields: vec![],
                },
            ],
            help: "test write".to_string(),
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.json");
        write_qed_input_to_file(&input, &path).unwrap();
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("test write"));
    }

    #[test]
    fn test_extract_counterexample_found() {
        let stdout = "Result: NotEquivalent\nCounterexample:\nx = 1\ny = 2\n";
        let ce = extract_counterexample(stdout);
        assert!(ce.is_some());
        assert!(ce.unwrap().contains("Counterexample"));
    }

    #[test]
    fn test_extract_counterexample_not_found() {
        let stdout = "Result: NotEquivalent\nNo details\n";
        let ce = extract_counterexample(stdout);
        assert!(ce.is_none());
    }

    // --- Real qed-prover output format tests ---

    #[test]
    fn test_parse_provable() {
        let output = make_output("Equivalence is provable for test.json\n", "", 0);
        let result = parse_prover_output(&output).unwrap();
        assert_eq!(result, ProofResult::Equivalent);
    }

    #[test]
    fn test_parse_not_provable() {
        let output = make_output("Equivalence is not provable for test.json\n", "", 0);
        let result = parse_prover_output(&output).unwrap();
        assert!(matches!(result, ProofResult::NotEquivalent { .. }));
        if let ProofResult::NotEquivalent { counterexample } = &result {
            assert!(counterexample.is_none());
        }
    }

    #[test]
    fn test_parse_trivially_true() {
        let output = make_output(
            "Trivially true!\nEquivalence is provable for test.json\n",
            "",
            0,
        );
        let result = parse_prover_output(&output).unwrap();
        assert_eq!(result, ProofResult::Equivalent);
    }

    #[test]
    fn test_parse_prover_with_stderr() {
        let output = make_output(
            "Equivalence is provable for test.json\n",
            "info: translating...\n",
            0,
        );
        let result = parse_prover_output(&output).unwrap();
        assert_eq!(result, ProofResult::Equivalent);
    }

    #[test]
    fn test_parse_result_file_provable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.result");
        std::fs::write(
            &path,
            r#"{"provable":true,"panicked":false,"complete_fragment":true,"smt_timed_out":false}"#,
        )
        .unwrap();
        let result = parse_result_file(&path).unwrap();
        assert!(result.provable);
        assert!(!result.panicked);
    }

    #[test]
    fn test_parse_result_file_not_provable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.result");
        std::fs::write(
            &path,
            r#"{"provable":false,"panicked":false,"complete_fragment":true,"smt_timed_out":false}"#,
        )
        .unwrap();
        let result = parse_result_file(&path).unwrap();
        assert!(!result.provable);
    }

    #[test]
    fn test_parse_result_file_missing() {
        let result = parse_result_file(std::path::Path::new("/tmp/nonexistent_12345.result"));
        assert!(result.is_err());
    }
}
