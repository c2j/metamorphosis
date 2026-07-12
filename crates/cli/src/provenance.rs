use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

use ogsql_parser::ast::plpgsql::{
    PlBlock, PlDeclaration, PlForKind, PlForStmt, PlOpenKind, PlOpenStmt, PlStatement, PlTypeDecl,
};
use ogsql_parser::ast::{
    CreatePackageBodyStatement, PackageItem, SelectStatement, Statement, TableRef,
};
use ogsql_parser::{Parser, SqlFormatter, StatementInfo};

pub struct SqlProvenance {
    pub source_file: String,
    pub procedure_name: Option<String>,
    pub start_line: usize,
    pub end_line: usize,
}

impl std::fmt::Display for SqlProvenance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.procedure_name {
            Some(proc) => write!(
                f,
                "{}:{} — line {}-{}",
                self.source_file, proc, self.start_line, self.end_line
            ),
            None => write!(
                f,
                "{} — line {}-{}",
                self.source_file, self.start_line, self.end_line
            ),
        }
    }
}

struct ExtractedSql {
    statement: Statement,
    provenance: SqlProvenance,
}

pub struct ProcedureAnalysis {
    pub extracted_sql: Vec<(Statement, SqlProvenance)>,
    pub variables: HashSet<String>,
}

pub fn analyze_procedure_file(path: &Path) -> ProcedureAnalysis {
    let sql = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("Error: cannot read '{}': {}", path.display(), e);
        std::process::exit(1);
    });
    let source_file = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());

    let (stmts, errors) = Parser::parse_sql(&sql);
    if !errors.is_empty() {
        eprintln!(
            "Warning: {} parse error(s) in '{}'",
            errors.len(),
            path.display()
        );
    }

    let mut variables = HashSet::new();
    let mut extracted = Vec::new();

    for si in &stmts {
        match &si.statement {
            Statement::CreateProcedure(p) => {
                let proc_name = p.name.join(".");
                collect_params_and_vars(&p.parameters, p.block.as_ref(), &mut variables);
                if let Some(ref block) = p.block {
                    extract_sql_from_block(block, &source_file, Some(&proc_name), &mut extracted);
                }
            }
            Statement::CreateFunction(f) => {
                let func_name = f.name.join(".");
                collect_params_and_vars(&f.parameters, f.block.as_ref(), &mut variables);
                if let Some(ref block) = f.block {
                    extract_sql_from_block(block, &source_file, Some(&func_name), &mut extracted);
                }
            }
            Statement::CreatePackageBody(pkg) => {
                extract_from_package(pkg, &source_file, &mut variables, &mut extracted);
            }
            _ => {}
        }
    }

    if variables.is_empty() {
        eprintln!("Warning: no variables found in '{}'", path.display());
    } else {
        eprintln!(
            "Extracted {} variable(s) from '{}'",
            variables.len(),
            path.display()
        );
    }

    if extracted.is_empty() {
        eprintln!("Warning: no SQL statements found in '{}'", path.display());
    } else {
        eprintln!(
            "Extracted {} SQL statement(s) from '{}'",
            extracted.len(),
            path.display()
        );
    }

    ProcedureAnalysis {
        extracted_sql: extracted
            .into_iter()
            .map(|e| (e.statement, e.provenance))
            .collect(),
        variables,
    }
}

fn extract_from_package(
    pkg: &CreatePackageBodyStatement,
    source_file: &str,
    variables: &mut HashSet<String>,
    extracted: &mut Vec<ExtractedSql>,
) {
    for item in &pkg.items {
        match item {
            PackageItem::Procedure(p) => {
                let proc_name = p.name.join(".");
                collect_params_and_vars(&p.parameters, p.block.as_ref(), variables);
                if let Some(ref block) = p.block {
                    extract_sql_from_block(block, source_file, Some(&proc_name), extracted);
                }
            }
            PackageItem::Function(f) => {
                let func_name = f.name.join(".");
                collect_params_and_vars(&f.parameters, f.block.as_ref(), variables);
                if let Some(ref block) = f.block {
                    extract_sql_from_block(block, source_file, Some(&func_name), extracted);
                }
            }
            PackageItem::Variable(v) => {
                variables.insert(v.name.to_lowercase());
            }
            _ => {}
        }
    }
}

