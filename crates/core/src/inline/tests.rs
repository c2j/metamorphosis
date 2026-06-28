use super::*;
use ogsql_parser::ast::{Expr, Literal, Statement};
use ogsql_parser::ParseOptions;
use ogsql_parser::Parser;
use std::collections::HashSet;

fn parse(sql: &str) -> Vec<Statement> {
    let (infos, _errors) = Parser::parse_sql(sql);
    infos.into_iter().map(|info| info.statement).collect()
}

fn parse_mybatis(sql: &str) -> Vec<Statement> {
    let output = Parser::parse_sql_with_options(
        sql,
        ParseOptions {
            preserve_comments: false,
            mybatis_params: true,
        },
    );
    output
        .statements
        .into_iter()
        .map(|info| info.statement)
        .collect()
}

fn inline_sql(
    sql: &str,
    params: &InlineParams,
    known_vars: Option<&HashSet<String>>,
) -> InlineResult {
    let stmts = parse(sql);
    assert!(
        !stmts.is_empty(),
        "Expected at least one statement from: {sql}"
    );
    inline_statement(&stmts[0], params, known_vars)
}

fn format_stmt(stmt: &Statement) -> String {
    use ogsql_parser::formatter::SqlFormatter;
    SqlFormatter::new().format_statement(stmt)
}

#[test]
fn test_inline_value_to_sql_literal() {
    assert_eq!(InlineValue::Null.to_sql_literal(), "NULL");
    assert_eq!(InlineValue::Boolean(true).to_sql_literal(), "TRUE");
    assert_eq!(InlineValue::Boolean(false).to_sql_literal(), "FALSE");
    assert_eq!(InlineValue::Integer(0).to_sql_literal(), "0");
    assert_eq!(InlineValue::Integer(-42).to_sql_literal(), "-42");
    assert_eq!(InlineValue::Float("3.14".into()).to_sql_literal(), "3.14");
    assert_eq!(
        InlineValue::String("hello".into()).to_sql_literal(),
        "'hello'"
    );
    assert_eq!(InlineValue::String("".into()).to_sql_literal(), "''");
}

#[test]
fn test_string_escaping() {
    assert_eq!(
        InlineValue::String("O'Brien".into()).to_sql_literal(),
        "'O''Brien'"
    );
    assert_eq!(
        InlineValue::String("it''s".into()).to_sql_literal(),
        "'it''''s'"
    );
}

#[test]
fn test_infer_value() {
    assert_eq!(infer_value("NULL"), InlineValue::Null);
    assert_eq!(infer_value("null"), InlineValue::Null);
    assert_eq!(infer_value("Null"), InlineValue::Null);
    assert_eq!(infer_value("TRUE"), InlineValue::Boolean(true));
    assert_eq!(infer_value("true"), InlineValue::Boolean(true));
    assert_eq!(infer_value("FALSE"), InlineValue::Boolean(false));
    assert_eq!(infer_value("false"), InlineValue::Boolean(false));
    assert_eq!(infer_value("42"), InlineValue::Integer(42));
    assert_eq!(infer_value("-1"), InlineValue::Integer(-1));
    assert_eq!(infer_value("0"), InlineValue::Integer(0));
    assert_eq!(infer_value("3.14"), InlineValue::Float("3.14".into()));
    assert_eq!(infer_value("hello"), InlineValue::String("hello".into()));
    assert_eq!(
        infer_value("TRUEish"),
        InlineValue::String("TRUEish".into())
    );
}

#[test]
fn test_infer_value_leading_zero_code() {
    assert_eq!(infer_value("001"), InlineValue::String("001".into()));
    assert_eq!(infer_value("00012"), InlineValue::String("00012".into()));
    assert_eq!(infer_value("010"), InlineValue::String("010".into()));
}

#[test]
fn test_infer_value_single_zero_still_integer() {
    assert_eq!(infer_value("0"), InlineValue::Integer(0));
}

#[test]
fn test_infer_value_explicit_sql_string_simple() {
    assert_eq!(infer_value("'001'"), InlineValue::String("001".into()));
    assert_eq!(infer_value("'hello'"), InlineValue::String("hello".into()));
}

