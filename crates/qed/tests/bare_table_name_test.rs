//! End-to-end test: bare-name table lookup with qualified DDL schema.
//!
//! When DDL registers a table with a qualified name (e.g. `"public.users"`)
//! but the SQL uses only the bare name (`"users"`), `RichSchema::find_table`
//! should resolve it automatically.

use metamorphosis_qed::schema::extract_rich_schema;
use metamorphosis_qed::translator::AstTranslator;
use ogsql_parser::Parser;

fn parse_ddl(sql: &str) -> Vec<ogsql_parser::ast::Statement> {
    let (stmts, _) = Parser::parse_sql(sql);
    stmts.into_iter().map(|si| si.statement).collect()
}

fn parse_single(sql: &str) -> ogsql_parser::ast::Statement {
    let (stmts, _) = Parser::parse_sql(sql);
    stmts
        .into_iter()
        .next()
        .expect("expected one statement")
        .statement
}

/// DDL with qualified table name → query with bare name must translate
/// successfully (no `TableNotFound`).
#[test]
fn test_bare_table_name_resolves_to_qualified() {
    let ddl = parse_ddl("CREATE TABLE public.users (id INT, name VARCHAR(100))");
    let schema = extract_rich_schema(&ddl);

    // The DDL registers "public.users" in the schema...
    assert!(
        schema.tables.contains_key("public.users"),
        "DDL should register table as 'public.users'"
    );

    // ...but the query references the table by bare name "users"
    let query = parse_single("SELECT * FROM users");
    let translator = AstTranslator::new(&schema);
    let result = translator.translate(&query);

    assert!(
        result.is_ok(),
        "bare name 'users' should resolve to 'public.users', got error: {:?}",
        result.err()
    );
}

/// DDL with qualified table → qualified query with the same name works
/// (exact match path).
#[test]
fn test_qualified_table_name_exact_match() {
    let ddl = parse_ddl("CREATE TABLE public.users (id INT, name VARCHAR(100))");
    let schema = extract_rich_schema(&ddl);
    let query = parse_single("SELECT id FROM public.users");
    let translator = AstTranslator::new(&schema);
    let result = translator.translate(&query);

    assert!(
        result.is_ok(),
        "qualified name 'public.users' should match exactly, got error: {:?}",
        result.err()
    );
}

/// Ambiguous bare name with two schemas must fail at translation.
#[test]
fn test_ambiguous_bare_table_name_fails() {
    let ddl = parse_ddl(
        "CREATE TABLE schema_a.users (id INT); \
         CREATE TABLE schema_b.users (id INT)",
    );
    let schema = extract_rich_schema(&ddl);

    assert!(schema.tables.contains_key("schema_a.users"));
    assert!(schema.tables.contains_key("schema_b.users"));

    let query = parse_single("SELECT * FROM users");
    let translator = AstTranslator::new(&schema);
    let result = translator.translate(&query);

    assert!(
        result.is_err(),
        "ambiguous bare name 'users' should fail translation"
    );
}
