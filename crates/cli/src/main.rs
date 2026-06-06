use clap::{Parser as ClapParser, Subcommand, ValueEnum};
use metamorphosis_core::extractor::extract_schema_from_dir;
use metamorphosis_core::types::RewriteAction;
use metamorphosis_core::{RewriteConfig, RewriteContext, RewriteEngine, RuleRegistry};
use ogsql_parser::analyzer::schema::SchemaMap;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::{ParseOptions, Parser, StatementInfo};
use std::path::{Path, PathBuf};

mod provenance;
mod verify_cmd;

#[derive(ValueEnum, Clone, Debug)]
enum OutputFormat {
    Text,
    Json,
    Tsv,
    Csv,
    /// Only output the generated probe SQL (one statement per entry, `;` terminated)
    #[clap(name = "sql-only")]
    SqlOnly,
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::Text => write!(f, "text"),
            OutputFormat::Json => write!(f, "json"),
            OutputFormat::Tsv => write!(f, "tsv"),
            OutputFormat::Csv => write!(f, "csv"),
            OutputFormat::SqlOnly => write!(f, "sql-only"),
        }
    }
}

#[derive(ClapParser)]
#[command(
    name = "metamorphosis",
    version,
    about = "SQL semantic rewriting & data quality probe engine"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Rewrite SQL using Safe (and optionally Conditional) rules
    Rewrite {
        file: PathBuf,
        #[arg(long)]
        version: Option<String>,
        #[arg(long, conflicts_with = "sql_dir")]
        schema: Option<PathBuf>,
        #[arg(long, conflicts_with = "schema")]
        sql_dir: Option<PathBuf>,
        #[arg(long)]
        rules: Option<String>,
        #[arg(long)]
        procedure: Option<PathBuf>,
        #[arg(long)]
        from_procedure: bool,
    },
    /// Generate suggestions using Manual rules (never rewrites)
    Suggest {
        file: PathBuf,
        #[arg(long)]
        version: Option<String>,
        #[arg(long, conflicts_with = "sql_dir")]
        schema: Option<PathBuf>,
        #[arg(long, conflicts_with = "schema")]
        sql_dir: Option<PathBuf>,
        #[arg(short = 'o', default_value_t = OutputFormat::Text)]
        output: OutputFormat,
        #[arg(long)]
        procedure: Option<PathBuf>,
        #[arg(long)]
        from_procedure: bool,
    },
    /// Verify semantic equivalence of two SQL queries using Z3 SMT solver
    Verify {
        /// Original SQL file
        original: PathBuf,
        /// Rewritten SQL file
        rewritten: PathBuf,
        #[arg(long, conflicts_with = "sql_dir")]
        schema: Option<PathBuf>,
        #[arg(long, conflicts_with = "schema")]
        sql_dir: Option<PathBuf>,
        #[arg(short = 'o', long, default_value = "text")]
        output: String,
    },
}

fn main() {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Command::Rewrite {
            file,
            version,
            schema,
            sql_dir,
            rules,
            procedure,
            from_procedure,
        } => run_rewrite(
            file,
            version.as_deref(),
            schema,
            sql_dir,
            rules,
            procedure,
            from_procedure,
        ),
        Command::Suggest {
            file,
            version,
            schema,
            sql_dir,
            output,
            procedure,
            from_procedure,
        } => run_suggest(
            file,
            version.as_deref(),
            schema,
            sql_dir,
            output,
            procedure,
            from_procedure,
        ),
        Command::Verify {
            original,
            rewritten,
            schema,
            sql_dir,
            output,
        } => verify_cmd::run_verify(original, rewritten, schema, sql_dir, &output),
    }
}

fn load_sql(file: &Path) -> String {
    std::fs::read_to_string(file).unwrap_or_else(|e| {
        eprintln!("Error: cannot read '{}': {}", file.display(), e);
        std::process::exit(1);
    })
}

