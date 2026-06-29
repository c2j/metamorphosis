use std::process::Command;
use std::str;

/// Write a string to a file inside a directory. Returns the file path.
fn write_file_in(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

/// Write a string to a file inside a temp directory. Returns the file path.
fn write_file(dir: &tempfile::TempDir, name: &str, content: &str) -> std::path::PathBuf {
    write_file_in(dir.path(), name, content)
}

/// Write a JSON schema file in the flat metamorphosis format:
/// `{"table": {"column": "type"}}`.
fn write_json_schema(
    dir: &tempfile::TempDir,
    name: &str,
    tables: &[(&str, &[(&str, &str)])],
) -> std::path::PathBuf {
    let mut schema = std::collections::HashMap::new();
    for (table, cols) in tables {
        let mut col_map = std::collections::HashMap::new();
        for (col, ty) in *cols {
            col_map.insert(col.to_string(), ty.to_string());
        }
        schema.insert(table.to_string(), col_map);
    }
    let json = serde_json::to_string(&schema).unwrap();
    write_file(dir, name, &json)
}

/// Verify that `--help` for the `verify` subcommand produces usage output.
#[test]
fn test_verify_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("verify")
        .arg("--help")
        .output()
        .expect("Failed to run metamorphosis verify --help");

    assert!(output.status.success(), "verify --help should succeed");
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("Usage") || stdout.contains("schema"),
        "stdout should contain usage or schema info, got: {}",
        stdout
    );
}

/// Verify that omitting both `--schema` and `--sql-dir` produces a runtime error.
#[test]
fn test_verify_missing_schema_and_dir_exits_error() {
    let dir = tempfile::tempdir().unwrap();
    let orig = write_file(&dir, "orig.sql", "SELECT id FROM users");
    let rewritten = write_file(&dir, "rewritten.sql", "SELECT id FROM users");

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("verify")
        .arg(orig.to_str().unwrap())
        .arg(rewritten.to_str().unwrap())
        .output()
        .expect("Failed to run metamorphosis verify");

    assert!(
        !output.status.success(),
        "verify without schema should fail"
    );
    let stderr = str::from_utf8(&output.stderr).unwrap();
    assert!(
        stderr.contains("required") || stderr.contains("--schema"),
        "stderr should mention --schema or --sql-dir requirement, got: {}",
        stderr
    );
}

/// Verify that providing both `--schema` and `--sql-dir` is rejected by clap.
#[test]
fn test_verify_schema_and_dir_mutually_exclusive() {
    let dir = tempfile::tempdir().unwrap();
    let orig = write_file(&dir, "orig.sql", "SELECT id FROM users");
    let rewritten = write_file(&dir, "rewritten.sql", "SELECT id FROM users");
    let schema = write_json_schema(&dir, "schema.json", &[("users", &[("id", "integer")])]);

    let ddl_dir = dir.path().join("ddl");
    std::fs::create_dir(&ddl_dir).unwrap();
    write_file_in(&ddl_dir, "empty.sql", "CREATE TABLE users (id INTEGER);");

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("verify")
        .arg(orig.to_str().unwrap())
        .arg(rewritten.to_str().unwrap())
        .arg("--schema")
        .arg(schema.to_str().unwrap())
        .arg("--sql-dir")
        .arg(ddl_dir.to_str().unwrap())
        .output()
        .expect("Failed to run metamorphosis verify");

    assert!(
        !output.status.success(),
        "mutually exclusive flags should fail"
    );
    let stderr = str::from_utf8(&output.stderr).unwrap();
    assert!(
        stderr.contains("cannot be used with")
            || stderr.contains("mutually exclusive")
            || stderr.contains("conflict"),
        "stderr should mention the mutual exclusion, got: {}",
        stderr
    );
}

