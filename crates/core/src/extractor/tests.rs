use super::*;
use std::io::Write;
use std::path::Path;

fn write_sql_file(dir: &Path, name: &str, sql: &str) {
    let mut f = std::fs::File::create(dir.join(name)).unwrap();
    f.write_all(sql.as_bytes()).unwrap();
}

fn create_temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("failed to create temp dir")
}

#[test]
fn test_single_create_table() {
    let dir = create_temp_dir();
    write_sql_file(
        dir.path(),
        "schema.sql",
        "CREATE TABLE users (id INTEGER, name VARCHAR(100), email TEXT);",
    );

    let schema = extract_schema_from_dir(dir.path()).unwrap();
    assert_eq!(schema.len(), 1);

    let users = schema.get("users").expect("users table");
    assert_eq!(users.get("id").unwrap(), "INTEGER");
    assert_eq!(users.get("name").unwrap(), "VARCHAR(100)");
    assert_eq!(users.get("email").unwrap(), "TEXT");
}

#[test]
fn test_multiple_tables_one_file() {
    let dir = create_temp_dir();
    write_sql_file(
        dir.path(),
        "ddl.sql",
        "CREATE TABLE employees (id INTEGER, name VARCHAR(100));
         CREATE TABLE departments (dept_id INTEGER, dept_name VARCHAR(200));",
    );

    let schema = extract_schema_from_dir(dir.path()).unwrap();
    assert_eq!(schema.len(), 2);
    assert!(schema.contains_key("employees"));
    assert!(schema.contains_key("departments"));
}

#[test]
fn test_multiple_files() {
    let dir = create_temp_dir();
    write_sql_file(dir.path(), "01_tables.sql", "CREATE TABLE t1 (a INTEGER);");
    write_sql_file(
        dir.path(),
        "02_tables.sql",
        "CREATE TABLE t2 (b VARCHAR(50));",
    );

    let schema = extract_schema_from_dir(dir.path()).unwrap();
    assert_eq!(schema.len(), 2);
    assert!(schema.contains_key("t1"));
    assert!(schema.contains_key("t2"));
}

#[test]
fn test_alter_table_add_column() {
    let dir = create_temp_dir();
    write_sql_file(
        dir.path(),
        "schema.sql",
        "CREATE TABLE users (id INTEGER PRIMARY KEY);
         ALTER TABLE users ADD COLUMN email VARCHAR(255);",
    );

    let schema = extract_schema_from_dir(dir.path()).unwrap();
    let users = schema.get("users").expect("users table");
    assert_eq!(users.get("id").unwrap(), "INTEGER");
    assert_eq!(users.get("email").unwrap(), "VARCHAR(255)");
}

#[test]
fn test_alter_table_drop_column() {
    let dir = create_temp_dir();
    write_sql_file(
        dir.path(),
        "schema.sql",
        "CREATE TABLE users (id INTEGER, name VARCHAR(100), email VARCHAR(255));
         ALTER TABLE users DROP COLUMN email;",
    );

    let schema = extract_schema_from_dir(dir.path()).unwrap();
    let users = schema.get("users").expect("users table");
    assert_eq!(users.len(), 2);
    assert!(users.contains_key("id"));
    assert!(users.contains_key("name"));
    assert!(!users.contains_key("email"));
}

#[test]
fn test_alter_table_rename_column() {
    let dir = create_temp_dir();
    write_sql_file(
        dir.path(),
        "schema.sql",
        "CREATE TABLE users (id INTEGER, username VARCHAR(100));
         ALTER TABLE users RENAME COLUMN username TO login;",
    );

    let schema = extract_schema_from_dir(dir.path()).unwrap();
    let users = schema.get("users").expect("users table");
    assert!(!users.contains_key("username"));
    assert_eq!(users.get("login").unwrap(), "VARCHAR(100)");
}

#[test]
fn test_drop_create_pattern() {
    let dir = create_temp_dir();
    write_sql_file(
        dir.path(),
        "schema.sql",
        "DROP TABLE IF EXISTS users CASCADE;
         CREATE TABLE users (id INTEGER, name VARCHAR(100));
         DROP TABLE IF EXISTS users CASCADE;
         CREATE TABLE users (id INTEGER, name VARCHAR(200), email TEXT);",
    );

    let schema = extract_schema_from_dir(dir.path()).unwrap();
    let users = schema.get("users").expect("users table");
    // Last definition wins
    assert_eq!(users.get("name").unwrap(), "VARCHAR(200)");
    assert!(users.contains_key("email"));
}

#[test]
fn test_case_insensitive_lowercase() {
    let dir = create_temp_dir();
    write_sql_file(
        dir.path(),
        "schema.sql",
        "CREATE TABLE \"MixedCase\" (\"ColA\" INTEGER);",
    );

    let schema = extract_schema_from_dir(dir.path()).unwrap();
    let t = schema.get("mixedcase").expect("mixedcase table");
    assert_eq!(t.get("cola").unwrap(), "INTEGER");
}