fn load_schema(schema_path: Option<PathBuf>, sql_dir: Option<PathBuf>) -> Option<SchemaMap> {
    match (schema_path, sql_dir) {
        (Some(p), None) => {
            let content = std::fs::read_to_string(&p).unwrap_or_else(|e| {
                eprintln!("Error: cannot read schema '{}': {}", p.display(), e);
                std::process::exit(1);
            });
            Some(serde_json::from_str(&content).unwrap_or_else(|e| {
                eprintln!("Error: invalid schema JSON '{}': {}", p.display(), e);
                std::process::exit(1);
            }))
        }
        (None, Some(dir)) => match extract_schema_from_dir(&dir) {
            Ok(schema) => {
                eprintln!(
                    "Extracted schema from {} table(s) in '{}'",
                    schema.len(),
                    dir.display()
                );
                Some(schema)
            }
            Err(e) => {
                eprintln!("Error: schema extraction failed: {e}");
                std::process::exit(1);
            }
        },
        (None, None) => None,
        (Some(_), Some(_)) => {
            eprintln!("Error: --schema and --sql-dir are mutually exclusive");
            std::process::exit(1);
        }
    }
}

fn build_engine(rules_opt: Option<String>) -> RewriteEngine {
    let all_rules = metamorphosis_rules::builtin_rules();

    let registry = if let Some(rules_str) = rules_opt {
        let enabled: std::collections::HashSet<String> =
            rules_str.split(',').map(|s| s.trim().to_string()).collect();
        let filtered: Vec<Box<dyn metamorphosis_core::RewriteRule>> = all_rules
            .into_iter()
            .filter(|r| enabled.contains(r.id()))
            .collect();
        RuleRegistry::new(filtered)
    } else {
        RuleRegistry::new(all_rules)
    };

    RewriteEngine::new(registry)
}

fn load_procedure_variables(
    procedure: Option<PathBuf>,
) -> Option<std::collections::HashSet<String>> {
    let path = procedure?;
    let analysis = provenance::analyze_procedure_file(&path);
    if analysis.variables.is_empty() {
        return None;
    }
    Some(analysis.variables)
}

fn run_rewrite(
    file: PathBuf,
    version: Option<&str>,
    schema_path: Option<PathBuf>,
    sql_dir: Option<PathBuf>,
    rules: Option<String>,
    procedure: Option<PathBuf>,
    from_procedure: bool,
) {
    let schema = load_schema(schema_path, sql_dir.clone());
    let engine = build_engine(rules);

    if from_procedure {
        run_rewrite_from_procedure(&file, version, schema.as_ref(), &engine);
    } else {
        run_rewrite_sql_file(
            &file,
            version,
            schema.as_ref(),
            &engine,
            procedure,
            sql_dir.as_ref(),
        );
    }
}

fn run_rewrite_from_procedure(
    file: &Path,
    version: Option<&str>,
    schema: Option<&SchemaMap>,
    engine: &RewriteEngine,
) {
    let analysis = provenance::analyze_procedure_file(file);
    let config = RewriteConfig::default();
    let known_variables = if analysis.variables.is_empty() {
        None
    } else {
        Some(analysis.variables)
    };
    let ctx = RewriteContext {
        version,
        schema,
        config: &config,
        source_file: Some(file.to_str().unwrap_or("unknown")),
        known_variables: known_variables.as_ref(),
    };

    let items = &analysis.extracted_sql;
    if items.is_empty() {
        println!("-- No SQL statements found in procedure");
        return;
    }

    let mut any_rewritten = false;
    for (i, (stmt, prov)) in items.iter().enumerate() {
        let result = engine.rewrite(&ctx, vec![stmt.clone()]);
        let header = provenance::format_provenance_header(
            i + 1,
            provenance::stmt_type_label(stmt),
            Some(&prov.source_file),
            prov.procedure_name.as_deref(),
            prov.start_line,
            prov.end_line,
            items.len(),
        );

        if result.changed {
            any_rewritten = true;
            println!("{}", header);
            for rewritten in &result.statements {
                println!(
                    "{};",
                    SqlFormatter::new()
                        .pretty_print(true)
                        .format_statement(rewritten)
                );
            }
        }
    }

    if !any_rewritten {
        println!("-- No rewrites applied");
        for (i, (stmt, prov)) in items.iter().enumerate() {
            let header = provenance::format_provenance_header(
                i + 1,
                provenance::stmt_type_label(stmt),
                Some(&prov.source_file),
                prov.procedure_name.as_deref(),
                prov.start_line,
                prov.end_line,
                items.len(),
            );
            println!("{} no matching rule", header);
        }
    }
}

