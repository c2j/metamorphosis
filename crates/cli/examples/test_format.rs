use ogsql_parser::{Parser, SqlFormatter};
fn main() {
    // SQL from case1.sql (standalone - variables as ColumnRef)
    let standalone = r#"SELECT t.trade_code
FROM dat_clr_cash_dtl t
WHERE t.account_date = in_accnt_date
  AND t.account_seqno = in_seq_no
  AND t.account_id = in_accnt_id
  AND t.interface_seq = in_interface_seq"#;

    let (stmts, _) = Parser::parse_sql(standalone);
    if let Some(si) = stmts.first() {
        let fmt = SqlFormatter::new();
        println!("=== Standalone formatted ===");
        println!("{}", fmt.format_statement(&si.statement));
    }

    // SQL from procedure body (variables as PlVariable)
    let proc_sql = r#"CREATE OR REPLACE PROCEDURE test_proc(in_accnt_date VARCHAR) IS
BEGIN
SELECT t.trade_code
INTO v_trade_code
FROM dat_clr_cash_dtl t
WHERE t.account_date = in_accnt_date
  AND t.account_seqno = in_seq_no
  AND t.account_id = in_accnt_id
  AND t.interface_seq = in_interface_seq;
END;"#;

    let (stmts2, _) = Parser::parse_sql(proc_sql);
    if let Some(si) = stmts2.first() {
        if let ogsql_parser::ast::Statement::CreateProcedure(p) = &si.statement {
            if let Some(ref block) = p.block {
                for s in &block.body {
                    if let ogsql_parser::ast::plpgsql::PlStatement::SqlStatement {
                        statement, ..
                    } = s
                    {
                        let fmt = SqlFormatter::new();
                        println!("\n=== Procedure body formatted ===");
                        println!("{}", fmt.format_statement(statement));
                    }
                }
            }
        }
    }
}
