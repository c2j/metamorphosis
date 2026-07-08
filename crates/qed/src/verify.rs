//! End-to-end verification pipeline tying schema extraction, translation,
//! and prover invocation together.

use crate::ir::{QedExpr, QedInput, QedRelation, QedSchema};
use crate::prover::{ProofResult, ProverConfig};
use crate::schema::RichSchema;
use crate::translator::AstTranslator;
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use std::collections::{HashMap, HashSet};
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
    /// Column names for the original query output (if computable).
    pub original_columns: Option<Vec<String>>,
    /// Column names for the rewritten query output (if computable).
    pub rewritten_columns: Option<Vec<String>>,
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

    let mut query1 = translator.translate(original)?;
    let mut query2 = translator.translate(rewritten)?;

    let qed_schemas = build_qed_schemas(schema);

    // Normalise output columns by name before prover invocation (Fix 4):
    // if both queries produce the same set of named columns (in any order),
    // wrap one in a Project to permute them positionally.
    normalize_output_columns(&mut query1, &mut query2, &qed_schemas);

    let orig_columns = output_column_names(&query1, &qed_schemas);
    let rewritten_columns = output_column_names(&query2, &qed_schemas);

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
        original_columns: orig_columns,
        rewritten_columns,
    })
}

/// Try to resolve the output column names of a [`QedRelation`].
///
/// Returns `None` when column names cannot be determined (e.g. raw `Values`
/// without context). Returns `Some` even if some names are synthetic (e.g.
/// `"fn_name_0"` for function-call columns).
pub(crate) fn output_column_names(rel: &QedRelation, schemas: &[QedSchema]) -> Option<Vec<String>> {
    let map: HashMap<&str, &QedSchema> = schemas.iter().map(|s| (s.name.as_str(), s)).collect();
    output_column_names_rec(rel, &map)
}

fn output_column_names_rec<'a>(
    rel: &'a QedRelation,
    schemas: &HashMap<&str, &'a QedSchema>,
) -> Option<Vec<String>> {
    match rel {
        QedRelation::Scan { table, fields } => {
            let s = schemas.get(table.as_str())?;
            Some(if fields.is_empty() {
                s.fields.clone()
            } else {
                fields.iter().map(|&i| s.fields[i].clone()).collect()
            })
        }
        QedRelation::Filter { input, .. }
        | QedRelation::Distinct { input }
        | QedRelation::QOp { input, .. } => output_column_names_rec(input, schemas),
        QedRelation::Project { exprs, input } => {
            let input_names = output_column_names_rec(input, schemas)?;
            Some(
                exprs
                    .iter()
                    .enumerate()
                    .map(|(i, expr)| match expr {
                        QedExpr::ColumnRef { index } => input_names
                            .get(*index)
                            .cloned()
                            .unwrap_or_else(|| format!("col_{i}")),
                        QedExpr::Literal { .. } => format!("literal_{i}"),
                        QedExpr::BinOp { op, .. } => format!("binop_{op}_{i}"),
                        QedExpr::UnOp { op, .. } => format!("unop_{op}_{i}"),
                        QedExpr::Function { name, .. } => format!("fn_{name}_{i}"),
                        QedExpr::Null => format!("null_{i}"),
                        QedExpr::Quantified { .. } => format!("quant_{i}"),
                    })
                    .collect(),
            )
        }
        QedRelation::Join { left, right, .. } => {
            let l = output_column_names_rec(left, schemas)?;
            let r = output_column_names_rec(right, schemas)?;
            let mut names = l;
            names.extend(r);
            Some(names)
        }
        QedRelation::Union { left, .. }
        | QedRelation::Intersect { left, .. }
        | QedRelation::Except { left, .. } => output_column_names_rec(left, schemas),
        QedRelation::Values { rows } => {
            let arity = rows.first().map_or(0, |r| r.len());
            Some((0..arity).map(|i| format!("col_{i}")).collect())
        }
        QedRelation::Aggregate { keys, aggs, input } => {
            let input_names = output_column_names_rec(input, schemas)?;
            let key_names: Vec<String> = keys
                .iter()
                .map(|&i| {
                    input_names
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| format!("key_{i}"))
                })
                .collect();
            let agg_names: Vec<String> = aggs
                .iter()
                .enumerate()
                .map(|(i, a)| format!("{}_{i}", a.func))
                .collect();
            let mut names = key_names;
            names.extend(agg_names);
            Some(names)
        }
    }
}