fn run_rewrite_sql_file(
    file: &Path,
    version: Option<&str>,
    schema: Option<&SchemaMap>,
    engine: &RewriteEngine,
    procedure: Option<PathBuf>,
    sql_dir: Option<&PathBuf>,
) {
    let sql = load_sql(file);
    let known_variables = load_procedure_variables(procedure);

    let prov_index = sql_dir
        .map(|d| provenance::ProvenanceIndex::build_from_dir(d))
        .unwrap_or_else(provenance::ProvenanceIndex::empty);

    let config = RewriteConfig::default();
    let ctx = RewriteContext {
        version,
        schema,
        config: &config,
        source_file: Some(file.to_str().unwrap_or("unknown")),
        known_variables: known_variables.as_ref(),
    };

    let parse_output = Parser::parse_sql_with_options(
        &sql,
        ParseOptions {
            preserve_comments: false,
            mybatis_params: false,
        },
    );

    if !parse_output.errors.is_empty() {
        for err in &parse_output.errors {
            eprintln!("Parse warning: {:?}", err);
        }
    }

    let stmt_infos = parse_output.statements;
    if stmt_infos.is_empty() {
        println!("-- No statements to process");
        return;
    }

    let source_file_str = file.to_str().unwrap_or("unknown");
    let mut any_rewritten = false;
    for (i, si) in stmt_infos.iter().enumerate() {
        let result = engine.rewrite(&ctx, vec![si.statement.clone()]);
        let header = if prov_index.has_entries() {
            provenance::format_stmtinfo_header_with_lookup(
                &prov_index,
                i + 1,
                si,
                Some(source_file_str),
                stmt_infos.len(),
            )
        } else {
            provenance::format_stmtinfo_header(i + 1, si, Some(source_file_str), stmt_infos.len())
        };

        if result.changed {
            any_rewritten = true;
            println!("{}", header);
            for rewritten in &result.statements {
                println!(
                    "{};",
                    SqlFormatter::new()
                        .pretty_print(true)
                        .format_statement(rewritten)
                );
            }
        }
    }

    if !any_rewritten {
        println!("-- No rewrites applied");
        if stmt_infos.len() > 1 {
            for (i, si) in stmt_infos.iter().enumerate() {
                let header = if prov_index.has_entries() {
                    provenance::format_stmtinfo_header_with_lookup(
                        &prov_index,
                        i + 1,
                        si,
                        Some(source_file_str),
                        stmt_infos.len(),
                    )
                } else {
                    provenance::format_stmtinfo_header(
                        i + 1,
                        si,
                        Some(source_file_str),
                        stmt_infos.len(),
                    )
                };
                println!("{} no matching rule", header);
            }
        }
    }
}

fn run_suggest(
    file: PathBuf,
    version: Option<&str>,
    schema_path: Option<PathBuf>,
    sql_dir: Option<PathBuf>,
    output: OutputFormat,
    procedure: Option<PathBuf>,
    from_procedure: bool,
) {
    let schema = load_schema(schema_path, sql_dir.clone());
    let engine = build_engine(None);

    if from_procedure {
        run_suggest_from_procedure(&file, version, schema.as_ref(), &engine, output);
    } else {
        run_suggest_sql_file(
            &file,
            version,
            schema.as_ref(),
            &engine,
            output,
            procedure,
            sql_dir.as_ref(),
        );
    }
}

