//! End-to-end verification pipeline tying schema extraction, translation,
//! and prover invocation together.

use crate::ir::{QedInput, QedSchema};
use crate::prover::{ProofResult, ProverConfig};
use crate::schema::RichSchema;
use crate::translator::AstTranslator;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use std::time::Instant;
use thiserror::Error;

/// Errors from the verification pipeline.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum VerifyError {
    /// AST → QED translation failed.
    #[error("translation failed: {0}")]
    Translation(#[from] crate::translator::TranslateError),
    /// Prover invocation failed.
    #[error("prover error: {0}")]
    Prover(#[from] crate::prover::ProverError),
    /// The rule does not apply to the given statement.
    #[error("rule not applicable to the given statement")]
    RuleNotApplicable,
}

/// Verification result for a single rewrite rule test case.
#[derive(Debug)]
pub struct VerificationResult {
    /// Rule that produced the rewrite.
    pub rule_id: String,
    /// Original SQL (formatted).
    pub original_sql: String,
    /// Rewritten SQL (formatted).
    pub rewritten_sql: String,
    /// QED proof result.
    pub proof: ProofResult,
    /// Time taken in milliseconds.
    pub elapsed_ms: u64,
}

/// Verify that a rewrite preserves semantic equivalence using QED.
///
/// Translates both the original and rewritten statements to QED Relations,
/// builds a [`QedInput`] with schema constraints, converts to the prover's
/// native format, and invokes the prover.
///
/// `schema_name_map` optionally maps table names to qualified names (e.g.
/// `"users"` → `"PUBLIC.users"`) for the prover. When `None`, table names
/// are used as-is.
pub fn verify_rewrite(
    rule_id: &str,
    original: &Statement,
    rewritten: &Statement,
    schema: &RichSchema,
    prover_config: &ProverConfig,
) -> Result<VerificationResult, VerifyError> {
    verify_rewrite_with_names(rule_id, original, rewritten, schema, prover_config, None)
}

/// Like [`verify_rewrite`], but accepts an optional schema name qualification map.
pub fn verify_rewrite_with_names(
    rule_id: &str,
    original: &Statement,
    rewritten: &Statement,
    schema: &RichSchema,
    prover_config: &ProverConfig,
    schema_name_map: Option<&std::collections::HashMap<String, String>>,
) -> Result<VerificationResult, VerifyError> {
    let translator = AstTranslator::new(schema);
    let start = Instant::now();

    let query1 = translator.translate(original)?;
    let query2 = translator.translate(rewritten)?;

    let qed_schemas = build_qed_schemas(schema);
    let input = QedInput {
        schemas: qed_schemas,
        queries: [query1, query2],
        help: format!("Verify semantic equivalence for rule '{rule_id}'"),
    };

    let proof = crate::prover::run_prover(&input, prover_config, schema_name_map)?;
    let elapsed = start.elapsed().as_millis() as u64;

    Ok(VerificationResult {
        rule_id: rule_id.to_string(),
        original_sql: SqlFormatter::new().format_statement(original),
        rewritten_sql: SqlFormatter::new().format_statement(rewritten),
        proof,
        elapsed_ms: elapsed,
    })
}

/// Convert a [`RichSchema`] to QED prover's schema format.
pub(crate) fn build_qed_schemas(schema: &RichSchema) -> Vec<QedSchema> {
    schema
        .tables
        .iter()
        .map(|(name, table)| {
            let fields: Vec<String> = table.columns.iter().map(|c| c.name.clone()).collect();
            let types: Vec<String> = table.columns.iter().map(|c| c.data_type.clone()).collect();
            let nullable: Vec<bool> = table.columns.iter().map(|c| c.nullable).collect();
            let key: Vec<usize> = table
                .constraints
                .primary_key
                .iter()
                .filter_map(|col| table.column_index(col))
                .collect();
            let guaranteed: Vec<String> = table
                .constraints
                .check
                .iter()
                .map(|c| c.expression.clone())
                .collect();

            QedSchema {
                name: name.clone(),
                types,
                key,
                nullable,
                guaranteed,
                fields,
            }
        })
        .collect()
}

/// Verify equivalence of a batch of test cases for a rule.
///
/// For each `(original, rewritten)` pair, translates both statements,
/// builds a [`QedInput`], and invokes the prover. Returns results for
/// each test case independently.
pub fn verify_batch(
    rule_id: &str,
    test_pairs: &[(Statement, Statement)],
    schema: &RichSchema,
    prover_config: &ProverConfig,
) -> Vec<Result<VerificationResult, VerifyError>> {
    test_pairs
        .iter()
        .map(|(original, rewritten)| {
            verify_rewrite(rule_id, original, rewritten, schema, prover_config)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ddl(sql: &str) -> Vec<Statement> {
        let (stmts, _) = ogsql_parser::Parser::parse_sql(sql);
        stmts.into_iter().map(|si| si.statement).collect()
    }

    fn parse_single(sql: &str) -> Statement {
        let (stmts, _) = ogsql_parser::Parser::parse_sql(sql);
        stmts.into_iter().next().map(|si| si.statement).expect("expected one statement")
    }

    #[test]
    fn test_build_qed_schemas_from_ddl() {
        let ddl = parse_ddl(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR(100) NOT NULL, email VARCHAR(200))",
        );
        let schema = crate::schema::extract_rich_schema(&ddl);
        let qed_schemas = build_qed_schemas(&schema);

        assert_eq!(qed_schemas.len(), 1);
        assert_eq!(qed_schemas[0].name, "users");
        assert_eq!(qed_schemas[0].fields, vec!["id", "name", "email"]);
        assert_eq!(qed_schemas[0].key, vec![0]);
        assert_eq!(qed_schemas[0].nullable, vec![false, false, true]);
    }

    #[test]
    fn test_verify_rewrite_identity() {
        let ddl = parse_ddl(
            "CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT NOT NULL)",
        );
        let schema = crate::schema::extract_rich_schema(&ddl);

        let original = parse_single("SELECT a, b FROM t WHERE a = 1");
        let rewritten = parse_single("SELECT a, b FROM t WHERE a = 1");

        // Use a prover config that will fail (no binary available), but
        // verify the pipeline up to the prover call works correctly.
        let config = ProverConfig {
            binary_path: std::path::PathBuf::from("nonexistent-qed-prover-binary"),
            timeout_secs: 5,
            workdir: None,
        };

        let result = verify_rewrite("identity-test", &original, &rewritten, &schema, &config);
        // Z3 solver handles this directly — identity queries are Equivalent.
        // The binary prover is only a fallback if Z3 fails.
        assert!(result.is_ok(), "Expected Ok, got: {result:?}");
        let vr = result.unwrap();
        assert!(
            matches!(vr.proof, crate::prover::ProofResult::Equivalent),
            "Expected Equivalent, got: {:?}",
            vr.proof
        );
    }

    #[test]
    fn test_verify_batch_empty() {
        let ddl = parse_ddl("CREATE TABLE t (id INTEGER)");
        let schema = crate::schema::extract_rich_schema(&ddl);
        let config = ProverConfig::default();

        let results = verify_batch("test", &[], &schema, &config);
        assert!(results.is_empty());
    }
}