#[test]
fn test_infer_value_explicit_sql_string_with_escape() {
    assert_eq!(
        infer_value("'O''Brien'"),
        InlineValue::String("O'Brien".into())
    );
}

#[test]
fn test_infer_value_explicit_sql_string_empty() {
    assert_eq!(infer_value("''"), InlineValue::String(String::new()));
}

#[test]
fn test_infer_value_cast_string_base() {
    assert_eq!(
        infer_value("'20260101'::date"),
        InlineValue::Cast {
            value: Box::new(InlineValue::String("20260101".into())),
            type_name: "date".into(),
        }
    );
}

#[test]
fn test_infer_value_cast_integer_base() {
    assert_eq!(
        infer_value("42::int"),
        InlineValue::Cast {
            value: Box::new(InlineValue::Integer(42)),
            type_name: "int".into(),
        }
    );
}

#[test]
fn test_infer_value_cast_preserves_type_args() {
    assert_eq!(
        infer_value("'5'::numeric(10,2)"),
        InlineValue::Cast {
            value: Box::new(InlineValue::String("5".into())),
            type_name: "numeric(10,2)".into(),
        }
    );
}

#[test]
fn test_infer_value_cast_with_escaped_quote_in_base() {
    assert_eq!(
        infer_value("'O''Brien'::text"),
        InlineValue::Cast {
            value: Box::new(InlineValue::String("O'Brien".into())),
            type_name: "text".into(),
        }
    );
}

#[test]
fn test_infer_value_cast_unquoted_shell_stripped() {
    // After shell quote-stripping the program sees `20260101::date`:
    // base infers to Integer, cast is still preserved.
    assert_eq!(
        infer_value("20260101::date"),
        InlineValue::Cast {
            value: Box::new(InlineValue::Integer(20260101)),
            type_name: "date".into(),
        }
    );
}

#[test]
fn test_infer_value_quoted_string_with_inner_colons_not_cast() {
    // `::` inside quotes must not be treated as a cast.
    assert_eq!(infer_value("'a::b'"), InlineValue::String("a::b".into()));
}

#[test]
fn test_infer_value_single_colon_not_cast() {
    assert_eq!(infer_value("12:30"), InlineValue::String("12:30".into()));
}

#[test]
fn test_inline_value_cast_to_sql_literal() {
    assert_eq!(
        InlineValue::Cast {
            value: Box::new(InlineValue::String("20260101".into())),
            type_name: "date".into(),
        }
        .to_sql_literal(),
        "'20260101'::date"
    );
    assert_eq!(
        InlineValue::Cast {
            value: Box::new(InlineValue::Integer(42)),
            type_name: "int".into(),
        }
        .to_sql_literal(),
        "42::int"
    );
}

#[test]
fn test_inline_value_cast_to_expr() {
    let expr = InlineValue::Cast {
        value: Box::new(InlineValue::String("20260101".into())),
        type_name: "date".into(),
    }
    .to_expr();
    match expr {
        Expr::TypeCast { type_name, .. } => match type_name {
            DataType::Custom(name, _) => {
                assert_eq!(name.len(), 1);
                assert_eq!(name[0].value, "date");
            }
            other => panic!("expected DataType::Custom, got {other:?}"),
        },
        other => panic!("expected Expr::TypeCast, got {other:?}"),
    }
}