#[test]
fn test_data_types_variety() {
    let dir = create_temp_dir();
    write_sql_file(
        dir.path(),
        "schema.sql",
        "CREATE TABLE types (
            c1 BIGINT,
            c2 SMALLINT,
            c3 NUMERIC(18,2),
            c4 REAL,
            c5 DOUBLE,
            c6 BOOLEAN,
            c7 DATE,
            c8 TIMESTAMP,
            c9 TIMESTAMP WITH TIME ZONE,
            c10 JSONB,
            c11 UUID,
            c12 TEXT
        );",
    );

    let schema = extract_schema_from_dir(dir.path()).unwrap();
    let t = schema.get("types").expect("types table");
    assert_eq!(t.get("c1").unwrap(), "BIGINT");
    assert_eq!(t.get("c2").unwrap(), "SMALLINT");
    assert_eq!(t.get("c3").unwrap(), "NUMERIC(18,2)");
    assert_eq!(t.get("c4").unwrap(), "REAL");
    assert_eq!(t.get("c5").unwrap(), "DOUBLE");
    assert_eq!(t.get("c6").unwrap(), "BOOLEAN");
    assert_eq!(t.get("c7").unwrap(), "DATE");
    assert_eq!(t.get("c8").unwrap(), "TIMESTAMP");
    assert_eq!(t.get("c9").unwrap(), "TIMESTAMP WITH TIME ZONE");
    assert_eq!(t.get("c10").unwrap(), "JSONB");
    assert_eq!(t.get("c11").unwrap(), "UUID");
    assert_eq!(t.get("c12").unwrap(), "TEXT");
}

#[test]
fn test_create_table_as_with_columns() {
    let dir = create_temp_dir();
    write_sql_file(
        dir.path(),
        "schema.sql",
        "CREATE TABLE backup (id, name) AS SELECT * FROM original;",
    );

    let schema = extract_schema_from_dir(dir.path()).unwrap();
    let t = schema.get("backup").expect("backup table");
    assert_eq!(t.get("id").unwrap(), "unknown");
    assert_eq!(t.get("name").unwrap(), "unknown");
}

#[test]
fn test_error_dir_not_found() {
    let result = extract_schema_from_dir(Path::new("/nonexistent/path"));
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, ExtractionError::DirNotFound(_)),
        "expected DirNotFound, got {err}"
    );
}

#[test]
fn test_error_no_sql_files() {
    let dir = create_temp_dir();
    let result = extract_schema_from_dir(dir.path());
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, ExtractionError::NoSqlFiles(_)),
        "expected NoSqlFiles, got {err}"
    );
}

#[test]
fn test_schema_file_not_json() {
    let dir = create_temp_dir();
    write_sql_file(dir.path(), "not_sql.txt", "this is not sql");
    let result = extract_schema_from_dir(dir.path());
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, ExtractionError::NoSqlFiles(_)),
        "expected NoSqlFiles (no .sql files), got {err}"
    );
}

#[test]
fn test_skip_parse_error_file() {
    let dir = create_temp_dir();
    // One broken file + one good file — schema still extracted from good file.
    write_sql_file(dir.path(), "broken.sql", "this is not valid SQL");
    write_sql_file(dir.path(), "good.sql", "CREATE TABLE t (x INTEGER);");

    let schema = extract_schema_from_dir(dir.path()).unwrap();
    assert_eq!(schema.len(), 1);
    assert!(schema.contains_key("t"));
    let t = schema.get("t").unwrap();
    assert_eq!(t.get("x").unwrap(), "INTEGER");
}

#[test]
fn test_all_files_parse_error() {
    let dir = create_temp_dir();
    // Unterminated string — guaranteed parse error.
    write_sql_file(
        dir.path(),
        "bad1.sql",
        "CREATE TABLE t (x INTEGER); 'unterminated",
    );
    write_sql_file(dir.path(), "bad2.sql", "SELECT 'no closing quote");

    let result = extract_schema_from_dir(dir.path());
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, ExtractionError::AllFilesSkipped(_)),
        "expected AllFilesSkipped, got {err}"
    );
}

#[test]
fn test_empty_sql_file() {
    let dir = create_temp_dir();
    write_sql_file(dir.path(), "empty.sql", "");
    let schema = extract_schema_from_dir(dir.path()).unwrap();
    assert!(schema.is_empty());
}

#[test]
fn test_ddl_statements_only() {
    let dir = create_temp_dir();
    write_sql_file(
        dir.path(),
        "mixed.sql",
        "CREATE TABLE t1 (x INTEGER);
         SELECT * FROM t1;
         UPDATE t1 SET x = 1;
         CREATE TABLE t2 (y VARCHAR(10));",
    );

    let schema = extract_schema_from_dir(dir.path()).unwrap();
    assert_eq!(schema.len(), 2);
    assert!(schema.contains_key("t1"));
    assert!(schema.contains_key("t2"));
    let t1 = schema.get("t1").unwrap();
    assert_eq!(t1.get("x").unwrap(), "INTEGER");
}

#[test]
fn test_utf8_bom_file() {
    let dir = create_temp_dir();
    use std::io::Write;
    {
        let mut f = std::fs::File::create(dir.path().join("bom.sql")).unwrap();
        // UTF-8 BOM (\xEF\xBB\xBF) followed by valid DDL.
        f.write_all(b"\xef\xbb\xbfCREATE TABLE t (x INTEGER);")
            .unwrap();
    }
    let schema = extract_schema_from_dir(dir.path()).unwrap();
    assert_eq!(schema.len(), 1);
    assert!(schema.contains_key("t"));
}
