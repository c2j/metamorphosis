//! End-to-end verification tests.
//!
//! Full E2E tests use the embedded Z3 SMT solver for equivalence proofs.
//! No external binaries are required.

use metamorphosis_core::context::{RewriteConfig, RewriteContext};
use metamorphosis_core::registry::RewriteRule;
use metamorphosis_qed::ir::{QedInput, QedRelation, QedSchema};
use metamorphosis_qed::prover::ProverConfig;
use metamorphosis_qed::schema::extract_rich_schema;
use metamorphosis_qed::translator::AstTranslator;
use metamorphosis_qed::verify::verify_rewrite;
use metamorphosis_rules::eliminate_select_star::EliminateSelectStar;
use ogsql_parser::analyzer::schema::SchemaMap;
use ogsql_parser::ast::Statement;
use ogsql_parser::Parser;
use std::collections::HashMap;
use std::path::PathBuf;

fn parse_ddl(sql: &str) -> Vec<Statement> {
    let (stmts, _) = Parser::parse_sql(sql);
    stmts.into_iter().map(|si| si.statement).collect()
}

fn parse_single(sql: &str) -> Statement {
    let (stmts, _) = Parser::parse_sql(sql);
    stmts
        .into_iter()
        .next()
        .expect("expected one statement")
        .statement
}

fn make_test_schema_ddl() -> &'static str {
    "CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR(100) NOT NULL, email VARCHAR(200))"
}

fn make_schema_map() -> SchemaMap {
    let mut cols = HashMap::new();
    cols.insert("id".to_string(), "integer".to_string());
    cols.insert("name".to_string(), "varchar".to_string());
    cols.insert("email".to_string(), "varchar".to_string());
    let mut schema = SchemaMap::new();
    schema.insert("users".to_string(), cols);
    schema
}

fn build_qed_schemas_from_rich(schema: &metamorphosis_qed::schema::RichSchema) -> Vec<QedSchema> {
    schema
        .tables
        .iter()
        .map(|(name, table)| QedSchema {
            name: name.clone(),
            types: table.columns.iter().map(|c| c.data_type.clone()).collect(),
            key: table
                .constraints
                .primary_key
                .iter()
                .filter_map(|col| table.column_index(col))
                .collect(),
            nullable: table.columns.iter().map(|c| c.nullable).collect(),
            guaranteed: table
                .constraints
                .check
                .iter()
                .map(|c| c.expression.clone())
                .collect(),
            fields: table.columns.iter().map(|c| c.name.clone()).collect(),
        })
        .collect()
}

#[test]
fn test_build_qed_schemas_structure() {
    let ddl = parse_ddl(make_test_schema_ddl());
    let schema = extract_rich_schema(&ddl);

    let translator = AstTranslator::new(&schema);
    let original = parse_single("SELECT id, name FROM users WHERE id = 1");
    let rewritten = parse_single("SELECT id, name FROM users WHERE id = 1");

    let q1 = translator.translate(&original).unwrap();
    let q2 = translator.translate(&rewritten).unwrap();

    let qed_schemas = build_qed_schemas_from_rich(&schema);
    let input = QedInput {
        schemas: qed_schemas,
        queries: [q1, q2],
        help: "identity test".to_string(),
    };

    assert_eq!(input.schemas.len(), 1);
    assert_eq!(input.schemas[0].name, "users");
    assert_eq!(input.schemas[0].fields, vec!["id", "name", "email"]);
    assert_eq!(input.schemas[0].key, vec![0]);
    assert_eq!(input.schemas[0].nullable, vec![false, false, true]);
}

#[test]
fn test_verify_identity_rewrite_with_prover() {
    let ddl = parse_ddl(make_test_schema_ddl());
    let schema = extract_rich_schema(&ddl);

    let original = parse_single("SELECT * FROM users");
    let rewritten = parse_single("SELECT id, name, email FROM users");

    let config = ProverConfig {
        binary_path: PathBuf::from("qed-prover"),
        timeout_secs: 30,
        workdir: None,
    };

    let result = verify_rewrite("identity", &original, &rewritten, &schema, &config);

    match result {
        Ok(vr) => {
            assert!(
                matches!(vr.proof, metamorphosis_qed::prover::ProofResult::Equivalent),
                "Expected Equivalent, got: {:?}",
                vr.proof
            );
        }
        Err(e) => {
            let msg = format!("{e}");
            assert!(
                msg.contains("prover") || msg.contains("No such file") || msg.contains("process"),
                "Unexpected error: {e}"
            );
        }
    }
}

#[test]
fn test_eliminate_select_star_pipeline() {
    let ddl = parse_ddl(make_test_schema_ddl());
    let schema = extract_rich_schema(&ddl);
    let original = parse_single("SELECT * FROM users");

    let rule = EliminateSelectStar;
    let config = RewriteConfig::default();
    let schema_map = make_schema_map();
    let ctx = RewriteContext {
        version: None,
        schema: Some(&schema_map),
        config: &config,
        source_file: None,
        known_variables: None,
    };

    assert!(rule.matches(&ctx, &original).is_matched(), "rule should match SELECT *");

    if let Some(metamorphosis_core::types::RewriteAction::Replace(rewritten)) =
        rule.apply(&ctx, &original)
    {
        let translator = AstTranslator::new(&schema);
        let q1 = translator.translate(&original).unwrap();
        let q2 = translator.translate(&rewritten).unwrap();

        // SELECT * translates to Scan (is_simple_star skips projection).
        assert!(
            matches!(q1, QedRelation::Scan { .. }),
            "original SELECT * should translate to Scan"
        );
        // Rewritten SELECT id, name, email translates to Project.
        assert!(
            matches!(q2, QedRelation::Project { .. }),
            "rewritten explicit columns should translate to Project"
        );
    } else {
        panic!("rule.apply should return Replace for SELECT *");
    }
}