fn run_suggest_from_procedure(
    file: &Path,
    version: Option<&str>,
    schema: Option<&SchemaMap>,
    engine: &RewriteEngine,
    output: OutputFormat,
) {
    let analysis = provenance::analyze_procedure_file(file);
    let config = RewriteConfig::default();
    let known_variables = if analysis.variables.is_empty() {
        None
    } else {
        Some(analysis.variables)
    };
    let ctx = RewriteContext {
        version,
        schema,
        config: &config,
        source_file: Some(file.to_str().unwrap_or("unknown")),
        known_variables: known_variables.as_ref(),
    };

    let items = &analysis.extracted_sql;
    if items.is_empty() {
        println!("No SQL statements found in procedure.");
        return;
    }

    match output {
        OutputFormat::Json => {
            let stmts: Vec<_> = items.iter().map(|(s, _)| s.clone()).collect();
            let result = engine.rewrite(&ctx, stmts);
            let suggestions_json = serde_json::to_string_pretty(&result.suggestions)
                .expect("Failed to serialize suggestions");
            println!("{}", suggestions_json);
        }
        OutputFormat::Text => {
            for (i, (stmt, prov)) in items.iter().enumerate() {
                let result = engine.rewrite(&ctx, vec![stmt.clone()]);
                let header = provenance::format_provenance_header(
                    i + 1,
                    provenance::stmt_type_label(stmt),
                    Some(&prov.source_file),
                    prov.procedure_name.as_deref(),
                    prov.start_line,
                    prov.end_line,
                    items.len(),
                );
                println!("{}", header);

                if !result.suggestions.is_empty() {
                    for s in &result.suggestions {
                        print_text_suggestion(s);
                    }
                } else {
                    println!("Result: no matching rule");
                }
            }
        }
        OutputFormat::Tsv => {
            for (i, (stmt, _prov)) in items.iter().enumerate() {
                let result = engine.rewrite(&ctx, vec![stmt.clone()]);
                if i > 0 && !result.suggestions.is_empty() {
                    println!();
                }
                for s in &result.suggestions {
                    print_tsv_suggestion(s);
                }
            }
        }
        OutputFormat::Csv => {
            let all_rule_ids: Vec<&str> = metamorphosis_rules::builtin_rules()
                .iter()
                .map(|r| r.id())
                .collect();
            let mut header = vec!["original_sql".to_string()];
            header.extend(all_rule_ids.iter().map(|s| s.to_string()));
            println!("{}", header.iter().map(|h| csv_escape(h)).collect::<Vec<_>>().join(","));
            for (stmt, _prov) in items.iter() {
                let result = engine.rewrite(&ctx, vec![stmt.clone()]);
                let mut probes = std::collections::HashMap::new();
                for s in &result.suggestions {
                    if let RewriteAction::Generate { ref stmt, .. } = s.action {
                        probes.insert(s.rule_id.as_str(), compress_sql(stmt));
                    }
                }
                let original_sql = compress_sql(stmt);
                let mut row = vec![csv_escape(&original_sql)];
                for rid in &all_rule_ids {
                    row.push(probes.get(*rid).map(|s| csv_escape(s)).unwrap_or_default());
                }
                println!("{}", row.join(","));
            }
        }
        OutputFormat::SqlOnly => {
            for (stmt, _prov) in items.iter() {
                let result = engine.rewrite(&ctx, vec![stmt.clone()]);
                for s in &result.suggestions {
                    print_sql_only_suggestion(s);
                }
            }
        }
    }
}

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

use std::io::IsTerminal;

fn ansi(code: &str) -> &str {
    if std::io::stdout().is_terminal() {
        code
    } else {
        ""
    }
}

fn color_for_confidence(c: &metamorphosis_core::Confidence) -> &'static str {
    use metamorphosis_core::Confidence;
    match c {
        Confidence::High => GREEN,
        Confidence::Medium => YELLOW,
        Confidence::Low => RED,
        _ => DIM,
    }
}