#[test]
fn test_inline_cast_in_statement() {
    let result = inline_sql(
        "SELECT * FROM t WHERE d = v_date",
        &InlineParams {
            named: [(
                "v_date".into(),
                InlineValue::Cast {
                    value: Box::new(InlineValue::String("20260101".into())),
                    type_name: "date".into(),
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        },
        None,
    );
    assert_eq!(result.replaced_named, 1);
    let sql = format_stmt(&result.statement);
    assert!(
        sql.contains("'20260101'::date"),
        "Expected '20260101'::date in: {sql}"
    );
}

#[test]
fn test_infer_value_normal_integers_unchanged() {
    assert_eq!(infer_value("100"), InlineValue::Integer(100));
    assert_eq!(infer_value("42"), InlineValue::Integer(42));
    assert_eq!(infer_value("-5"), InlineValue::Integer(-5));
}

#[test]
fn test_jdbc_param_simple() {
    let result = inline_sql(
        "SELECT * FROM t WHERE id = ?",
        &InlineParams {
            positional: vec![InlineValue::String("ACC001".into())],
            ..Default::default()
        },
        None,
    );
    assert_eq!(result.replaced_positional, 1);
    assert_eq!(result.replaced_named, 0);
    assert!(result.remaining.is_empty());
    let sql = format_stmt(&result.statement);
    assert!(sql.contains("'ACC001'"), "Expected ACC001 in: {sql}");
}

#[test]
fn test_jdbc_param_multiple() {
    let result = inline_sql(
        "SELECT * FROM t WHERE a = ? AND b = ?",
        &InlineParams {
            positional: vec![InlineValue::Integer(1), InlineValue::String("two".into())],
            ..Default::default()
        },
        None,
    );
    assert_eq!(result.replaced_positional, 2);
    assert!(result.remaining.is_empty());
    let sql = format_stmt(&result.statement);
    assert!(sql.contains('1'), "Expected 1 in: {sql}");
    assert!(sql.contains("'two'"), "Expected 'two' in: {sql}");
}

#[test]
fn test_jdbc_param_in_case() {
    let result = inline_sql(
        "SELECT CASE WHEN a = ? THEN ? ELSE ? END FROM t",
        &InlineParams {
            positional: vec![
                InlineValue::Integer(1),
                InlineValue::String("yes".into()),
                InlineValue::String("no".into()),
            ],
            ..Default::default()
        },
        None,
    );
    assert_eq!(result.replaced_positional, 3);
    assert!(result.remaining.is_empty());
    let sql = format_stmt(&result.statement);
    assert!(sql.contains('1'), "Expected 1 in: {sql}");
    assert!(sql.contains("'yes'"), "Expected 'yes' in: {sql}");
    assert!(sql.contains("'no'"), "Expected 'no' in: {sql}");
}

#[test]
fn test_jdbc_param_in_function() {
    let result = inline_sql(
        "SELECT COALESCE(a, ?) FROM t",
        &InlineParams {
            positional: vec![InlineValue::String("default".into())],
            ..Default::default()
        },
        None,
    );
    assert_eq!(result.replaced_positional, 1);
    assert!(result.remaining.is_empty());
    let sql = format_stmt(&result.statement);
    assert!(sql.contains("'default'"), "Expected 'default' in: {sql}");
}

#[test]
fn test_jdbc_param_in_between() {
    let result = inline_sql(
        "SELECT * FROM t WHERE col BETWEEN ? AND ?",
        &InlineParams {
            positional: vec![InlineValue::Integer(10), InlineValue::Integer(20)],
            ..Default::default()
        },
        None,
    );
    assert_eq!(result.replaced_positional, 2);
    assert!(result.remaining.is_empty());
    let sql = format_stmt(&result.statement);
    assert!(sql.contains("10"), "Expected 10 in: {sql}");
    assert!(sql.contains("20"), "Expected 20 in: {sql}");
}

#[test]
fn test_mybatis_param() {
    let sql = "SELECT * FROM t WHERE col = #{status}";
    let stmts = parse_mybatis(sql);
    assert!(!stmts.is_empty(), "Expected at least one statement");
    let result = inline_statement(
        &stmts[0],
        &InlineParams {
            named: [("status".into(), InlineValue::String("active".into()))]
                .into_iter()
                .collect(),
            ..Default::default()
        },
        None,
    );
    assert_eq!(result.replaced_named, 1);
    assert!(result.remaining.is_empty());
    let out = format_stmt(&result.statement);
    assert!(out.contains("'active'"), "Expected 'active' in: {out}");
}

#[test]
fn test_mybatis_raw_expr() {
    let sql = "SELECT * FROM t WHERE col = ${type}";
    let stmts = parse_mybatis(sql);
    assert!(!stmts.is_empty(), "Expected at least one statement");
    let result = inline_statement(
        &stmts[0],
        &InlineParams {
            named: [("type".into(), InlineValue::String("admin".into()))]
                .into_iter()
                .collect(),
            ..Default::default()
        },
        None,
    );
    assert_eq!(result.replaced_named, 1);
    assert!(result.remaining.is_empty());
    let out = format_stmt(&result.statement);
    assert!(out.contains("'admin'"), "Expected 'admin' in: {out}");
}

#[test]
fn test_mybatis_param_missing() {
    let sql = "SELECT * FROM t WHERE status = #{status}";
    let stmts = parse_mybatis(sql);
    assert!(!stmts.is_empty());
    let result = inline_statement(&stmts[0], &InlineParams::default(), None);
    assert_eq!(result.replaced_named, 0);
    assert_eq!(result.remaining.len(), 1);
    assert_eq!(result.remaining[0].kind, "mybatis");
    assert_eq!(result.remaining[0].name, Some("status".into()));
}

#[test]
fn test_stored_proc_variable() {
    let result = inline_sql(
        "SELECT * FROM t WHERE col = in_accnt_date",
        &InlineParams {
            named: [(
                "in_accnt_date".into(),
                InlineValue::String("20240101".into()),
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        },
        Some(&HashSet::from(["in_accnt_date".into()])),
    );
    assert_eq!(result.replaced_named, 1);
    assert!(result.remaining.is_empty());
    let sql = format_stmt(&result.statement);
    assert!(sql.contains("'20240101'"), "Expected '20240101' in: {sql}");
}

#[test]
fn test_stored_proc_fallback_to_params_named() {
    let result = inline_sql(
        "SELECT * FROM t WHERE col = in_accnt_date",
        &InlineParams {
            named: [(
                "in_accnt_date".into(),
                InlineValue::String("20240101".into()),
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        },
        None,
    );
    assert_eq!(
        result.replaced_named, 1,
        "Fallback should substitute when name matches params.named"
    );
    assert!(result.remaining.is_empty());
    let sql = format_stmt(&result.statement);
    assert!(sql.contains("'20240101'"), "Expected '20240101' in: {sql}");
}

#[test]
fn test_stored_proc_column_not_replaced() {
    let result = inline_sql(
        "SELECT * FROM t WHERE col = regular_column",
        &InlineParams {
            named: [("regular_column".into(), InlineValue::Integer(42))]
                .into_iter()
                .collect(),
            ..Default::default()
        },
        Some(&HashSet::from(["other_var".into()])),
    );
    assert_eq!(result.replaced_named, 0);
    assert!(result.remaining.is_empty());
}

#[test]
fn test_no_fallback_when_name_not_in_params() {
    let result = inline_sql(
        "SELECT * FROM t WHERE col = some_real_column",
        &InlineParams::default(),
        None,
    );
    assert_eq!(result.replaced_named, 0);
    assert!(result.remaining.is_empty());
}

#[test]
fn test_fallback_substitutes_qualified_columns() {
    let result = inline_sql(
        "SELECT * FROM t WHERE t.id = p_id AND other = p_id",
        &InlineParams {
            named: [("p_id".into(), InlineValue::Integer(42))]
                .into_iter()
                .collect(),
            ..Default::default()
        },
        None,
    );
    assert_eq!(result.replaced_named, 2);
    let sql = format_stmt(&result.statement);
    assert!(
        !sql.contains("p_id"),
        "Expected p_id to be replaced in: {sql}"
    );
}

#[test]
fn test_empty_whitelist_disables_columnref_fallback() {
    // Regression for MyBatis collision: when known_vars is Some(empty),
    // ColumnRef is NOT substituted even if name matches params.named.
    // MyBatisParam substitution still works normally.
    let stmts = parse_mybatis("SELECT * FROM t WHERE status = #{status}");
    assert!(!stmts.is_empty());
    let result = inline_statement(
        &stmts[0],
        &InlineParams {
            named: [("status".into(), InlineValue::String("active".into()))]
                .into_iter()
                .collect(),
            ..Default::default()
        },
        Some(&HashSet::new()),
    );
    assert_eq!(
        result.replaced_named, 1,
        "Only the MyBatis param should be replaced, not the ColumnRef"
    );
    let sql = format_stmt(&result.statement);
    assert!(
        !sql.contains("'active' = 'active'"),
        "Double-substitution bug detected in: {sql}"
    );
}

#[test]
fn test_parameter_numbered() {
    let result = inline_sql(
        "SELECT * FROM t WHERE a = $1 AND b = $2",
        &InlineParams {
            positional: vec![
                InlineValue::Integer(100),
                InlineValue::String("hello".into()),
            ],
            ..Default::default()
        },
        None,
    );
    assert_eq!(result.replaced_positional, 2);
    assert!(result.remaining.is_empty());
    let sql = format_stmt(&result.statement);
    assert!(sql.contains("100"), "Expected 100 in: {sql}");
    assert!(sql.contains("'hello'"), "Expected 'hello' in: {sql}");
}

#[test]
fn test_missing_positional() {
    let result = inline_sql(
        "SELECT * FROM t WHERE a = ? AND b = ? AND c = ?",
        &InlineParams {
            positional: vec![InlineValue::Integer(1)],
            ..Default::default()
        },
        None,
    );
    assert_eq!(result.replaced_positional, 1);
    assert_eq!(result.remaining.len(), 2);
}

#[test]
fn test_multiple_statements() {
    let sql = "SELECT * FROM t WHERE a = ?; SELECT * FROM t2 WHERE b = ?";
    let stmts = parse(sql);
    assert_eq!(stmts.len(), 2);
    let params = InlineParams {
        positional: vec![InlineValue::Integer(1), InlineValue::Integer(2)],
        ..Default::default()
    };
    let r1 = inline_statement(&stmts[0], &params, None);
    assert_eq!(r1.replaced_positional, 1);
    assert!(r1.remaining.is_empty());
    let r2 = inline_statement(&stmts[1], &params, None);
    assert_eq!(r2.replaced_positional, 1);
    assert!(r2.remaining.is_empty());
}

#[test]
fn test_update_with_positional() {
    let result = inline_sql(
        "UPDATE t SET name = ? WHERE id = ?",
        &InlineParams {
            positional: vec![
                InlineValue::String("new_name".into()),
                InlineValue::Integer(42),
            ],
            ..Default::default()
        },
        None,
    );
    assert_eq!(result.replaced_positional, 2);
    assert!(result.remaining.is_empty());
    let sql = format_stmt(&result.statement);
    assert!(sql.contains("'new_name'"), "Expected 'new_name' in: {sql}");
    assert!(sql.contains("42"), "Expected 42 in: {sql}");
}

#[test]
fn test_delete_with_positional() {
    let result = inline_sql(
        "DELETE FROM t WHERE id = ?",
        &InlineParams {
            positional: vec![InlineValue::Integer(99)],
            ..Default::default()
        },
        None,
    );
    assert_eq!(result.replaced_positional, 1);
    assert!(result.remaining.is_empty());
    let sql = format_stmt(&result.statement);
    assert!(sql.contains("99"), "Expected 99 in: {sql}");
}

#[test]
fn test_insert_with_values() {
    let result = inline_sql(
        "INSERT INTO t (a, b) VALUES (?, ?)",
        &InlineParams {
            positional: vec![InlineValue::Integer(1), InlineValue::String("two".into())],
            ..Default::default()
        },
        None,
    );
    assert_eq!(result.replaced_positional, 2);
    assert!(result.remaining.is_empty());
    let sql = format_stmt(&result.statement);
    assert!(sql.contains('1'), "Expected 1 in: {sql}");
    assert!(sql.contains("'two'"), "Expected 'two' in: {sql}");
}

#[test]
fn test_non_dml_passthrough() {
    let result = inline_sql(
        "CREATE TABLE t (id INT)",
        &InlineParams {
            positional: vec![InlineValue::Integer(1)],
            ..Default::default()
        },
        None,
    );
    assert_eq!(result.replaced_positional, 0);
    assert_eq!(result.replaced_named, 0);
    assert!(result.remaining.is_empty());
}

#[test]
fn test_inline_value_to_expr() {
    assert_eq!(InlineValue::Null.to_expr(), Expr::Literal(Literal::Null));
    assert_eq!(
        InlineValue::Boolean(true).to_expr(),
        Expr::Literal(Literal::Boolean(true))
    );
    assert_eq!(
        InlineValue::Integer(42).to_expr(),
        Expr::Literal(Literal::Integer(42))
    );
    assert_eq!(
        InlineValue::Float("3.14".into()).to_expr(),
        Expr::Literal(Literal::Float("3.14".into()))
    );
    assert_eq!(
        InlineValue::String("hello".into()).to_expr(),
        Expr::Literal(Literal::String("hello".into()))
    );
}

#[test]
fn test_mixed_params() {
    let sql = "SELECT * FROM t WHERE col = #{status} AND id = ?";
    let stmts = parse_mybatis(sql);
    assert!(!stmts.is_empty());
    let result = inline_statement(
        &stmts[0],
        &InlineParams {
            named: [("status".into(), InlineValue::String("active".into()))]
                .into_iter()
                .collect(),
            positional: vec![InlineValue::Integer(5)],
        },
        None,
    );
    assert_eq!(result.replaced_named, 1);
    assert_eq!(result.replaced_positional, 1);
    assert!(result.remaining.is_empty());
    let out = format_stmt(&result.statement);
    assert!(out.contains("'active'"), "Expected 'active' in: {out}");
    assert!(out.contains("5"), "Expected 5 in: {out}");
}

#[test]
fn test_strip_select_into() {
    let sql = "SELECT t.c1 INTO var FROM t WHERE status = #{status}";
    let stmts = parse_mybatis(sql);
    assert!(!stmts.is_empty());
    let result = inline_statement(
        &stmts[0],
        &InlineParams {
            named: [("status".into(), InlineValue::String("active".into()))]
                .into_iter()
                .collect(),
            ..Default::default()
        },
        None,
    );
    let out = format_stmt(&result.statement);
    assert!(
        !out.to_uppercase().contains("INTO"),
        "INTO clause should be stripped, got: {out}"
    );
    assert!(out.contains("'active'"), "Expected 'active' in: {out}");
    assert!(out.contains("FROM t"), "Expected FROM t in: {out}");
}

#[test]
fn test_strip_update_returning_into() {
    let sql = "UPDATE t SET status = 'active' WHERE id = #{id} RETURNING name INTO var";
    let stmts = parse_mybatis(sql);
    assert!(!stmts.is_empty());
    let result = inline_statement(
        &stmts[0],
        &InlineParams {
            named: [("id".into(), InlineValue::Integer(42))]
                .into_iter()
                .collect(),
            ..Default::default()
        },
        None,
    );
    let out = format_stmt(&result.statement);
    assert!(
        !out.to_uppercase().contains("INTO"),
        "PL/pgSQL INTO should be stripped, got: {out}"
    );
    assert!(
        out.to_uppercase().contains("RETURNING"),
        "RETURNING clause should be preserved, got: {out}"
    );
}

#[test]
fn test_strip_delete_returning_into() {
    let sql = "DELETE FROM t WHERE id = #{id} RETURNING name INTO var";
    let stmts = parse_mybatis(sql);
    assert!(!stmts.is_empty());
    let result = inline_statement(
        &stmts[0],
        &InlineParams {
            named: [("id".into(), InlineValue::Integer(42))]
                .into_iter()
                .collect(),
            ..Default::default()
        },
        None,
    );
    let out = format_stmt(&result.statement);
    assert!(
        !out.to_uppercase().contains("INTO"),
        "PL/pgSQL INTO should be stripped, got: {out}"
    );
    assert!(
        out.to_uppercase().contains("RETURNING"),
        "RETURNING clause should be preserved, got: {out}"
    );
}