fn collect_params_and_vars(
    params: &[ogsql_parser::ast::RoutineParam],
    block: Option<&PlBlock>,
    vars: &mut HashSet<String>,
) {
    for param in params {
        vars.insert(param.name.to_lowercase());
    }
    if let Some(block) = block {
        collect_block_vars(block, vars);
    }
}

pub fn collect_block_vars(block: &PlBlock, vars: &mut HashSet<String>) {
    for decl in &block.declarations {
        match decl {
            PlDeclaration::Variable(v) => {
                vars.insert(v.name.to_lowercase());
            }
            PlDeclaration::Cursor(c) => {
                vars.insert(c.name.to_lowercase());
            }
            PlDeclaration::Record(r) => {
                vars.insert(r.name.to_lowercase());
            }
            PlDeclaration::Type(t) => match t {
                PlTypeDecl::Record { name, .. }
                | PlTypeDecl::TableOf { name, .. }
                | PlTypeDecl::VarrayOf { name, .. }
                | PlTypeDecl::RefCursor { name } => {
                    vars.insert(name.to_lowercase());
                }
            },
            PlDeclaration::NestedProcedure(p) => {
                for param in &p.parameters {
                    vars.insert(param.name.to_lowercase());
                }
            }
            PlDeclaration::NestedFunction(f) => {
                for param in &f.parameters {
                    vars.insert(param.name.to_lowercase());
                }
            }
            _ => {}
        }
    }
}

fn extract_sql_from_block(
    block: &PlBlock,
    source_file: &str,
    procedure_name: Option<&str>,
    results: &mut Vec<ExtractedSql>,
) {
    for stmt in &block.body {
        extract_sql_from_pl_stmt(stmt, source_file, procedure_name, results);
    }
    if let Some(ref exc) = block.exception_block {
        for handler in &exc.handlers {
            for stmt in &handler.statements {
                extract_sql_from_pl_stmt(stmt, source_file, procedure_name, results);
            }
        }
    }
}

fn extract_sql_from_pl_stmt(
    pl_stmt: &PlStatement,
    source_file: &str,
    procedure_name: Option<&str>,
    results: &mut Vec<ExtractedSql>,
) {
    match pl_stmt {
        PlStatement::SqlStatement {
            span,
            sql_text: _,
            statement,
        } => {
            let (start, end) = span_lines(span);
            results.push(ExtractedSql {
                statement: (**statement).clone(),
                provenance: SqlProvenance {
                    source_file: source_file.to_string(),
                    procedure_name: procedure_name.map(|s| s.to_string()),
                    start_line: start,
                    end_line: end,
                },
            });
        }

        PlStatement::Execute(spanned_exec) => {
            if let Some(ref query) = spanned_exec.parsed_query {
                let (start, end) = span_lines_from_spanned(&spanned_exec.span);
                results.push(ExtractedSql {
                    statement: (**query).clone(),
                    provenance: SqlProvenance {
                        source_file: source_file.to_string(),
                        procedure_name: procedure_name.map(|s| s.to_string()),
                        start_line: start,
                        end_line: end,
                    },
                });
            }
        }

        PlStatement::Perform {
            span,
            parsed_query: Some(ref query),
            ..
        } => {
            let (start, end) = span_lines(span);
            results.push(ExtractedSql {
                statement: (**query).clone(),
                provenance: SqlProvenance {
                    source_file: source_file.to_string(),
                    procedure_name: procedure_name.map(|s| s.to_string()),
                    start_line: start,
                    end_line: end,
                },
            });
        }

        PlStatement::Open(spanned_open) => {
            extract_from_open(spanned_open, source_file, procedure_name, results);
        }

        PlStatement::For(spanned_for) => {
            extract_from_for(spanned_for, source_file, procedure_name, results);
        }

        PlStatement::Block(spanned_block) => {
            extract_sql_from_block(spanned_block, source_file, procedure_name, results);
        }

        PlStatement::If(spanned_if) => {
            for stmt in &spanned_if.then_stmts {
                extract_sql_from_pl_stmt(stmt, source_file, procedure_name, results);
            }
            for elsif in &spanned_if.elsifs {
                for stmt in &elsif.stmts {
                    extract_sql_from_pl_stmt(stmt, source_file, procedure_name, results);
                }
            }
            for stmt in &spanned_if.else_stmts {
                extract_sql_from_pl_stmt(stmt, source_file, procedure_name, results);
            }
        }

        PlStatement::Case(spanned_case) => {
            for when in &spanned_case.whens {
                for stmt in &when.stmts {
                    extract_sql_from_pl_stmt(stmt, source_file, procedure_name, results);
                }
            }
            for stmt in &spanned_case.else_stmts {
                extract_sql_from_pl_stmt(stmt, source_file, procedure_name, results);
            }
        }

        PlStatement::Loop(spanned_loop) => {
            for stmt in &spanned_loop.body {
                extract_sql_from_pl_stmt(stmt, source_file, procedure_name, results);
            }
        }

        PlStatement::While(spanned_while) => {
            for stmt in &spanned_while.body {
                extract_sql_from_pl_stmt(stmt, source_file, procedure_name, results);
            }
        }

        PlStatement::ForEach(spanned_foreach) => {
            for stmt in &spanned_foreach.body {
                extract_sql_from_pl_stmt(stmt, source_file, procedure_name, results);
            }
        }

        _ => {}
    }
}