fn csv_escape(s: &str) -> String {
    let needs_quoting = s.contains(',') || s.contains('"') || s.contains('\n');
    if needs_quoting {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn compress_sql(stmt: &ogsql_parser::ast::Statement) -> String {
    SqlFormatter::new()
        .pretty_print(false)
        .format_statement(stmt)
        .replace('\n', " ")
        .replace('\t', " ")
}

fn print_text_suggestion(s: &metamorphosis_core::Suggestion) {
    if let RewriteAction::Generate {
        ref stmt,
        purpose: _,
        ref confidence,
    } = s.action
    {
        let conf_color = color_for_confidence(confidence);
        println!(
            "{}[{}]{}  {}{:?}{}",
            ansi(BOLD), s.rule_id, ansi(RESET), ansi(conf_color), confidence, ansi(RESET)
        );
        println!("{}  {}{}", ansi(DIM), s.rule_description, ansi(RESET));
        println!("{}  ────────── PROBE ──────────{}", ansi(DIM), ansi(RESET));
        let sql = SqlFormatter::new()
            .pretty_print(true)
            .format_statement(stmt);
        for line in sql.lines() {
            println!("  {line}");
        }
        println!("{}  ────────────────────────────{}", ansi(DIM), ansi(RESET));
        println!();
    }
}

fn print_tsv_suggestion(s: &metamorphosis_core::Suggestion) {
    if let RewriteAction::Generate {
        ref stmt,
        ref purpose,
        ref confidence,
    } = s.action
    {
        let confidence_str = format!("{:?}", confidence);
        let sql = SqlFormatter::new()
            .pretty_print(false)
            .format_statement(stmt);
        let sql_oneline = sql.replace('\n', " ");
        println!("{}\t{}\t{}\t{};", s.rule_id, confidence_str, purpose, sql_oneline);
    }
}

fn print_sql_only_suggestion(s: &metamorphosis_core::Suggestion) {
    if let RewriteAction::Generate { ref stmt, .. } = s.action {
        println!(
            "{};",
            SqlFormatter::new()
                .pretty_print(true)
                .format_statement(stmt)
        );
        println!();
    }
}

fn run_suggest_sql_file(
    file: &Path,
    version: Option<&str>,
    schema: Option<&SchemaMap>,
    engine: &RewriteEngine,
    output: OutputFormat,
    procedure: Option<PathBuf>,
    sql_dir: Option<&PathBuf>,
) {
    let sql = load_sql(file);
    let known_variables = load_procedure_variables(procedure);

    let prov_index = sql_dir
        .map(|d| provenance::ProvenanceIndex::build_from_dir(d))
        .unwrap_or_else(provenance::ProvenanceIndex::empty);

    let config = RewriteConfig::default();
    let ctx = RewriteContext {
        version,
        schema,
        config: &config,
        source_file: Some(file.to_str().unwrap_or("unknown")),
        known_variables: known_variables.as_ref(),
    };

    let parse_output = Parser::parse_sql_with_options(&sql, ParseOptions::default());

    if !parse_output.errors.is_empty() {
        for err in &parse_output.errors {
            eprintln!("Parse warning: {:?}", err);
        }
    }

    let stmt_infos: Vec<StatementInfo> = parse_output.statements;
    if stmt_infos.is_empty() {
        println!("No statements to process.");
        return;
    }

    let source_file_str = file.to_str().unwrap_or("unknown");

    match output {
        OutputFormat::Json => {
            let stmts: Vec<_> = stmt_infos.iter().map(|si| si.statement.clone()).collect();
            let result = engine.rewrite(&ctx, stmts);
            let suggestions_json = serde_json::to_string_pretty(&result.suggestions)
                .expect("Failed to serialize suggestions");
            println!("{}", suggestions_json);
        }
        OutputFormat::Text => {
            for (i, si) in stmt_infos.iter().enumerate() {
                let result = engine.rewrite(&ctx, vec![si.statement.clone()]);
                let header = if prov_index.has_entries() {
                    provenance::format_stmtinfo_header_with_lookup(
                        &prov_index,
                        i + 1,
                        si,
                        Some(source_file_str),
                        stmt_infos.len(),
                    )
                } else {
                    provenance::format_stmtinfo_header(i + 1, si, Some(source_file_str), stmt_infos.len())
                };
                println!("{}", header);

                if !result.suggestions.is_empty() {
                    for s in &result.suggestions {
                        print_text_suggestion(s);
                    }
                } else {
                    println!("Result: no matching rule");
                }
            }
        }
        OutputFormat::Tsv => {
            for (i, si) in stmt_infos.iter().enumerate() {
                let result = engine.rewrite(&ctx, vec![si.statement.clone()]);
                if i > 0 && !result.suggestions.is_empty() {
                    println!();
                }
                for s in &result.suggestions {
                    print_tsv_suggestion(s);
                }
            }
        }
        OutputFormat::Csv => {
            let all_rule_ids: Vec<&str> = metamorphosis_rules::builtin_rules()
                .iter()
                .map(|r| r.id())
                .collect();
            let mut header = vec!["original_sql".to_string()];
            header.extend(all_rule_ids.iter().map(|s| s.to_string()));
            println!("{}", header.iter().map(|h| csv_escape(h)).collect::<Vec<_>>().join(","));
            for si in stmt_infos.iter() {
                let result = engine.rewrite(&ctx, vec![si.statement.clone()]);
                let mut probes = std::collections::HashMap::new();
                for s in &result.suggestions {
                    if let RewriteAction::Generate { ref stmt, .. } = s.action {
                        probes.insert(s.rule_id.as_str(), compress_sql(stmt));
                    }
                }
                let original_sql = si.sql_text.replace('\n', " ").replace('\t', " ");
                let mut row = vec![csv_escape(&original_sql)];
                for rid in &all_rule_ids {
                    row.push(probes.get(*rid).map(|s| csv_escape(s)).unwrap_or_default());
                }
                println!("{}", row.join(","));
            }
        }
        OutputFormat::SqlOnly => {
            for si in stmt_infos.iter() {
                let result = engine.rewrite(&ctx, vec![si.statement.clone()]);
                for s in &result.suggestions {
                    print_sql_only_suggestion(s);
                }
            }
        }
    }
}
