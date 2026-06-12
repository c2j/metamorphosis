use std::process::Command;
use std::str;

/// End-to-end: verify that the CLI binary prints help message.
#[test]
fn test_cli_rewrite_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("rewrite")
        .arg("--help")
        .output()
        .expect("Failed to run metamorphosis rewrite --help");

    assert!(output.status.success(), "rewrite --help should succeed");
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        !stdout.contains('*'),
        "Output should NOT contain '/*', got: {}",
        stdout
    );
}

/// End-to-end: --file still works as before for rewrite.
#[test]
fn test_cli_rewrite_with_file_flag() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let sql_path = dir.path().join("query.sql");
    let schema_path = dir.path().join("schema.json");

    let mut f = std::fs::File::create(&sql_path).unwrap();
    f.write_all(b"SELECT * FROM users WHERE id = 1").unwrap();

    let mut schema = std::collections::HashMap::new();
    let mut cols = std::collections::HashMap::new();
    cols.insert("id".to_string(), "integer".to_string());
    cols.insert("name".to_string(), "varchar".to_string());
    schema.insert("users".to_string(), cols);
    let schema_json = serde_json::to_string(&schema).unwrap();
    let mut f = std::fs::File::create(&schema_path).unwrap();
    f.write_all(schema_json.as_bytes()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("rewrite")
        .arg("--file")
        .arg(sql_path.to_str().unwrap())
        .arg("--schema")
        .arg(schema_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "rewrite --file should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("id"),
        "Should contain 'id', got: {}",
        stdout
    );
    assert!(
        stdout.contains("name"),
        "Should contain 'name', got: {}",
        stdout
    );
}

/// End-to-end: rewrite a simple SQL file with SELECT *.
#[test]
fn test_cli_rewrite_select_star() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let sql_path = dir.path().join("query.sql");
    let schema_path = dir.path().join("schema.json");

    let mut f = std::fs::File::create(&sql_path).unwrap();
    f.write_all(b"SELECT * FROM users WHERE id = 1").unwrap();

    let mut schema = std::collections::HashMap::new();
    let mut cols = std::collections::HashMap::new();
    cols.insert("id".to_string(), "integer".to_string());
    cols.insert("name".to_string(), "varchar".to_string());
    schema.insert("users".to_string(), cols);
    let schema_json = serde_json::to_string(&schema).unwrap();
    let mut f = std::fs::File::create(&schema_path).unwrap();
    f.write_all(schema_json.as_bytes()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("rewrite")
        .arg("--file")
        .arg(sql_path.to_str().unwrap())
        .arg("--schema")
        .arg(schema_path.to_str().unwrap())
        .output()
        .expect("Failed to run metamorphosis rewrite");

    assert!(
        output.status.success(),
        "rewrite should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("id"),
        "Output should contain column 'id', got: {}",
        stdout
    );
    assert!(
        stdout.contains("name"),
        "Output should contain column 'name', got: {}",
        stdout
    );
    assert!(
        !stdout.contains('*'),
        "Output should NOT contain '/*', got: {}",
        stdout
    );
}

/// End-to-end: verify suggest command produces output for a query with multiple equality conditions.
#[test]
fn test_cli_suggest_duplicate_keys() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let sql_path = dir.path().join("query.sql");

    let mut f = std::fs::File::create(&sql_path).unwrap();
    // Column-unknown patterns: orders.account_id = v_user_id (v_user_id not in FROM → tier1)
    f.write_all(
        b"SELECT * FROM orders WHERE orders.account_id = v_user_id AND orders.status = v_status",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("suggest")
        .arg("--file")
        .arg(sql_path.to_str().unwrap())
        .output()
        .expect("Failed to run metamorphosis suggest");

    assert!(
        output.status.success(),
        "suggest should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("detect-duplicate-eq-keys"),
        "Output should contain rule ID, got: {}",
        stdout
    );
    assert!(
        stdout.contains("GROUP BY") || stdout.contains("group by"),
        "Output should contain GROUP BY, got: {}",
        stdout
    );
}