fn extract_from_open(
    spanned_open: &ogsql_parser::ast::Spanned<PlOpenStmt>,
    source_file: &str,
    procedure_name: Option<&str>,
    results: &mut Vec<ExtractedSql>,
) {
    if let PlOpenKind::ForQuery {
        parsed_query: Some(ref query),
        ..
    } = &spanned_open.kind
    {
        let (start, end) = span_lines_from_spanned(&spanned_open.span);
        results.push(ExtractedSql {
            statement: (**query).clone(),
            provenance: SqlProvenance {
                source_file: source_file.to_string(),
                procedure_name: procedure_name.map(|s| s.to_string()),
                start_line: start,
                end_line: end,
            },
        });
    }
}

fn extract_from_for(
    spanned_for: &ogsql_parser::ast::Spanned<PlForStmt>,
    source_file: &str,
    procedure_name: Option<&str>,
    results: &mut Vec<ExtractedSql>,
) {
    if let PlForKind::Query {
        parsed_query: Some(ref query),
        ..
    } = &spanned_for.kind
    {
        let (start, end) = span_lines_from_spanned(&spanned_for.span);
        results.push(ExtractedSql {
            statement: (**query).clone(),
            provenance: SqlProvenance {
                source_file: source_file.to_string(),
                procedure_name: procedure_name.map(|s| s.to_string()),
                start_line: start,
                end_line: end,
            },
        });
    }
    for stmt in &spanned_for.body {
        extract_sql_from_pl_stmt(stmt, source_file, procedure_name, results);
    }
}

fn span_lines(span: &Option<ogsql_parser::ast::SourceSpan>) -> (usize, usize) {
    match span {
        Some(s) => (s.start.line, s.end.line),
        None => (0, 0),
    }
}

fn span_lines_from_spanned(span: &Option<ogsql_parser::ast::SourceSpan>) -> (usize, usize) {
    span_lines(span)
}

pub fn format_provenance_header(
    index: usize,
    stmt_type: &str,
    source_file: Option<&str>,
    procedure_name: Option<&str>,
    start_line: usize,
    end_line: usize,
    total: usize,
) -> String {
    let idx = if total > 1 {
        format!("Statement {} ", index)
    } else {
        String::new()
    };

    let location = if start_line > 0 {
        if start_line == end_line {
            format!("line {}", start_line)
        } else {
            format!("line {}-{}", start_line, end_line)
        }
    } else {
        String::new()
    };

    let proc = procedure_name
        .map(|p| format!("{}:", p))
        .unwrap_or_default();

    let file = source_file
        .map(|f| {
            let name = std::path::Path::new(f)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| f.to_string());
            format!("{}:", name)
        })
        .unwrap_or_default();

    let parts: Vec<&str> = [&idx, "(", stmt_type, ") ", &file, &proc, &location]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect();

    format!("--- {} ---", parts.join(""))
}

