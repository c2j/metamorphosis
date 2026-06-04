//! QED prover binary harness.
//!
//! Wraps the external `qed-prover` binary as a subprocess: serializes [`QedInput`]
//! to a temp JSON file, invokes the prover, and parses its output into a
//! [`ProofResult`].
//!
//! Tests mock `std::process::Output` directly — no `qed-prover` binary required.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use crate::ir::QedInput;

/// Result of a QED equivalence proof attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProofResult {
    /// QED proved the two queries are semantically equivalent.
    Equivalent,
    /// QED found a counterexample (queries are NOT equivalent).
    NotEquivalent {
        counterexample: Option<String>,
    },
    /// QED could not determine equivalence within the timeout.
    Unknown {
        reason: String,
    },
    /// The prover process timed out.
    Timeout {
        seconds: u64,
    },
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
/// Serializes `input` to a temporary JSON file, spawns the `qed-prover` binary
/// with a timeout, and returns the parsed [`ProofResult`].
pub fn run_prover(input: &QedInput, config: &ProverConfig) -> Result<ProofResult, ProverError> {
    let json = serde_json::to_string_pretty(input)
        .map_err(|e| ProverError::Serialization(e.to_string()))?;

    let temp_dir =
        tempfile::tempdir().map_err(|e| ProverError::Io(e.to_string()))?;
    let input_path = temp_dir.path().join("input.json");
    std::fs::write(&input_path, &json).map_err(|e| ProverError::Io(e.to_string()))?;

    let (tx, rx) = mpsc::channel();
    let binary = config.binary_path.clone();
    let input_path_clone = input_path.clone();

    let handle = std::thread::spawn(move || {
        let result = std::process::Command::new(&binary)
            .arg(&input_path_clone)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(Duration::from_secs(config.timeout_secs)) {
        Ok(Ok(output)) => parse_prover_output(&output),
        Ok(Err(e)) => Err(ProverError::Process(e.to_string())),
        Err(_) => {
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
/// Recognises keywords in stdout (case-insensitive):
/// - "equivalent" (but not "not equivalent" / "notequivalent") → [`ProofResult::Equivalent`]
/// - "notequivalent" or "not equivalent" → [`ProofResult::NotEquivalent`]
/// - "unknown" → [`ProofResult::Unknown`]
///
/// Falls back to exit code 0 = Equivalent if no keywords matched.
fn parse_prover_output(output: &std::process::Output) -> Result<ProofResult, ProverError> {
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    let stdout_lower = stdout.to_lowercase();

    if stdout_lower.contains("equivalent")
        && !stdout_lower.contains("not equivalent")
        && !stdout_lower.contains("notequivalent")
    {
        Ok(ProofResult::Equivalent)
    } else if stdout_lower.contains("notequivalent") || stdout_lower.contains("not equivalent") {
        Ok(ProofResult::NotEquivalent {
            counterexample: extract_counterexample(&stdout),
        })
    } else if stdout_lower.contains("unknown") {
        Ok(ProofResult::Unknown {
            reason: extract_reason(&stdout, &stderr),
        })
    } else if output.status.success() {
        // Exit code 0, no keyword matched.
        Ok(ProofResult::Equivalent)
    } else {
        Err(ProverError::UnexpectedOutput { stdout, stderr })
    }
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
        let output = make_output(
            "NotEquivalent\nCounterexample: { x = 1 }\n",
            "",
            0,
        );
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
}