/// End-to-end: rewrite SELECT * using --sql-dir DDL extraction.
#[test]
fn test_cli_rewrite_with_sql_dir() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let sql_path = dir.path().join("query.sql");
    let ddl_dir = dir.path().join("ddl");
    std::fs::create_dir(&ddl_dir).unwrap();

    let mut f = std::fs::File::create(&sql_path).unwrap();
    f.write_all(b"SELECT * FROM users WHERE id = 1").unwrap();

    let mut f = std::fs::File::create(ddl_dir.join("schema.sql")).unwrap();
    f.write_all(b"CREATE TABLE users (id INTEGER, name VARCHAR(100), email TEXT);")
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("rewrite")
        .arg("--file")
        .arg(sql_path.to_str().unwrap())
        .arg("--sql-dir")
        .arg(ddl_dir.to_str().unwrap())
        .output()
        .expect("Failed to run metamorphosis rewrite --sql-dir");

    assert!(
        output.status.success(),
        "rewrite --sql-dir should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("id"),
        "Output should contain column 'id', got: {}",
        stdout
    );
    assert!(
        stdout.contains("name"),
        "Output should contain column 'name', got: {}",
        stdout
    );
    assert!(
        !stdout.contains('*'),
        "Output should NOT contain '/*', got: {}",
        stdout
    );
    let stderr = str::from_utf8(&output.stderr).unwrap();
    assert!(
        stderr.contains("Extracted schema"),
        "stderr should log extraction, got: {}",
        stderr
    );
}

/// End-to-end: --sql-dir with non-existent directory should fail.
#[test]
fn test_cli_rewrite_sql_dir_not_found() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let sql_path = dir.path().join("query.sql");
    let mut f = std::fs::File::create(&sql_path).unwrap();
    f.write_all(b"SELECT 1").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("rewrite")
        .arg("--file")
        .arg(sql_path.to_str().unwrap())
        .arg("--sql-dir")
        .arg("/nonexistent/path")
        .output()
        .expect("Failed to run metamorphosis rewrite --sql-dir");

    assert!(
        !output.status.success(),
        "rewrite with bad --sql-dir should fail"
    );
    let stderr = str::from_utf8(&output.stderr).unwrap();
    assert!(
        stderr.contains("Error:"),
        "stderr should contain error message, got: {}",
        stderr
    );
}

/// End-to-end: --schema and --sql-dir are mutually exclusive.
#[test]
fn test_cli_rewrite_schema_and_sql_dir_mutually_exclusive() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let sql_path = dir.path().join("query.sql");
    let ddl_dir = dir.path().join("ddl");
    std::fs::create_dir(&ddl_dir).unwrap();
    let schema_path = dir.path().join("schema.json");

    let mut f = std::fs::File::create(&sql_path).unwrap();
    f.write_all(b"SELECT 1").unwrap();
    let mut f = std::fs::File::create(&schema_path).unwrap();
    f.write_all(b"{}").unwrap();
    let mut f = std::fs::File::create(ddl_dir.join("empty.sql")).unwrap();
    f.write_all(b"CREATE TABLE t (x INTEGER);").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("rewrite")
        .arg("--file")
        .arg(sql_path.to_str().unwrap())
        .arg("--schema")
        .arg(schema_path.to_str().unwrap())
        .arg("--sql-dir")
        .arg(ddl_dir.to_str().unwrap())
        .output()
        .expect("Failed to run metamorphosis rewrite");

    assert!(
        !output.status.success(),
        "mutual exclusive flags should fail"
    );
}