pub fn format_stmtinfo_header(
    index: usize,
    si: &StatementInfo,
    source_file: Option<&str>,
    total: usize,
) -> String {
    format_provenance_header(
        index,
        stmt_type_label(&si.statement),
        source_file,
        None,
        si.start_line,
        si.end_line,
        total,
    )
}

pub fn stmt_type_label(stmt: &Statement) -> &'static str {
    match stmt {
        Statement::Select(_) => "SELECT",
        Statement::Insert(_) | Statement::InsertAll(_) | Statement::InsertFirst(_) => "INSERT",
        Statement::Update(_) => "UPDATE",
        Statement::Delete(_) => "DELETE",
        Statement::Merge(_) => "MERGE",
        Statement::CreateTable { .. } => "CREATE TABLE",
        _ => "SQL",
    }
}

struct IndexedSql {
    table_names: HashSet<String>,
    provenance: SqlProvenance,
}

pub struct ProvenanceIndex {
    by_fingerprint: HashMap<u64, Vec<IndexedSql>>,
    all_entries: Vec<IndexedSql>,
}

impl ProvenanceIndex {
    pub fn empty() -> Self {
        ProvenanceIndex {
            by_fingerprint: HashMap::new(),
            all_entries: Vec::new(),
        }
    }

    pub fn has_entries(&self) -> bool {
        !self.all_entries.is_empty()
    }

    pub fn build_from_dir(sql_dir: &Path) -> Self {
        let mut index = ProvenanceIndex {
            by_fingerprint: HashMap::new(),
            all_entries: Vec::new(),
        };

        let files = match collect_sql_files(sql_dir) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Warning: cannot scan sql-dir for procedures: {}", e);
                return index;
            }
        };

        if files.is_empty() {
            return index;
        }

        eprintln!(
            "Scanning {} file(s) in '{}' for procedure SQL...",
            files.len(),
            sql_dir.display()
        );

        let mut total_sql = 0usize;
        for path in &files {
            let analysis = analyze_procedure_file_silent(path);
            for (stmt, prov) in &analysis.extracted_sql {
                let fp = fingerprint_statement(stmt);
                let tables = collect_table_names(stmt);
                index
                    .by_fingerprint
                    .entry(fp)
                    .or_default()
                    .push(IndexedSql {
                        table_names: tables.clone(),
                        provenance: SqlProvenance {
                            source_file: prov.source_file.clone(),
                            procedure_name: prov.procedure_name.clone(),
                            start_line: prov.start_line,
                            end_line: prov.end_line,
                        },
                    });
                index.all_entries.push(IndexedSql {
                    table_names: tables,
                    provenance: SqlProvenance {
                        source_file: prov.source_file.clone(),
                        procedure_name: prov.procedure_name.clone(),
                        start_line: prov.start_line,
                        end_line: prov.end_line,
                    },
                });
                total_sql += 1;
            }
        }

        if total_sql > 0 {
            eprintln!(
                "Indexed {} SQL statement(s) from {} procedure file(s)",
                total_sql,
                files.len()
            );
        }

        index
    }

    pub fn lookup(&self, stmt: &Statement) -> Option<&SqlProvenance> {
        let fp = fingerprint_statement(stmt);
        if let Some(entries) = self.by_fingerprint.get(&fp) {
            return Some(&entries[0].provenance);
        }

        let input_tables = collect_table_names(stmt);
        if input_tables.is_empty() {
            return None;
        }

        let mut best: Option<(&IndexedSql, usize)> = None;
        for entry in &self.all_entries {
            let common = input_tables.intersection(&entry.table_names).count();
            if common == 0 {
                continue;
            }
            let input_coverage = common * 100 / input_tables.len();
            if input_coverage < 50 {
                continue;
            }
            let proc_coverage = common * 100 / entry.table_names.len().max(1);
            let score = input_coverage * 2 + proc_coverage;
            match best {
                Some((_, best_score)) if score <= best_score => {}
                _ => best = Some((entry, score)),
            }
        }

        best.map(|(e, _)| &e.provenance)
    }
}

fn collect_sql_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    metamorphosis_core::extractor::collect_sql_files(dir).map_err(|e| e.to_string())
}