/// Verify identity equivalence (same SQL in both files) with JSON schema using QED.
#[test]
fn test_verify_identity_qed_with_json_schema() {
    let dir = tempfile::tempdir().unwrap();
    let sql = "SELECT id, name FROM users WHERE id = 1";
    let orig = write_file(&dir, "orig.sql", sql);
    let rewritten = write_file(&dir, "rewritten.sql", sql);
    let schema = write_json_schema(
        &dir,
        "schema.json",
        &[("users", &[("id", "integer"), ("name", "varchar(100)")])],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("verify")
        .arg(orig.to_str().unwrap())
        .arg(rewritten.to_str().unwrap())
        .arg("--schema")
        .arg(schema.to_str().unwrap())
        .output()
        .expect("Failed to run metamorphosis verify");

    assert!(
        output.status.success(),
        "verify should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("Equivalent"),
        "stdout should contain 'Equivalent', got: {}",
        stdout
    );
}

/// Verify identity equivalence using a DDL directory with QED.
#[test]
fn test_verify_identity_qed_with_sql_dir() {
    let dir = tempfile::tempdir().unwrap();
    let sql = "SELECT id, name FROM users WHERE id = 1";
    let orig = write_file(&dir, "orig.sql", sql);
    let rewritten = write_file(&dir, "rewritten.sql", sql);

    let ddl_dir = dir.path().join("ddl");
    std::fs::create_dir(&ddl_dir).unwrap();
    write_file_in(
        &ddl_dir,
        "schema.sql",
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR(100) NOT NULL);",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("verify")
        .arg(orig.to_str().unwrap())
        .arg(rewritten.to_str().unwrap())
        .arg("--sql-dir")
        .arg(ddl_dir.to_str().unwrap())
        .output()
        .expect("Failed to run metamorphosis verify");

    assert!(
        output.status.success(),
        "verify --sql-dir should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("Equivalent"),
        "stdout should contain 'Equivalent', got: {}",
        stdout
    );
}

/// Verify SELECT * expansion is equivalent to explicit column list (QED).
#[test]
fn test_verify_select_star_expansion_qed() {
    let dir = tempfile::tempdir().unwrap();
    let orig = write_file(&dir, "orig.sql", "SELECT * FROM users WHERE id = 1");
    let rewritten = write_file(
        &dir,
        "rewritten.sql",
        "SELECT id, name FROM users WHERE id = 1",
    );
    let schema = write_json_schema(
        &dir,
        "schema.json",
        &[("users", &[("id", "integer"), ("name", "varchar(100)")])],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("verify")
        .arg(orig.to_str().unwrap())
        .arg(rewritten.to_str().unwrap())
        .arg("--schema")
        .arg(schema.to_str().unwrap())
        .output()
        .expect("Failed to run metamorphosis verify");

    assert!(
        output.status.success(),
        "verify SELECT * expansion should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("Equivalent"),
        "SELECT * and explicit columns should be equivalent, got: {}",
        stdout
    );
}

/// Verify `a BETWEEN 5 AND 5` collapses to `a = 5` (QED).
#[test]
fn test_verify_between_collapsed_to_eq_qed() {
    let dir = tempfile::tempdir().unwrap();
    let orig = write_file(&dir, "orig.sql", "SELECT a FROM t WHERE a BETWEEN 5 AND 5");
    let rewritten = write_file(&dir, "rewritten.sql", "SELECT a FROM t WHERE a = 5");
    let schema = write_json_schema(
        &dir,
        "schema.json",
        &[("t", &[("a", "integer"), ("b", "integer")])],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("verify")
        .arg(orig.to_str().unwrap())
        .arg(rewritten.to_str().unwrap())
        .arg("--schema")
        .arg(schema.to_str().unwrap())
        .output()
        .expect("Failed to run metamorphosis verify");

    assert!(
        output.status.success(),
        "verify BETWEEN -> = should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("Equivalent"),
        "BETWEEN 5 AND 5 should be equivalent to = 5, got: {}",
        stdout
    );
}

/// Verify identity equivalence using the VeriEQL engine.
#[test]
fn test_verify_identity_verieql() {
    let dir = tempfile::tempdir().unwrap();
    let sql = "SELECT id FROM users";
    let orig = write_file(&dir, "orig.sql", sql);
    let rewritten = write_file(&dir, "rewritten.sql", sql);
    let schema = write_json_schema(
        &dir,
        "schema.json",
        &[("users", &[("id", "integer"), ("name", "varchar(100)")])],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("verify")
        .arg(orig.to_str().unwrap())
        .arg(rewritten.to_str().unwrap())
        .arg("--schema")
        .arg(schema.to_str().unwrap())
        .arg("--engine")
        .arg("verieql")
        .output()
        .expect("Failed to run metamorphosis verify --engine verieql");

    assert!(
        output.status.success(),
        "verify verieql should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("Equivalent"),
        "VeriEQL should find identity equivalent, got: {}",
        stdout
    );
}

/// Verify JSON output format contains a valid JSON object with `"result": "Equivalent"`.
#[test]
fn test_verify_json_output_format() {
    let dir = tempfile::tempdir().unwrap();
    let sql = "SELECT id, name FROM users WHERE id = 1";
    let orig = write_file(&dir, "orig.sql", sql);
    let rewritten = write_file(&dir, "rewritten.sql", sql);
    let schema = write_json_schema(
        &dir,
        "schema.json",
        &[("users", &[("id", "integer"), ("name", "varchar(100)")])],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("verify")
        .arg(orig.to_str().unwrap())
        .arg(rewritten.to_str().unwrap())
        .arg("--schema")
        .arg(schema.to_str().unwrap())
        .arg("-o")
        .arg("json")
        .output()
        .expect("Failed to run metamorphosis verify -o json");

    assert!(
        output.status.success(),
        "verify -o json should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    let value: serde_json::Value =
        serde_json::from_str(stdout).expect("stdout should be valid JSON");
    assert_eq!(
        value["result"], "Equivalent",
        "JSON result should be Equivalent, got: {}",
        stdout
    );
}

/// Verify EXISTS-to-DISTINCT-JOIN equivalence WITH PK info in DDL (baseline).
///
/// With PRIMARY KEY declared in DDL, QED can prove the DISTINCT JOIN
/// preserves bag semantics. This establishes the passing baseline that
/// must remain after the #39 schema protocol upgrade.
#[test]
fn test_verify_exists_to_distinct_join_with_pk_ddl() {
    let dir = tempfile::tempdir().unwrap();
    let orig = write_file(
        &dir,
        "orig.sql",
        "SELECT o.order_id FROM orders o WHERE EXISTS (SELECT 1 FROM users u WHERE u.id = o.user_id)",
    );
    let rewritten = write_file(
        &dir,
        "rewritten.sql",
        "SELECT DISTINCT o.order_id FROM orders o JOIN users u ON u.id = o.user_id",
    );

    let ddl_dir = dir.path().join("ddl");
    std::fs::create_dir(&ddl_dir).unwrap();
    write_file_in(
        &ddl_dir,
        "schema.sql",
        "CREATE TABLE orders (order_id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount NUMERIC);\n\
         CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR(100) NOT NULL);",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("verify")
        .arg(orig.to_str().unwrap())
        .arg(rewritten.to_str().unwrap())
        .arg("--sql-dir")
        .arg(ddl_dir.to_str().unwrap())
        .output()
        .expect("Failed to run metamorphosis verify");

    assert!(
        output.status.success(),
        "verify EXISTS -> DISTINCT JOIN with PK should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("Equivalent"),
        "EXISTS -> DISTINCT JOIN with PK info should be Equivalent, got: {}",
        stdout
    );
}

/// Verify EXISTS-to-JOIN (without DISTINCT) is NOT Equivalent with flat JSON schema.
///
/// # Gap documented for issue #39
///
/// The flat JSON schema format (`{"table": {"col": "type"}}`) carries no
/// PRIMARY KEY information. Without PK info, QED cannot prove that
/// `users.id` is unique, so it cannot prove that the JOIN (without DISTINCT)
/// preserves bag semantics relative to EXISTS. The result will be either
/// `Not Equivalent` or `Unknown`, but NOT `Equivalent`.
///
/// After #39 implements the new schema protocol with `primary_key` support
/// in JSON schemas, this same test with a PK-annotated schema should
/// return Equivalent (matching test 10).
#[test]
fn test_verify_exists_to_join_json_schema_no_pk() {
    let dir = tempfile::tempdir().unwrap();
    let orig = write_file(
        &dir,
        "orig.sql",
        "SELECT o.order_id FROM orders o WHERE EXISTS (SELECT 1 FROM users u WHERE u.id = o.user_id)",
    );
    let rewritten = write_file(
        &dir,
        "rewritten.sql",
        "SELECT o.order_id FROM orders o JOIN users u ON u.id = o.user_id",
    );

    let schema = write_json_schema(
        &dir,
        "schema.json",
        &[
            (
                "orders",
                &[
                    ("order_id", "integer"),
                    ("user_id", "integer"),
                    ("amount", "numeric"),
                ],
            ),
            ("users", &[("id", "integer"), ("name", "varchar(100)")]),
        ],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("verify")
        .arg(orig.to_str().unwrap())
        .arg(rewritten.to_str().unwrap())
        .arg("--schema")
        .arg(schema.to_str().unwrap())
        .output()
        .expect("Failed to run metamorphosis verify");

    assert!(
        output.status.success(),
        "verify command itself should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        !stdout.contains("Equivalent"),
        "Without PK info, equivalent JOIN should NOT be provable. This is the #39 gap. Got: {}",
        stdout
    );
}

/// Verify IN-subquery-to-DISTINCT-JOIN equivalence WITH PK info in DDL.
#[test]
fn test_verify_in_subquery_to_distinct_join_with_pk_ddl() {
    let dir = tempfile::tempdir().unwrap();
    let orig = write_file(
        &dir,
        "orig.sql",
        "SELECT o.order_id FROM orders o WHERE o.user_id IN (SELECT id FROM users)",
    );
    let rewritten = write_file(
        &dir,
        "rewritten.sql",
        "SELECT DISTINCT o.order_id FROM orders o JOIN users u ON o.user_id = u.id",
    );

    let ddl_dir = dir.path().join("ddl");
    std::fs::create_dir(&ddl_dir).unwrap();
    write_file_in(
        &ddl_dir,
        "schema.sql",
        "CREATE TABLE orders (order_id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, amount NUMERIC);\n\
         CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR(100) NOT NULL);",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("verify")
        .arg(orig.to_str().unwrap())
        .arg(rewritten.to_str().unwrap())
        .arg("--sql-dir")
        .arg(ddl_dir.to_str().unwrap())
        .output()
        .expect("Failed to run metamorphosis verify");

    assert!(
        output.status.success(),
        "verify IN -> DISTINCT JOIN with PK should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("Equivalent"),
        "IN -> DISTINCT JOIN with PK info should be Equivalent, got: {}",
        stdout
    );
}

/// Verify EXISTS→JOIN equivalence with NEW-format JSON schema containing primary_key.
///
/// This is the #39 fix test: with `primary_key` declared in the JSON schema,
/// QED can now prove the DISTINCT JOIN preserves bag semantics — matching
/// the DDL-with-PK result from test_verify_exists_to_distinct_join_with_pk_ddl.
#[test]
fn test_verify_exists_to_distinct_join_with_json_pk() {
    let dir = tempfile::tempdir().unwrap();
    let orig = write_file(
        &dir,
        "orig.sql",
        "SELECT o.order_id FROM orders o WHERE EXISTS (SELECT 1 FROM users u WHERE u.id = o.user_id)",
    );
    let rewritten = write_file(
        &dir,
        "rewritten.sql",
        "SELECT DISTINCT o.order_id FROM orders o JOIN users u ON u.id = o.user_id",
    );

    // New-format JSON schema with primary_key declarations
    let schema_json = r#"{
        "orders": {
            "columns": {"order_id": "integer", "user_id": "integer", "amount": "numeric"},
            "primary_key": ["order_id"]
        },
        "users": {
            "columns": {"id": "integer", "name": "varchar(100)"},
            "primary_key": ["id"]
        }
    }"#;
    let schema_path = write_file(&dir, "schema_pk.json", schema_json);

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("verify")
        .arg(orig.to_str().unwrap())
        .arg(rewritten.to_str().unwrap())
        .arg("--schema")
        .arg(schema_path.to_str().unwrap())
        .output()
        .expect("Failed to run metamorphosis verify");

    assert!(
        output.status.success(),
        "verify with PK JSON schema should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("Equivalent"),
        "EXISTS -> DISTINCT JOIN with PK in JSON schema should be Equivalent (#39 fix). Got: {}",
        stdout
    );
}

/// Verify that legacy-format JSON schema still works with VeriEQL engine
/// (backward compatibility after #39 new-format support).
#[test]
fn test_verify_legacy_schema_still_works_verieql() {
    let dir = tempfile::tempdir().unwrap();
    let sql = "SELECT id FROM users";
    let orig = write_file(&dir, "orig.sql", sql);
    let rewritten = write_file(&dir, "rewritten.sql", sql);
    let schema = write_json_schema(
        &dir,
        "schema.json",
        &[("users", &[("id", "integer"), ("name", "varchar(100)")])],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("verify")
        .arg(orig.to_str().unwrap())
        .arg(rewritten.to_str().unwrap())
        .arg("--schema")
        .arg(schema.to_str().unwrap())
        .arg("--engine")
        .arg("verieql")
        .output()
        .expect("Failed to run metamorphosis verify");

    assert!(
        output.status.success(),
        "legacy schema with verieql should still work. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("Equivalent"),
        "legacy schema should still produce Equivalent. Got: {}",
        stdout
    );
}
