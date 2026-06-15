use std::fs;

fn main() {
    let dir =
        std::path::Path::new("/Users/c2j/Projects/Desktop_Projects/DB/metamorphosis/testcases");
    let sql_files = [
        "orig.sql",
        "rewrite.sql",
        "case-wenhao.sql",
        "case1.sql",
        "case4.sql",
    ];

    let mut total = 0usize;
    let mut parsed = 0usize;
    let mut translated = 0usize;
    let mut failed = Vec::new();

    for fname in &sql_files {
        let path = dir.join(fname);
        let content = fs::read_to_string(&path).unwrap();

        // Split by semicolons, filter empty/comments
        let stmts: Vec<String> = content
            .split(';')
            .map(|s| s.trim())
            .filter(|s| {
                let clean: String = s
                    .lines()
                    .filter(|l| !l.trim().starts_with("--"))
                    .collect::<Vec<_>>()
                    .join("\n");
                !clean.trim().is_empty()
            })
            .map(|s| s.to_string())
            .collect();

        println!("=== {} ({} statements) ===", fname, stmts.len());

        for (i, sql) in stmts.iter().enumerate() {
            total += 1;
            let preview = if sql.len() > 80 {
                format!("{}...", &sql[..77])
            } else {
                sql.clone()
            };
            print!("  [{:02}] {:80}", i, preview);

            match ogsql_parser::Tokenizer::new(sql).tokenize() {
                Ok(tokens) => {
                    let mut parser = ogsql_parser::parser::Parser::new(tokens);
                    let ast = parser.parse();
                    if ast.is_empty() {
                        println!("SKIP (no statements)");
                        continue;
                    }
                    parsed += 1;

                    match metamorphosis_verieql::translator::translate(&ast[0]) {
                        Ok(_) => {
                            translated += 1;
                            println!("OK");
                        }
                        Err(e) => {
                            failed.push((fname.to_string(), i, format!("{}", e)));
                            println!("TRANSLATE_ERR: {}", e);
                        }
                    }
                }
                Err(e) => {
                    failed.push((fname.to_string(), i, format!("PARSE: {}", e)));
                    println!("PARSE_ERR: {}", e);
                }
            }
        }
        println!();
    }

    println!("========================================");
    println!("Total statements: {}", total);
    println!("Parsed by ogsql-parser: {}/{}", parsed, total);
    println!("Translated to IR: {}/{}", translated, total);
    println!("Failed: {}/{}", failed.len(), total);

    if !failed.is_empty() {
        println!("\nFailed details:");
        for (f, i, e) in &failed {
            println!("  {}[{}]: {}", f, i, e);
        }
    }
}
