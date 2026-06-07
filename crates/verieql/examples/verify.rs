use metamorphosis_verieql::{types::*, VeriEql};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: cargo run --example verify <sql1> <sql2> [bound]");
        eprintln!();
        eprintln!("Example:");
        eprintln!("  cargo run -p metamorphosis-verieql --example verify \\");
        eprintln!("    'SELECT ID FROM EMP' 'SELECT ID FROM EMP'");
        std::process::exit(1);
    }

    let sql1 = &args[1];
    let sql2 = &args[2];
    let bound: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2);

    let schema = vec![TableSchema {
        name: "EMP".into(),
        columns: vec![
            ColumnDef {
                name: "ID".into(),
                col_type: ColumnType::Integer,
            },
            ColumnDef {
                name: "NAME".into(),
                col_type: ColumnType::Varchar,
            },
            ColumnDef {
                name: "DEPT".into(),
                col_type: ColumnType::Integer,
            },
        ],
    }];

    println!("SQL 1: {sql1}");
    println!("SQL 2: {sql2}");
    println!("Bound: {bound}");
    println!();

    match VeriEql::verify(
        sql1,
        sql2,
        &schema,
        &serde_json::json!(null),
        Bound(bound),
        Semantics::Bag,
    ) {
        Ok(report) => {
            println!("Result: {:?}", report.result);
            println!(
                "Translate: {}ms, Solve: {}ms",
                report.translate_ms, report.solve_ms
            );
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}
