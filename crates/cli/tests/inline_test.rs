use std::io::Write;
use std::process::{Command, Stdio};
use std::str;

fn metamorphosis() -> Command {
    Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
}

#[test]
fn test_inline_help() {
    let output = metamorphosis()
        .arg("inline")
        .arg("--help")
        .output()
        .expect("Failed to run metamorphosis inline --help");
    assert!(output.status.success(), "inline --help should succeed");
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("Replace parameters/placeholders"),
        "Help should describe the command, got: {}",
        stdout
    );
}

#[test]
fn test_inline_jdbc_stdin() {
    let mut child = metamorphosis()
        .arg("inline")
        .arg("--val")
        .arg("ACC001")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn metamorphosis inline");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"SELECT * FROM t WHERE id = ?")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "inline should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("ACC001") || stdout.contains("'ACC001'"),
        "Output should contain the substituted value 'ACC001', got: {}",
        stdout
    );
}

#[test]
fn test_inline_mybatis_named() {
    let mut child = metamorphosis()
        .arg("inline")
        .arg("--mybatis")
        .arg("--param")
        .arg("status=active")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn metamorphosis inline");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"SELECT * FROM t WHERE status = #{status}")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "inline --mybatis should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("'active'"),
        "Output should contain the substituted value 'active', got: {}",
        stdout
    );
}

#[test]
fn test_inline_jdbc_file() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let sql_path = dir.path().join("query.sql");
    let mut f = std::fs::File::create(&sql_path).unwrap();
    f.write_all(b"SELECT * FROM t WHERE id = ? AND name = ?")
        .unwrap();

    let output = metamorphosis()
        .arg("inline")
        .arg("--file")
        .arg(sql_path.to_str().unwrap())
        .arg("--val")
        .arg("42")
        .arg("--val")
        .arg("hello")
        .output()
        .expect("Failed to run metamorphosis inline --file");
    assert!(
        output.status.success(),
        "inline --file should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("42"),
        "Output should contain 42, got: {}",
        stdout
    );
    assert!(
        stdout.contains("'hello'"),
        "Output should contain 'hello', got: {}",
        stdout
    );
}

#[test]
fn test_inline_params_file() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let sql_path = dir.path().join("query.sql");
    let params_path = dir.path().join("params.json");
    let mut f = std::fs::File::create(&sql_path).unwrap();
    f.write_all(b"SELECT * FROM t WHERE id = ? AND status = #{status}")
        .unwrap();
    let params_json = serde_json::json!({
        "positional": [42],
        "status": "active"
    });
    let mut f = std::fs::File::create(&params_path).unwrap();
    f.write_all(params_json.to_string().as_bytes()).unwrap();

    let output = metamorphosis()
        .arg("inline")
        .arg("--file")
        .arg(sql_path.to_str().unwrap())
        .arg("--params-file")
        .arg(params_path.to_str().unwrap())
        .arg("--mybatis")
        .output()
        .expect("Failed to run metamorphosis inline --params-file");
    assert!(
        output.status.success(),
        "inline --params-file should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("42"),
        "Output should contain 42, got: {}",
        stdout
    );
    assert!(
        stdout.contains("'active'"),
        "Output should contain 'active', got: {}",
        stdout
    );
}

#[test]
fn test_inline_json_output() {
    let mut child = metamorphosis()
        .arg("inline")
        .arg("--val")
        .arg("42")
        .arg("-o")
        .arg("json")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn metamorphosis inline");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"SELECT * FROM t WHERE id = ?")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "inline -o json should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("replaced_positional"),
        "JSON output should contain replaced_positional, got: {}",
        stdout
    );
    assert!(
        stdout.contains("statement"),
        "JSON output should contain statement field, got: {}",
        stdout
    );
}

#[test]
fn test_inline_remaining_placeholder() {
    let mut child = metamorphosis()
        .arg("inline")
        .arg("-o")
        .arg("json")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn metamorphosis inline");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"SELECT * FROM t WHERE id = ? AND status = ?")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "inline with missing params should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("remaining"),
        "Output should report remaining placeholders, got: {}",
        stdout
    );
}

#[test]
fn test_inline_from_procedure() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let proc_path = dir.path().join("test_proc.sql");
    let mut f = std::fs::File::create(&proc_path).unwrap();
    f.write_all(
        b"CREATE OR REPLACE PROCEDURE test_proc(
        in_accnt_date VARCHAR2,
        in_seq_no VARCHAR2
    ) IS
    BEGIN
        SELECT t.trade_code
        INTO v_trade_code
        FROM dat_clr_cash_dtl t
        WHERE t.account_date = in_accnt_date
          AND t.account_seqno = in_seq_no;
    END;",
    )
    .unwrap();

    let output = metamorphosis()
        .arg("inline")
        .arg("--file")
        .arg(proc_path.to_str().unwrap())
        .arg("--from-procedure")
        .arg("--param")
        .arg("in_accnt_date=20240101")
        .arg("--param")
        .arg("in_seq_no=SEQ001")
        .output()
        .expect("Failed to run metamorphosis inline --from-procedure");
    assert!(
        output.status.success(),
        "inline --from-procedure should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("20240101"),
        "Output should contain substituted value '20240101', got: {}",
        stdout
    );
    assert!(
        stdout.contains("SEQ001") || stdout.contains("'SEQ001'"),
        "Output should contain substituted value 'SEQ001', got: {}",
        stdout
    );
}

#[test]
fn test_inline_procedure_flag() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let proc_path = dir.path().join("vars_proc.sql");
    let sql_path = dir.path().join("query.sql");
    let mut f = std::fs::File::create(&proc_path).unwrap();
    f.write_all(b"CREATE OR REPLACE PROCEDURE demo(p_id VARCHAR2) IS BEGIN NULL; END;")
        .unwrap();
    let mut f = std::fs::File::create(&sql_path).unwrap();
    f.write_all(b"SELECT * FROM t WHERE id = p_id").unwrap();

    let output = metamorphosis()
        .arg("inline")
        .arg("--file")
        .arg(sql_path.to_str().unwrap())
        .arg("--procedure")
        .arg(proc_path.to_str().unwrap())
        .arg("--param")
        .arg("p_id=ABC123")
        .output()
        .expect("Failed to run metamorphosis inline --procedure");
    assert!(
        output.status.success(),
        "inline --procedure should succeed. stderr: {}",
        str::from_utf8(&output.stderr).unwrap()
    );
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(
        stdout.contains("ABC123") || stdout.contains("'ABC123'"),
        "Output should contain substituted 'ABC123', got: {}",
        stdout
    );
}