/// Normalise output column order between two [`QedRelation`]s by name.
///
/// If both relations produce the same set of named columns (possibly in a
/// different order), wraps `q2` in a [`QedRelation::Project`] that permutes
/// its output columns to match `q1`'s order.  This lets the Z3 solver compare
/// columns positionally even when the two SQL queries list columns differently.
///
/// Does nothing when either side has unknown column names, different arities,
/// or genuinely different column sets.
fn normalize_output_columns(q1: &mut QedRelation, q2: &mut QedRelation, schemas: &[QedSchema]) {
    let names1 = output_column_names(q1, schemas);
    let names2 = output_column_names(q2, schemas);

    let (Some(n1), Some(n2)) = (&names1, &names2) else {
        return;
    };

    if n1.len() != n2.len() {
        return;
    }
    if *n1 == *n2 {
        return; // already matching
    }

    // Check that both have the same set of column names
    let set1: HashSet<&str> = n1.iter().map(|s| s.as_str()).collect();
    let set2: HashSet<&str> = n2.iter().map(|s| s.as_str()).collect();
    if set1 != set2 {
        return;
    }

    // Build the permutation: for each position in q1's output,
    // find which position in q2's output provides that column.
    let perm: Vec<usize> = n1
        .iter()
        .map(|name| {
            n2.iter()
                .position(|n| n == name)
                .expect("set equality check ensures this succeeds")
        })
        .collect();

    // Skip identity permutation
    if perm.iter().enumerate().all(|(i, &p)| p == i) {
        return;
    }

    let old_q2 = std::mem::replace(q2, QedRelation::Values { rows: vec![] });
    *q2 = QedRelation::Project {
        exprs: perm
            .iter()
            .map(|&i| QedExpr::ColumnRef { index: i })
            .collect(),
        input: Box::new(old_q2),
    };
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
        stmts
            .into_iter()
            .next()
            .map(|si| si.statement)
            .expect("expected one statement")
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
        let ddl = parse_ddl("CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT NOT NULL)");
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
    fn test_output_column_names_scan() {
        let ddl = parse_ddl("CREATE TABLE t (a INTEGER, b TEXT, c BOOLEAN)");
        let schema = crate::schema::extract_rich_schema(&ddl);
        let qed = build_qed_schemas(&schema);

        let scan = QedRelation::Scan {
            table: "t".into(),
            fields: vec![],
        };
        let names = output_column_names(&scan, &qed).unwrap();
        assert_eq!(names, vec!["a", "b", "c"]);

        let scan2 = QedRelation::Scan {
            table: "t".into(),
            fields: vec![2, 0],
        };
        let names2 = output_column_names(&scan2, &qed).unwrap();
        assert_eq!(names2, vec!["c", "a"]);
    }

    #[test]
    fn test_output_column_names_project() {
        let ddl = parse_ddl("CREATE TABLE t (x INTEGER, y INTEGER)");
        let schema = crate::schema::extract_rich_schema(&ddl);
        let qed = build_qed_schemas(&schema);

        let scan = QedRelation::Scan {
            table: "t".into(),
            fields: vec![],
        };
        let proj = QedRelation::Project {
            exprs: vec![
                QedExpr::ColumnRef { index: 1 },
                QedExpr::ColumnRef { index: 0 },
            ],
            input: Box::new(scan),
        };
        let names = output_column_names(&proj, &qed).unwrap();
        assert_eq!(names, vec!["y", "x"]);
    }

    #[test]
    fn test_normalize_output_columns_permutes() {
        let ddl = parse_ddl("CREATE TABLE t (a INTEGER, b TEXT, c BOOLEAN)");
        let schema = crate::schema::extract_rich_schema(&ddl);
        let qed = build_qed_schemas(&schema);

        // q1: Scan (all cols in order) → [a, b, c]
        let mut q1 = QedRelation::Scan {
            table: "t".into(),
            fields: vec![],
        };
        // q2: Project reordering → [c, a, b]
        let mut q2 = QedRelation::Project {
            exprs: vec![
                QedExpr::ColumnRef { index: 2 },
                QedExpr::ColumnRef { index: 0 },
                QedExpr::ColumnRef { index: 1 },
            ],
            input: Box::new(QedRelation::Scan {
                table: "t".into(),
                fields: vec![],
            }),
        };

        normalize_output_columns(&mut q1, &mut q2, &qed);

        // After normalization q2 should be wrapped in a Project that reorders to [a, b, c]
        let names = output_column_names(&q2, &qed).unwrap();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_verify_rewrite_different_column_order_is_equivalent() {
        let ddl = parse_ddl("CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT NOT NULL, c BOOLEAN)");
        let schema = crate::schema::extract_rich_schema(&ddl);
        let config = ProverConfig::default();

        // Same columns, different order — should be Equivalent after Fix 4 normalization
        let original = parse_single("SELECT a, b, c FROM t");
        let rewritten = parse_single("SELECT c, a, b FROM t");

        let result = verify_rewrite("reorder-test", &original, &rewritten, &schema, &config);
        assert!(result.is_ok(), "Expected Ok, got: {result:?}");
        let vr = result.unwrap();
        assert!(
            matches!(vr.proof, crate::prover::ProofResult::Equivalent),
            "SELECT a,b,c vs SELECT c,a,b should be Equivalent after normalization, got: {:?}",
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

    #[test]
    fn test_bool_projection_gt_equivalent() {
        let ddl = parse_ddl("CREATE TABLE t (a INTEGER PRIMARY KEY, b INTEGER NOT NULL)");
        let schema = crate::schema::extract_rich_schema(&ddl);
        let config = ProverConfig::default();

        let original = parse_single("SELECT a > 5 FROM t");
        let rewritten = parse_single("SELECT a >= 6 FROM t");

        let result = verify_rewrite("bool-proj-gt", &original, &rewritten, &schema, &config);
        assert!(result.is_ok(), "Expected Ok, got: {result:?}");
        let vr = result.unwrap();
        assert!(
            matches!(vr.proof, crate::prover::ProofResult::Equivalent),
            "a > 5 vs a >= 6 (bool projection) should be Equivalent, got: {:?}",
            vr.proof
        );
    }

    #[test]
    fn test_bool_eq_projection_equivalent() {
        let ddl = parse_ddl("CREATE TABLE t (a INTEGER PRIMARY KEY, b INTEGER NOT NULL)");
        let schema = crate::schema::extract_rich_schema(&ddl);
        let config = ProverConfig::default();

        let original = parse_single("SELECT a = b FROM t");
        let rewritten = parse_single("SELECT b = a FROM t");

        let result = verify_rewrite("bool-proj-eq", &original, &rewritten, &schema, &config);
        assert!(result.is_ok(), "Expected Ok, got: {result:?}");
        let vr = result.unwrap();
        assert!(
            matches!(vr.proof, crate::prover::ProofResult::Equivalent),
            "a = b vs b = a (bool projection) should be Equivalent, got: {:?}",
            vr.proof
        );
    }

    #[test]
    fn test_substr_eq_vs_like_not_equivalent() {
        let ddl = parse_ddl("CREATE TABLE t (id INTEGER PRIMARY KEY)");
        let schema = crate::schema::extract_rich_schema(&ddl);
        let config = ProverConfig::default();

        let sql_a = "SELECT substr('000000000000abxyzcd',1,17) = lpad('ab%cd',17,'0') FROM t";
        let sql_b = "SELECT '000000000000abxyzcd' like lpad('ab%cd',17,'0') || '%' FROM t";
        let original = parse_single(sql_a);
        let rewritten = parse_single(sql_b);

        let result = verify_rewrite("substr-eq-vs-like", &original, &rewritten, &schema, &config);
        assert!(result.is_ok(), "Expected Ok, got: {result:?}");
        let vr = result.unwrap();
        assert!(
            matches!(vr.proof, crate::prover::ProofResult::NotEquivalent { .. }),
            "= vs LIKE should be NotEquivalent, got: {:?}",
            vr.proof
        );
    }
}