/// End-to-end: suggest with --sql-dir works.
#[test]
fn test_cli_suggest_with_sql_dir() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let sql_path = dir.path().join("query.sql");
    let ddl_dir = dir.path().join("ddl");
    std::fs::create_dir(&ddl_dir).unwrap();

    // Column-unknown patterns: orders.account_id = v_user_id (v_user_id not in FROM → tier1)
    let mut f = std::fs::File::create(&sql_path).unwrap();
    f.write_all(
        b"SELECT * FROM orders WHERE orders.account_id = v_user_id AND orders.status = v_status",
    )
    .unwrap();
    let mut f = std::fs::File::create(ddl_dir.join("schema.sql")).unwrap();
    f.write_all(b"CREATE TABLE orders (account_id INTEGER, status VARCHAR(20));")
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("suggest")
        .arg("--file")
        .arg(sql_path.to_str().unwrap())
        .arg("--sql-dir")
        .arg(ddl_dir.to_str().unwrap())
        .output()
        .expect("Failed to run metamorphosis suggest --sql-dir");

    assert!(
        output.status.success(),
        "suggest --sql-dir should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("detect-duplicate-eq-keys"),
        "Output should contain rule ID, got: {}",
        stdout
    );
}

/// End-to-end: rewrite SELECT * via stdin pipe.
#[test]
fn test_cli_rewrite_from_stdin() {
    use std::io::Write;
    use std::process::Stdio;

    let dir = tempfile::tempdir().unwrap();
    let schema_path = dir.path().join("schema.json");

    let mut schema = std::collections::HashMap::new();
    let mut cols = std::collections::HashMap::new();
    cols.insert("id".to_string(), "integer".to_string());
    cols.insert("name".to_string(), "varchar".to_string());
    schema.insert("users".to_string(), cols);
    let schema_json = serde_json::to_string(&schema).unwrap();
    let mut f = std::fs::File::create(&schema_path).unwrap();
    f.write_all(schema_json.as_bytes()).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("rewrite")
        .arg("--schema")
        .arg(schema_path.to_str().unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn metamorphosis rewrite");

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"SELECT * FROM users WHERE id = 1")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "rewrite from stdin should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("id"),
        "Output should contain column 'id', got: {}",
        stdout
    );
    assert!(
        stdout.contains("name"),
        "Output should contain column 'name', got: {}",
        stdout
    );
    assert!(
        !stdout.contains('*'),
        "Output should NOT contain '/*', got: {}",
        stdout
    );
}

/// End-to-end: suggest via stdin pipe.
#[test]
fn test_cli_suggest_from_stdin() {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("suggest")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn metamorphosis suggest");

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(
            b"SELECT * FROM orders WHERE orders.account_id = v_user_id AND orders.status = v_status",
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "suggest from stdin should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("detect-duplicate-eq-keys"),
        "Output should contain rule ID, got: {}",
        stdout
    );
    assert!(
        stdout.contains("GROUP BY") || stdout.contains("group by"),
        "Output should contain GROUP BY, got: {}",
        stdout
    );
}

/// End-to-end: rewrite using `-` as file path (stdin alias).
#[test]
fn test_cli_rewrite_from_stdin_dash() {
    use std::io::Write;
    use std::process::Stdio;

    let dir = tempfile::tempdir().unwrap();
    let schema_path = dir.path().join("schema.json");

    let mut schema = std::collections::HashMap::new();
    let mut cols = std::collections::HashMap::new();
    cols.insert("id".to_string(), "integer".to_string());
    cols.insert("name".to_string(), "varchar".to_string());
    schema.insert("users".to_string(), cols);
    let schema_json = serde_json::to_string(&schema).unwrap();
    let mut f = std::fs::File::create(&schema_path).unwrap();
    f.write_all(schema_json.as_bytes()).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("rewrite")
        .arg("--file")
        .arg("-")
        .arg("--schema")
        .arg(schema_path.to_str().unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn metamorphosis rewrite -");

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"SELECT * FROM users WHERE id = 1")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "rewrite from stdin (-) should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("id"),
        "Output should contain column 'id', got: {}",
        stdout
    );
    assert!(
        !stdout.contains('*'),
        "Output should NOT contain '/*', got: {}",
        stdout
    );
}