fn analyze_procedure_file_silent(path: &Path) -> ProcedureAnalysis {
    let sql = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => {
            return ProcedureAnalysis {
                extracted_sql: Vec::new(),
                variables: HashSet::new(),
            }
        }
    };
    let source_file = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let (stmts, _errors) = Parser::parse_sql(&sql);

    let mut variables = HashSet::new();
    let mut extracted = Vec::new();

    for si in &stmts {
        match &si.statement {
            Statement::CreateProcedure(p) => {
                let proc_name = p.name.join(".");
                collect_params_and_vars(&p.parameters, p.block.as_ref(), &mut variables);
                if let Some(ref block) = p.block {
                    extract_sql_from_block(block, &source_file, Some(&proc_name), &mut extracted);
                }
            }
            Statement::CreateFunction(f) => {
                let func_name = f.name.join(".");
                collect_params_and_vars(&f.parameters, f.block.as_ref(), &mut variables);
                if let Some(ref block) = f.block {
                    extract_sql_from_block(block, &source_file, Some(&func_name), &mut extracted);
                }
            }
            Statement::CreatePackageBody(pkg) => {
                extract_from_package(pkg, &source_file, &mut variables, &mut extracted);
            }
            _ => {}
        }
    }

    ProcedureAnalysis {
        extracted_sql: extracted
            .into_iter()
            .map(|e| (e.statement, e.provenance))
            .collect(),
        variables,
    }
}

fn fingerprint_statement(stmt: &Statement) -> u64 {
    let normalized = SqlFormatter::new().format_statement(stmt);
    let mut hasher = DefaultHasher::new();
    normalized.hash(&mut hasher);
    hasher.finish()
}

fn collect_table_names(stmt: &Statement) -> HashSet<String> {
    let mut tables = HashSet::new();
    match stmt {
        Statement::Select(sel) => collect_from_select(sel, &mut tables),
        Statement::Insert(ins) => {
            let name = ins
                .table
                .last()
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            if !name.is_empty() {
                tables.insert(name);
            }
        }
        Statement::Update(upd) => {
            for tr in &upd.tables {
                collect_from_table_ref(tr, &mut tables);
            }
            for tr in &upd.from {
                collect_from_table_ref(tr, &mut tables);
            }
        }
        Statement::Delete(del) => {
            for tr in &del.tables {
                collect_from_table_ref(tr, &mut tables);
            }
            for tr in &del.using {
                collect_from_table_ref(tr, &mut tables);
            }
        }
        _ => {}
    }
    tables
}

fn collect_from_select(sel: &SelectStatement, tables: &mut HashSet<String>) {
    for from in &sel.from {
        collect_from_table_ref(from, tables);
    }
}

fn collect_from_table_ref(tr: &TableRef, tables: &mut HashSet<String>) {
    match tr {
        TableRef::Table { name, .. } => {
            let table_name = name.last().map(|s| s.to_lowercase()).unwrap_or_default();
            if !table_name.is_empty() {
                tables.insert(table_name);
            }
        }
        TableRef::Subquery { query, .. } => {
            collect_from_select(query, tables);
        }
        TableRef::Join { left, right, .. } => {
            collect_from_table_ref(left, tables);
            collect_from_table_ref(right, tables);
        }
        TableRef::Pivot { source, .. } | TableRef::Unpivot { source, .. } => {
            collect_from_table_ref(source, tables);
        }
        TableRef::FunctionCall { .. } | TableRef::Values { .. } => {}
    }
}

pub fn format_stmtinfo_header_with_lookup(
    index: &ProvenanceIndex,
    i: usize,
    si: &StatementInfo,
    source_file: Option<&str>,
    total: usize,
) -> String {
    let base = format_stmtinfo_header(i, si, source_file, total);
    if let Some(prov) = index.lookup(&si.statement) {
        let proc = prov.procedure_name.as_deref().unwrap_or("");
        if prov.start_line > 0 {
            let loc = if prov.start_line == prov.end_line {
                format!("line {}", prov.start_line)
            } else {
                format!("line {}-{}", prov.start_line, prov.end_line)
            };
            if proc.is_empty() {
                format!("{} ← {}:{}", base, prov.source_file, loc)
            } else {
                format!("{} ← {}:{}:{}", base, prov.source_file, proc, loc)
            }
        } else {
            base
        }
    } else {
        base
    }
}
