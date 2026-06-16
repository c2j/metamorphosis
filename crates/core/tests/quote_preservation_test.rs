//! Integration tests for identifier quote preservation during SQL round-trip.
//!
//! Contract: `SQL → parse → format → SQL` must preserve the quoting style of
//! identifiers from the original input. An identifier that was unquoted in the
//! source must remain unquoted in the output; one that was quoted must stay
//! quoted. Violating this changes query semantics in openGauss/GaussDB, where
//! `"MyTable"` (case-sensitive) and `MyTable` (folded per `lower_case_table_names`)
//! may resolve to different tables.
//!
//! Background: ogsql-parser's `parse_identifier` merges `Token::Ident` and
//! `Token::QuotedIdent` into the same `String`, discarding the quote-style.
//! `ObjectName = Vec<String>` has no field to carry this information. The
//! `SqlFormatter` then uses a character-set heuristic (uppercase → must quote)
//! that assumes Postgres-style folding — which the parser does not perform.
//!
//! See: <https://github.com/c2j/ogsql-parser/issues/224> (fixed by PR #226)

use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::Parser;

fn round_trip(sql: &str) -> String {
    let (stmts, errors) = Parser::parse_sql(sql);
    assert!(
        errors.is_empty(),
        "Parse errors for {:?}: {:?}",
        sql,
        errors
    );
    assert_eq!(
        stmts.len(),
        1,
        "Expected exactly 1 statement for {:?}, got {}",
        sql,
        stmts.len()
    );
    SqlFormatter::new().format_statement(&stmts[0].statement)
}

// ---------------------------------------------------------------------------
// Cases that PASS today (no bug).
// ---------------------------------------------------------------------------

/// Lowercase unquoted identifier: no quotes in → no quotes out.
#[test]
fn lowercase_unquoted_stays_unquoted() {
    let out = round_trip("SELECT * FROM mytable");
    assert!(
        !out.contains('"'),
        "lowercase unquoted identifier should stay unquoted; got: {}",
        out
    );
}

/// Quoted identifier with uppercase: quotes preserved (currently passes, but
/// for the wrong reason — the formatter re-adds quotes based on case, not on
/// original quote-style).
#[test]
fn quoted_uppercase_stays_quoted() {
    let out = round_trip("SELECT * FROM \"MyTable\"");
    assert!(
        out.contains("\"MyTable\""),
        "quoted identifier should preserve quotes; got: {}",
        out
    );
}

/// Quoted identifier containing a space: quotes are semantically required,
/// so they must be preserved.
#[test]
fn quoted_with_space_stays_quoted() {
    let out = round_trip("SELECT * FROM \"my table\"");
    assert!(
        out.contains("\"my table\""),
        "identifier with space must keep quotes; got: {}",
        out
    );
}

/// Schema-qualified name where both parts are lowercase unquoted.
#[test]
fn schema_qualified_lowercase_stays_unquoted() {
    let out = round_trip("SELECT * FROM public.users");
    assert!(
        !out.contains('"'),
        "lowercase schema.table should stay unquoted; got: {}",
        out
    );
}

// ---------------------------------------------------------------------------
// Cases that EXPOSE THE BUG — ignored until ogsql-parser is fixed.
// ---------------------------------------------------------------------------

/// **DESIRED behavior** (currently fails): an uppercase identifier written
/// *without* quotes must not gain quotes after round-trip.
///
/// Root cause: the tokenizer stores `Token::Ident("MyTable")` (no folding),
/// `parse_identifier` strips the token-type distinction, and `quote_identifier`
/// adds quotes because it sees an uppercase letter — even though the parser
/// treated the identifier as valid unquoted.
///
/// Why it matters: in openGauss, `FROM MyTable` (unquoted) and
/// `FROM "MyTable"` (quoted) can resolve to different tables. The round-trip
/// silently changes query semantics.
#[test]
fn uppercase_unquoted_must_stay_unquoted() {
    let out = round_trip("SELECT * FROM MyTable");
    assert!(
        !out.contains('"'),
        "unquoted uppercase identifier must NOT gain quotes — this changes semantics; got: {}",
        out
    );
}

/// **DESIRED behavior** (currently fails): a mixed-case identifier written
/// *without* quotes must not gain quotes after round-trip.
#[test]
fn mixedcase_unquoted_must_stay_unquoted() {
    let out = round_trip("SELECT id FROM userDetails WHERE id = 1");
    assert!(
        !out.contains('"'),
        "unquoted mixed-case identifier must NOT gain quotes; got: {}",
        out
    );
}

/// **DESIRED behavior** (currently fails): schema-qualified uppercase name
/// written *without* quotes must not gain quotes.
#[test]
fn schema_qualified_uppercase_must_stay_unquoted() {
    let out = round_trip("SELECT * FROM public.MyTable");
    assert!(
        !out.contains('"'),
        "unquoted schema-qualified uppercase must NOT gain quotes; got: {}",
        out
    );
}

/// **DESIRED behavior**: a quoted *lowercase* identifier should keep its
/// quotes in the output. Once ogsql-parser preserves `quote_style`, this case
/// distinguishes "the user wanted case-sensitivity on a lowercase name" from
/// the unquoted default. Currently this test passes by accident (lowercase
/// never triggers quotes), which is wrong — the quotes are dropped.
#[test]
fn quoted_lowercase_must_stay_quoted() {
    let out = round_trip("SELECT * FROM \"mytable\"");
    assert!(
        out.contains("\"mytable\""),
        "quoted lowercase identifier must preserve quotes (case-sensitive intent); got: {}",
        out
    );
}
