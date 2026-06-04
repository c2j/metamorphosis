# Metamorphosis MVP Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use executing-plans to implement this plan task-by-task.

**Goal:** Build Metamorphosis engine skeleton + one Safe rule + one Manual rule + CLI, end-to-end.

**Architecture:** 4-layer Cargo workspace — `core/` (engine + abstractions), `rules/` (built-in rules), `cli/` (CLI entrypoint), plus `docs/`. Consumes ogsql-parser AST without parsing SQL directly. Engine iterates statements with `max_iterations` loop prevention, dispatches rules by SafetyLevel (Safe auto-executes, Manual collects suggestions).

**Tech Stack:** Rust 2021 edition, ogsql-parser v0.6.5 (sibling crate), clap 4 (CLI), thiserror (library errors), tracing (logging), rustfmt (formatting).

**Key Constraints from docs/CONTRIBUTING.md:**
- Core crate: zero IO dependencies, no `unwrap()` in lib, use `thiserror` not `anyhow`
- All `pub` items need doc comments, `#[non_exhaustive]` on exported structs/enums
- Max 600 lines per `.rs` (entry files ≤200), rustfmt enforced, no bare `as` casts
- `tracing` for logging (not `log`)

**ogsql-parser API Notes:**
- `Statement` variants wrapped in `Spanned<T>` (implements `Deref<Target=T>`)
- `SelectTarget::Expr(Expr, Option<String>)` and `SelectTarget::Star(Option<String>)`
- `SelectTarget` NOT re-exported from ogsql-parser lib.rs — use `ogsql_parser::ast::SelectTarget`
- `ParseOutput { statements: Vec<StatementInfo>, errors, comments }` — extract `.statement` field
- `Spanned::without_span(node)` to construct AST nodes without source location

---

### Task 1: Project Scaffolding & Cargo Workspace

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `core/Cargo.toml`
- Create: `core/src/lib.rs`
- Create: `rules/Cargo.toml`
- Create: `rules/src/lib.rs`
- Create: `cli/Cargo.toml`
- Create: `cli/src/main.rs`

**Step 1: Create workspace Cargo.toml**

```toml
[workspace]
members = ["core", "rules", "cli"]
resolver = "2"

[workspace.package]
edition = "2021"
rust-version = "1.75"
license = "MIT OR Apache-2.0"
```

**Step 2: Create core/Cargo.toml**

```toml
[package]
name = "metamorphosis-core"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
ogsql-parser = { path = "../../ogsql-parser" }
thiserror = "2"
serde = { version = "1", features = ["derive"] }
tracing = "0.1"
```

**Step 3: Create core/src/lib.rs** (placeholder)

```rust
//! Metamorphosis — SQL semantic rewriting & data quality probe engine.
//!
//! Consumes AST output from `ogsql-parser` (never parses SQL directly),
//! applies pluggable rewrite rules, and produces diagnostic/rewritten SQL.
//! Also provides safety-gated rule system (Safe / Conditional / Manual).

pub mod context;
pub mod engine;
pub mod registry;
pub mod types;

// Re-exports
pub use context::{RewriteConfig, RewriteContext};
pub use engine::RewriteEngine;
pub use registry::{RewriteRule, RuleCategory, RuleRegistry};
pub use types::{Confidence, RewriteAction, RewriteResult, SafetyLevel, Suggestion};
```

**Step 4: Create rules/Cargo.toml**

```toml
[package]
name = "metamorphosis-rules"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
metamorphosis-core = { path = "../core" }
ogsql-parser = { path = "../../ogsql-parser" }
tracing = "0.1"
```

**Step 5: Create rules/src/lib.rs** (placeholder)

```rust
//! Built-in rewrite rules for Metamorphosis.
//!
//! Each rule implements `RewriteRule` from `metamorphosis-core`.

pub mod eliminate_select_star;
pub mod detect_duplicate_eq_keys;

use metamorphosis_core::{RewriteRule, RuleCategory};
use std::fmt::Debug;

/// Returns all built-in rules for registration.
pub fn builtin_rules() -> Vec<Box<dyn RewriteRule>> {
    vec![
        Box::new(eliminate_select_star::EliminateSelectStar),
        Box::new(detect_duplicate_eq_keys::DetectDuplicateEqKeys),
    ]
}
```

**Step 6: Create cli/Cargo.toml**

```toml
[package]
name = "metamorphosis-cli"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
metamorphosis-core = { path = "../core" }
metamorphosis-rules = { path = "../rules" }
ogsql-parser = { path = "../../ogsql-parser" }
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
```

**Step 7: Create cli/src/main.rs** (placeholder)

```rust
//! Metamorphosis CLI — SQL rewriting and suggestion commands.
//!
//! # Usage
//! ```bash
//! metamorphosis rewrite query.sql --version 5.0 --schema schema.json
//! metamorphosis suggest query.sql --version 5.0 -o json
//! ```

fn main() {
    println!("Metamorphosis — SQL semantic rewriting engine");
}
```

**Step 8: Verify compilation**

Run: `cargo check --workspace`
Expected: builds without errors (allow unused warnings for now)

---

### Task 2: Core Types & Abstractions

**Files:**
- Create: `core/src/types.rs`
- Create: `core/src/context.rs`
- Create: `core/src/registry.rs`
- Create: `core/src/engine.rs`
- Modify: `core/src/lib.rs`

**Step 1: Create core/src/types.rs**

```rust
use ogsql_parser::ast::Statement;
use serde::{Deserialize, Serialize};

/// Safety level determines how the engine handles a rule's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SafetyLevel {
    /// Semantically equivalent rewrite — engine auto-executes.
    Safe,
    /// Semantically equivalent only when preconditions are met — engine
    /// verifies preconditions before executing.
    Conditional,
    /// Not semantically equivalent — generates suggestions only, never
    /// replaces the original statement automatically.
    Manual,
}

/// Confidence level for a rewrite result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Confidence {
    /// Single table, no subqueries, pure literal equality — rewrite is
    /// deterministic.
    High,
    /// Penetrated a derived table or removed EXISTS — structural change
    /// but semantics are traceable.
    Medium,
    /// Multi-table JOIN, dynamic subqueries — result requires human review.
    Low,
}

/// Action produced by a rule after matching.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RewriteAction {
    /// Semantically equivalent replacement for the original statement.
    Replace(Box<Statement>),
    /// Generates a derived SQL (e.g., data quality probe) that coexists
    /// with the original rather than replacing it.
    Generate {
        stmt: Box<Statement>,
        purpose: String,
        confidence: Confidence,
    },
    /// Text-only suggestion, does not produce an AST.
    Suggest {
        message: String,
        severity: Severity,
    },
}

/// Severity level for text suggestions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

/// Result of rewriting a set of statements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewriteResult {
    /// Rewritten statements (Safe / Conditional level).
    pub statements: Vec<Statement>,
    /// Manual-level suggestions requiring human review.
    pub suggestions: Vec<Suggestion>,
    /// Whether any rewrite occurred.
    pub changed: bool,
}

/// A single suggestion produced by a Manual-level rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub rule_id: String,
    pub rule_description: String,
    pub action: RewriteAction,
    pub confidence: Confidence,
    pub notes: Vec<String>,
}

/// Category for grouping and filtering rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RuleCategory {
    Performance,
    DataQuality,
    Style,
    Semantic,
    Safety,
}
```

**Step 2: Create core/src/context.rs**

```rust
use ogsql_parser::analyzer::SchemaMap;
use std::collections::HashSet;

/// Configuration for the rewrite engine.
#[derive(Debug, Clone)]
pub struct RewriteConfig {
    /// Rules explicitly enabled (empty = all enabled).
    pub enabled_rules: HashSet<String>,
    /// Rules explicitly disabled.
    pub disabled_rules: HashSet<String>,
    /// Maximum rewrite iterations per statement (loop prevention).
    pub max_iterations: usize,
    /// Whether to preserve comments (requires ogsql-parser trivia support).
    pub preserve_comments: bool,
    /// Default LIMIT for generated probe SQL.
    pub probe_default_limit: usize,
}

impl Default for RewriteConfig {
    fn default() -> Self {
        Self {
            enabled_rules: HashSet::new(),
            disabled_rules: HashSet::new(),
            max_iterations: 10,
            preserve_comments: false,
            probe_default_limit: 10,
        }
    }
}

/// Context provided to each rule during matching and application.
#[derive(Debug, Clone)]
pub struct RewriteContext<'a> {
    /// Database version (for version-gated rules).
    pub version: Option<&'a str>,
    /// Table schema information (for SELECT * expansion, type inference).
    pub schema: Option<&'a SchemaMap>,
    /// User configuration.
    pub config: &'a RewriteConfig,
    /// Source file name for provenance.
    pub source_file: Option<&'a str>,
}
```

**Step 3: Create core/src/registry.rs**

```rust
use crate::context::RewriteContext;
use crate::types::{RewriteAction, RuleCategory, SafetyLevel};
use ogsql_parser::ast::Statement;
use std::fmt::Debug;

/// Trait that every rewrite rule must implement.
///
/// Engine dispatches via trait object — supports runtime registration.
pub trait RewriteRule: Debug + Send + Sync {
    /// Unique rule identifier, e.g., "eliminate-select-star".
    fn id(&self) -> &'static str;

    /// Human-readable description.
    fn description(&self) -> &'static str;

    /// Category for UI grouping and permission control.
    fn category(&self) -> RuleCategory;

    /// Whether this rule is enabled by default.
    fn default_enabled(&self) -> bool {
        true
    }

    /// Safety level: determines how the engine handles matched results.
    fn safety_level(&self) -> SafetyLevel;

    /// Check whether this rule applies to the given statement.
    fn matches(&self, ctx: &RewriteContext, stmt: &Statement) -> bool;

    /// Execute the rewrite, returning an action if the rule matched.
    fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Option<RewriteAction>;
}

/// Registry holding all available rules from multiple sources.
#[derive(Debug, Default)]
pub struct RuleRegistry {
    builtin: Vec<Box<dyn RewriteRule>>,
    // config: Vec<Box<dyn RewriteRule>>,  // TOML-loaded rules (future)
    // plugins: Vec<Box<dyn RewriteRule>>, // WASM/dylib (future)
}

impl RuleRegistry {
    /// Create a new registry with the given built-in rules.
    pub fn new(rules: Vec<Box<dyn RewriteRule>>) -> Self {
        Self { builtin: rules }
    }

    /// Return all registered rules.
    pub fn all_rules(&self) -> &[Box<dyn RewriteRule>] {
        &self.builtin
    }

    /// Return rules filtered by version compatibility and config.
    pub fn filtered_rules<'a>(
        &'a self,
        ctx: &RewriteContext,
    ) -> Vec<&'a dyn RewriteRule> {
        self.builtin
            .iter()
            .filter(|r| {
                let enabled = ctx.config.enabled_rules.is_empty()
                    || ctx.config.enabled_rules.contains(r.id());
                let not_disabled = !ctx.config.disabled_rules.contains(r.id());
                enabled && not_disabled && r.default_enabled()
            })
            .map(|r| r.as_ref())
            .collect()
    }
}
```

**Step 4: Create core/src/engine.rs**

```rust
use crate::context::RewriteContext;
use crate::registry::RuleRegistry;
use crate::types::{RewriteAction, RewriteResult, SafetyLevel, Suggestion};
use ogsql_parser::ast::Statement;
use ogsql_parser::formatter::SqlFormatter;
use tracing::debug;

/// The rewrite engine: orchestrates rule matching, application, and loop
/// prevention for a set of SQL statements.
#[derive(Debug)]
pub struct RewriteEngine {
    registry: RuleRegistry,
}

impl RewriteEngine {
    /// Create a new engine with the given rule registry.
    pub fn new(registry: RuleRegistry) -> Self {
        Self { registry }
    }

    /// Rewrite a list of statements by applying all matching rules.
    ///
    /// For each statement, the engine iterates up to `max_iterations` times,
    /// applying Safe/Conditional rules first, then collecting Manual suggestions.
    /// After each replacement, matching restarts from the top (priority order).
    pub fn rewrite(
        &self,
        ctx: &RewriteContext,
        stmts: Vec<Statement>,
    ) -> RewriteResult {
        let mut result = Vec::with_capacity(stmts.len());
        let mut all_suggestions = Vec::new();
        let mut any_changed = false;

        for stmt in stmts {
            let (rewritten, suggestions, changed) = self.rewrite_one(ctx, stmt);
            result.push(rewritten);
            all_suggestions.extend(suggestions);
            if changed {
                any_changed = true;
            }
        }

        RewriteResult {
            statements: result,
            suggestions: all_suggestions,
            changed: any_changed,
        }
    }

    /// Rewrite a single statement with loop prevention.
    fn rewrite_one(
        &self,
        ctx: &RewriteContext,
        mut stmt: Statement,
    ) -> (Statement, Vec<Suggestion>, bool) {
        let rules = self.registry.filtered_rules(ctx);
        let mut suggestions = Vec::new();
        let mut iteration = 0;
        let mut changed = false;

        // Separate Safe/Conditional from Manual rules
        let (auto_rules, manual_rules): (Vec<_>, Vec<_>) = rules
            .into_iter()
            .partition(|r| matches!(r.safety_level(), SafetyLevel::Safe | SafetyLevel::Conditional));

        loop {
            let mut iteration_changed = false;
            iteration += 1;

            // Apply auto-execute rules (Safe/Conditional)
            for rule in &auto_rules {
                if rule.matches(ctx, &stmt) {
                    if let Some(action) = rule.apply(ctx, &stmt) {
                        match action {
                            RewriteAction::Replace(new_stmt) => {
                                if validate_statement(&new_stmt) {
                                    stmt = *new_stmt;
                                    iteration_changed = true;
                                    changed = true;
                                    debug!(
                                        rule_id = rule.id(),
                                        iteration = iteration,
                                        "Safe rewrite applied"
                                    );
                                    break; // Re-match from top
                                }
                            }
                            _ => {
                                // Generate/Suggest from auto rules collected separately
                            }
                        }
                    }
                }
            }

            if !iteration_changed {
                break;
            }
            if iteration >= ctx.config.max_iterations {
                debug!(
                    max_iterations = ctx.config.max_iterations,
                    "Rewrite loop: max iterations reached"
                );
                break;
            }
        }

        // Collect Manual-level suggestions (never auto-execute)
        for rule in &manual_rules {
            if rule.matches(ctx, &stmt) {
                if let Some(action) = rule.apply(ctx, &stmt) {
                    suggestions.push(Suggestion {
                        rule_id: rule.id().to_string(),
                        rule_description: rule.description().to_string(),
                        action,
                        confidence: crate::types::Confidence::High,
                        notes: Vec::new(),
                    });
                }
            }
        }

        (stmt, suggestions, changed)
    }
}

/// Validate that a rewritten statement can be formatted and re-parsed.
fn validate_statement(stmt: &Statement) -> bool {
    let sql = SqlFormatter::new().format_statement(stmt);
    let (parsed, errors) = ogsql_parser::Parser::parse_sql(&sql);
    !parsed.is_empty() && errors.iter().all(|e| {
        use ogsql_parser::parser::ParserError;
        matches!(e, ParserError::ParserError { .. } | ParserError::TokenizerError(_))
    })
}
```

**Step 5: Verify core compiles**

Run: `cargo check -p metamorphosis-core`
Expected: builds without errors

---

### Task 3: EliminateSelectStar Rule (Safe)

**Files:**
- Create: `rules/src/eliminate_select_star.rs`
- Modify: `rules/src/lib.rs` (register the rule)
- Create: `rules/tests/eliminate_select_star_test.rs` (unit tests)

**Step 1: Implement the rule**

Create `rules/src/eliminate_select_star.rs`:

```rust
use metamorphosis_core::context::RewriteContext;
use metamorphosis_core::registry::RewriteRule;
use metamorphosis_core::types::{RewriteAction, RuleCategory, SafetyLevel};
use ogsql_parser::ast::visitor::VisitorResult;
use ogsql_parser::ast::{
    Expr, ObjectName, SelectStatement, SelectTarget, Spanned, Statement, TableRef,
};
use std::fmt;
use tracing::debug;

/// Rule: replace `SELECT *` (or `SELECT t.*`) with explicit column names
/// from the schema map.
///
/// Safety: Safe (semantically equivalent when schema is accurate).
#[derive(Debug)]
pub struct EliminateSelectStar;

impl RewriteRule for EliminateSelectStar {
    fn id(&self) -> &'static str {
        "eliminate-select-star"
    }

    fn description(&self) -> &'static str {
        "Replace SELECT * with explicit column names using schema metadata"
    }

    fn category(&self) -> RuleCategory {
        RuleCategory::Semantic
    }

    fn safety_level(&self) -> SafetyLevel {
        SafetyLevel::Safe
    }

    fn matches(&self, ctx: &RewriteContext, stmt: &Statement) -> bool {
        // Requires schema to expand wildcards
        if ctx.schema.is_none() {
            return false;
        }

        match stmt {
            Statement::Select(spanned) => {
                has_wildcard_target(&spanned.node.targets)
            }
            _ => false,
        }
    }

    fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Option<RewriteAction> {
        let schema = ctx.schema?;
        let spanned = match stmt {
            Statement::Select(s) => s,
            _ => return None,
        };

        let select = &spanned.node;
        if !has_wildcard_target(&select.targets) {
            return None;
        }

        // Resolve the base table to look up columns in schema
        let (table_name, _alias) = resolve_base_table(&select.from)?;
        let table_key = table_name.0.join(".").to_lowercase();
        let columns = schema.get(&table_key)?;

        debug!(
            table = %table_key,
            column_count = columns.len(),
            "Expanding SELECT *"
        );

        // Build new target list replacing wildcards with explicit column refs
        let mut new_targets: Vec<SelectTarget> = Vec::with_capacity(select.targets.len());
        for target in &select.targets {
            match target {
                SelectTarget::Star(prefix) => {
                    // If prefix is Some("t"), only expand columns for table "t"
                    // If prefix is None, expand all columns from base table
                    for col_name in columns.keys() {
                        let column_ref = if let Some(_p) = prefix {
                            // Qualified: prefix.col_name
                            let mut parts = vec![_p.clone()];
                            parts.push(col_name.clone());
                            Expr::ColumnRef(ObjectName(parts))
                        } else {
                            Expr::ColumnRef(ObjectName(vec![col_name.clone()]))
                        };
                        new_targets.push(SelectTarget::Expr(column_ref, None));
                    }
                }
                other => new_targets.push(other.clone()),
            }
        }

        let mut new_select = select.clone();
        new_select.targets = new_targets;

        Some(RewriteAction::Replace(Box::new(Statement::Select(
            Spanned::without_span(new_select),
        ))))
    }
}

/// Check if any target is a wildcard (including qualified wildcards like `t.*`).
fn has_wildcard_target(targets: &[SelectTarget]) -> bool {
    targets.iter().any(|t| matches!(t, SelectTarget::Star(_)))
}

/// Resolve the first base table from the FROM clause, skipping subqueries/joins.
fn resolve_base_table(from: &[TableRef]) -> Option<(&ObjectName, &Option<String>)> {
    from.iter().find_map(|tr| match tr {
        TableRef::Table {
            name,
            alias,
            ..
        } => Some((name, alias)),
        _ => None,
    })
}
```

**Step 2: Register in rules/src/lib.rs**

Edit `rules/src/lib.rs` — the `pub mod eliminate_select_star;` is already there, and the `builtin_rules()` function already includes it.

**Step 3: Create rules/tests/eliminate_select_star_test.rs**

```rust
#[cfg(test)]
mod tests {
    use metamorphosis_core::context::{RewriteConfig, RewriteContext};
    use metamorphosis_core::engine::RewriteEngine;
    use metamorphosis_core::registry::RuleRegistry;
    use metamorphosis_core::types::RewriteResult;
    use metamorphosis_rules::eliminate_select_star::EliminateSelectStar;
    use ogsql_parser::analyzer::SchemaMap;
    use ogsql_parser::ast::{Statement, Spanned};
    use ogsql_parser::formatter::SqlFormatter;
    use ogsql_parser::Parser;
    use std::collections::HashMap;

    fn make_schema() -> SchemaMap {
        let mut cols = HashMap::new();
        cols.insert("id".to_string(), "integer".to_string());
        cols.insert("name".to_string(), "varchar".to_string());
        cols.insert("email".to_string(), "varchar".to_string());
        let mut schema = SchemaMap::new();
        schema.insert("users".to_string(), cols);
        schema
    }

    fn test_rewrite(sql: &str, schema: &SchemaMap) -> RewriteResult {
        let engine = RewriteEngine::new(RuleRegistry::new(vec![
            Box::new(EliminateSelectStar),
        ]));
        let config = RewriteConfig::default();
        let ctx = RewriteContext {
            version: None,
            schema: Some(schema),
            config: &config,
            source_file: None,
        };

        let (_stmts, _errors) = Parser::parse_sql(sql);
        let statements: Vec<Statement> = _stmts.into_iter()
            .map(|si| si.statement)
            .collect();

        engine.rewrite(&ctx, statements)
    }

    #[test]
    fn test_expand_select_star() {
        let result = test_rewrite("SELECT * FROM users", &make_schema());
        assert!(result.changed);
        assert_eq!(result.statements.len(), 1);

        let sql = SqlFormatter::new().format_statement(&result.statements[0]);
        assert!(sql.contains("id"));
        assert!(sql.contains("name"));
        assert!(sql.contains("email"));
        assert!(!sql.contains('*'));
    }

    #[test]
    fn test_no_star_no_change() {
        let result = test_rewrite("SELECT id, name FROM users", &make_schema());
        assert!(!result.changed);
    }

    #[test]
    fn test_no_schema_no_match() {
        let engine = RewriteEngine::new(RuleRegistry::new(vec![
            Box::new(EliminateSelectStar),
        ]));
        let config = RewriteConfig::default();
        let ctx = RewriteContext {
            version: None,
            schema: None,  // No schema — rule should skip
            config: &config,
            source_file: None,
        };

        let (_stmts, _errors) = Parser::parse_sql("SELECT * FROM users");
        let statements: Vec<Statement> = _stmts.into_iter()
            .map(|si| si.statement)
            .collect();

        let result = engine.rewrite(&ctx, statements);
        assert!(!result.changed, "Without schema, SELECT * should not expand");
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p metamorphosis-rules --test eliminate_select_star_test`
Expected: all 3 tests pass

---

### Task 4: DetectDuplicateEqKeys Rule (Manual)

**Files:**
- Create: `rules/src/detect_duplicate_eq_keys.rs`
- Modify: `rules/src/lib.rs`
- Create: `rules/tests/detect_duplicate_eq_keys_test.rs` (unit tests)

**Step 1: Implement the rule**

Create `rules/src/detect_duplicate_eq_keys.rs`:

```rust
use metamorphosis_core::context::RewriteContext;
use metamorphosis_core::registry::RewriteRule;
use metamorphosis_core::types::{Confidence, RewriteAction, RuleCategory, SafetyLevel};
use ogsql_parser::ast::visitor::{visit_expr, VisitorResult};
use ogsql_parser::ast::{
    BinaryOperator, Expr, Literal, ObjectName, OrderByExpr, SelectStatement, SelectTarget,
    Spanned, Statement, TableRef,
};
use std::fmt;
use std::collections::HashSet;
use tracing::debug;

/// Rule: detect duplicate candidate keys from equality conditions and generate
/// a GROUP BY probe SQL to verify uniqueness.
///
/// Manual level: only generates suggestions (probe SQL), never replaces.
#[derive(Debug)]
pub struct DetectDuplicateEqKeys;

impl RewriteRule for DetectDuplicateEqKeys {
    fn id(&self) -> &'static str {
        "detect-duplicate-eq-keys"
    }

    fn description(&self) -> &'static str {
        "Detect candidate keys from equality conditions and generate uniqueness probe"
    }

    fn category(&self) -> RuleCategory {
        RuleCategory::DataQuality
    }

    fn safety_level(&self) -> SafetyLevel {
        SafetyLevel::Manual
    }

    fn matches(&self, ctx: &RewriteContext, stmt: &Statement) -> bool {
        let select = match stmt {
            Statement::Select(s) => &s.node,
            _ => return false,
        };

        // Find base table
        let Some((base_table, _alias)) = resolve_base_table(&select.from) else {
            return false;
        };

        // Collect equality conditions
        let mut collector = EqPredicateCollector::new(base_table);
        collector.visit_statement(stmt);
        let total_eq = collector.tier1.len() + collector.tier2.len();
        total_eq >= 2
    }

    fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Option<RewriteAction> {
        let select = match stmt {
            Statement::Select(s) => s,
            _ => return None,
        };
        let (base_table, _alias) = resolve_base_table(&select.node.from)?;

        let mut collector = EqPredicateCollector::new(base_table);
        collector.visit_statement(stmt);

        let mut group_cols: Vec<String> = collector.tier1.clone();
        group_cols.extend(collector.tier2.clone());
        // Deduplicate by collecting into a set then back
        let mut seen = HashSet::new();
        group_cols.retain(|c| seen.insert(c.clone()));

        if group_cols.is_empty() {
            return None;
        }

        let limit = ctx.config.probe_default_limit;
        let probe = build_probe_statement(base_table, &group_cols, &collector.non_eq, limit);

        debug!(
            rule_id = self.id(),
            group_cols = ?group_cols,
            "Generated duplicate key probe"
        );

        Some(RewriteAction::Generate {
            stmt: Box::new(Statement::Select(probe)),
            purpose: "Candidate key duplicate detection: verify uniqueness of equality-condition columns".to_string(),
            confidence: if collector.has_subquery {
                Confidence::Medium
            } else {
                Confidence::High
            },
        })
    }
}

/// Collects equality predicates from a SELECT statement, partitioned by tier.
struct EqPredicateCollector {
    base_table: ObjectName,
    /// Tier 1: column = literal/placeholder/bound variable (high confidence)
    pub tier1: Vec<String>,
    /// Tier 2: column = same-table column (e.g., EXISTS subquery correlation)
    pub tier2: Vec<String>,
    /// Tier 3: column = scalar subquery / dynamic expression (not in GROUP BY)
    pub tier3: Vec<Expr>,
    /// Non-equality conditions (BETWEEN, LIKE, range, IN, etc.)
    pub non_eq: Vec<Expr>,
    /// Whether any subquery was encountered (affects confidence)
    pub has_subquery: bool,
}

impl EqPredicateCollector {
    fn new(base_table: &ObjectName) -> Self {
        Self {
            base_table: base_table.clone(),
            tier1: Vec::new(),
            tier2: Vec::new(),
            tier3: Vec::new(),
            non_eq: Vec::new(),
            has_subquery: false,
        }
    }

    fn visit_statement(&mut self, stmt: &Statement) {
        self.visit_stmt_inner(stmt);
    }

    fn visit_stmt_inner(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Select(spanned) => {
                self.visit_select(&spanned.node);
            }
            _ => {}
        }
    }

    fn visit_select(&mut self, select: &SelectStatement) {
        if let Some(ref where_clause) = select.where_clause {
            self.visit_where(where_clause);
        }
    }

    fn visit_where(&mut self, expr: &Expr) {
        match expr {
            Expr::BinaryOp {
                left,
                op: BinaryOperator::Eq,
                right,
            } => {
                self.handle_equality(left, right);
            }
            Expr::BinaryOp { left, op, right } => {
                // Non-equality operator: recurse into both sides
                self.visit_expr_inner(left);
                self.visit_expr_inner(right);
                self.non_eq.push(expr.clone());
            }
            Expr::And(left, right) => {
                self.visit_where(left);
                self.visit_where(right);
            }
            Expr::Or(left, right) => {
                self.visit_where(left);
                self.visit_where(right);
            }
            _ => {
                self.visit_expr_inner(expr);
                self.non_eq.push(expr.clone());
            }
        }
    }

    fn handle_equality(&mut self, left: &Expr, right: &Expr) {
        // Determine which side is the base table column
        let (col_expr, val_expr) = if self.is_base_column(left) {
            (left, right)
        } else if self.is_base_column(right) {
            (right, left)
        } else {
            // Neither side is a base table column — skip
            self.visit_expr_inner(left);
            self.visit_expr_inner(right);
            return;
        };

        // Extract column name
        let col_name = self.extract_column_name(col_expr);

        // Classify the value expression
        if self.is_literal_or_placeholder(val_expr) {
            // Tier 1: column = literal / placeholder / variable
            if let Some(name) = col_name {
                self.tier1.push(name);
            }
        } else if self.is_base_column(val_expr) {
            // Tier 2: column = same-table column (could be correlation)
            if let Some(name) = col_name {
                self.tier2.push(name);
            }
        } else if self.is_subquery(val_expr) {
            // Tier 3: column = scalar subquery
            self.has_subquery = true;
            if let Some(name) = col_name {
                self.tier3.push(Expr::BinaryOp {
                    left: Box::new(col_expr.clone()),
                    op: BinaryOperator::Eq,
                    right: Box::new(val_expr.clone()),
                });
            }
        } else {
            // Function call or complex expression — Tier 3
            self.has_subquery = true;
            if let Some(name) = col_name {
                self.tier3.push(Expr::BinaryOp {
                    left: Box::new(col_expr.clone()),
                    op: BinaryOperator::Eq,
                    right: Box::new(val_expr.clone()),
                });
            }
        }
    }

    fn is_base_column(&self, expr: &Expr) -> bool {
        match expr {
            Expr::ColumnRef(name) => {
                // Match if unqualified, or if first part matches base table name/alias
                name.0.len() == 1 || name.0.len() == 2
            }
            _ => false,
        }
    }

    fn extract_column_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::ColumnRef(name) => {
                // Return last segment (the actual column name)
                name.0.last().cloned()
            }
            _ => None,
        }
    }

    fn is_literal_or_placeholder(&self, expr: &Expr) -> bool {
        matches!(
            expr,
            Expr::Literal(_) | Expr::Placeholder(_)
        )
    }

    fn is_subquery(&self, expr: &Expr) -> bool {
        matches!(
            expr,
            Expr::Subquery(_) | Expr::Exists(_)
        )
    }

    fn visit_expr_inner(&mut self, expr: &Expr) {
        // Minimal expr visitor — just check for subqueries
        if matches!(expr, Expr::Subquery(_) | Expr::Exists(_)) {
            self.has_subquery = true;
        }
    }
}

/// Build probe SQL: SELECT col1, col2, ..., count(1) AS cnt
/// FROM table WHERE ... GROUP BY col1, col2, ... HAVING count(1) > 1 ORDER BY cnt DESC LIMIT N
fn build_probe_statement(
    table: &ObjectName,
    group_cols: &[String],
    non_eq: &[Expr],
    limit: usize,
) -> Spanned<SelectStatement> {
    use ogsql_parser::ast::OrderByExpr;

    // Build target list: key columns + count(1) AS cnt
    let mut targets: Vec<SelectTarget> = group_cols
        .iter()
        .map(|col| {
            SelectTarget::Expr(
                Expr::ColumnRef(ObjectName(vec![col.clone()])),
                None,
            )
        })
        .collect();
    targets.push(SelectTarget::Expr(
        Expr::FunctionCall {
            name: ObjectName(vec!["count".to_string()]),
            args: vec![Expr::Literal(Literal::Integer(1))],
            over: None,
            distinct: false,
            filter: None,
            within_group: false,
        },
        Some("cnt".to_string()),
    ));

    // WHERE = merge non_eq conditions with AND
    let where_clause = merge_conditions(non_eq);

    // GROUP BY all candidate key columns
    let group_by: Vec<_> = group_cols
        .iter()
        .map(|col| ogsql_parser::ast::GroupByItem::Expr(Expr::ColumnRef(ObjectName(vec![col.clone()]))))
        .collect();

    // HAVING count(1) > 1
    let having = Some(Expr::BinaryOp {
        left: Box::new(Expr::FunctionCall {
            name: ObjectName(vec!["count".to_string()]),
            args: vec![Expr::Literal(Literal::Integer(1))],
            over: None,
            distinct: false,
            filter: None,
            within_group: false,
        }),
        op: BinaryOperator::Gt,
        right: Box::new(Expr::Literal(Literal::Integer(1))),
    });

    // ORDER BY cnt DESC
    let order_by = vec![OrderByExpr {
        expr: Expr::ColumnRef(ObjectName(vec!["cnt".to_string()])),
        asc: Some(false),
    }];

    // LIMIT N
    let limit_expr = Some(Expr::Literal(Literal::Integer(limit as i64)));

    Spanned::without_span(SelectStatement {
        targets,
        from: vec![TableRef::Table {
            name: table.clone(),
            alias: None,
            column_aliases: vec![],
            partition: None,
            timecapsule: None,
            tablesample: None,
        }],
        where_clause,
        group_by,
        having,
        order_by,
        limit: limit_expr,
        ..Default::default()
    })
}

/// Merge conditions with AND. Returns None if empty.
fn merge_conditions(conditions: &[Expr]) -> Option<Expr> {
    match conditions.len() {
        0 => None,
        1 => Some(conditions[0].clone()),
        _ => {
            let mut iter = conditions.iter();
            let first = iter.next().unwrap().clone();
            Some(iter.fold(first, |acc, expr| {
                Expr::And(Box::new(acc), Box::new(expr.clone()))
            }))
        }
    }
}

fn resolve_base_table(from: &[TableRef]) -> Option<(&ObjectName, &Option<String>)> {
    from.iter().find_map(|tr| match tr {
        TableRef::Table { name, alias, .. } => Some((name, alias)),
        _ => None,
    })
}
```

**Step 2: Register in rules/src/lib.rs**

The `pub mod detect_duplicate_eq_keys;` is already declared. Verify `builtin_rules()` includes it.

**Step 3: Create rules/tests/detect_duplicate_eq_keys_test.rs**

```rust
#[cfg(test)]
mod tests {
    use metamorphosis_core::context::{RewriteConfig, RewriteContext};
    use metamorphosis_core::engine::RewriteEngine;
    use metamorphosis_core::registry::RuleRegistry;
    use metamorphosis_core::types::RewriteResult;
    use metamorphosis_rules::detect_duplicate_eq_keys::DetectDuplicateEqKeys;
    use ogsql_parser::ast::Statement;
    use ogsql_parser::formatter::SqlFormatter;
    use ogsql_parser::Parser;

    fn test_suggest(sql: &str) -> RewriteResult {
        let engine = RewriteEngine::new(RuleRegistry::new(vec![
            Box::new(DetectDuplicateEqKeys),
        ]));
        let config = RewriteConfig::default();
        let ctx = RewriteContext {
            version: None,
            schema: None,
            config: &config,
            source_file: None,
        };

        let (_stmts, _errors) = Parser::parse_sql(sql);
        let statements: Vec<Statement> = _stmts.into_iter()
            .map(|si| si.statement)
            .collect();

        engine.rewrite(&ctx, statements)
    }

    #[test]
    fn test_generate_probe_for_two_eq_keys() {
        let result = test_suggest(
            "SELECT * FROM orders WHERE account_id = 100 AND status = 'ACTIVE'"
        );
        assert!(result.changed, "Rule should detect two eq conditions");
        assert_eq!(result.suggestions.len(), 1, "Should produce one suggestion");

        let suggestion = &result.suggestions[0];
        assert_eq!(suggestion.rule_id, "detect-duplicate-eq-keys");
    }

    #[test]
    fn test_probe_sql_contains_group_by() {
        let result = test_suggest(
            "SELECT * FROM users WHERE tenant_id = :tid AND user_id = ?"
        );

        let suggestion = &result.suggestions[0];
        if let metamorphosis_core::types::RewriteAction::Generate { ref stmt, .. } = suggestion.action {
            let sql = SqlFormatter::new().format_statement(stmt);
            assert!(sql.to_uppercase().contains("GROUP BY"), "Probe SQL must have GROUP BY: {}", sql);
            assert!(sql.to_uppercase().contains("HAVING"), "Probe SQL must have HAVING: {}", sql);
            assert!(sql.contains("tenant_id"), "Probe must reference tenant_id");
            assert!(sql.contains("user_id"), "Probe must reference user_id");
        } else {
            panic!("Expected Generate action");
        }
    }

    #[test]
    fn test_single_eq_no_match() {
        let result = test_suggest("SELECT * FROM users WHERE id = 1");
        assert!(!result.changed, "Single eq condition not a candidate key");
        assert_eq!(result.suggestions.len(), 0);
    }

    #[test]
    fn test_no_eq_no_match() {
        let result = test_suggest("SELECT * FROM users");
        assert!(!result.changed);
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p metamorphosis-rules`
Expected: all 7 tests pass (3 from EliminateSelectStar + 4 from DetectDuplicateEqKeys)

---

### Task 5: CLI (rewrite/suggest commands)

**Files:**
- Modify: `cli/src/main.rs`
- Test: manual CLI invocation tests

**Step 1: Implement CLI with clap**

Edit `cli/src/main.rs`:

```rust
//! Metamorphosis CLI — SQL rewriting and suggestion engine.
//!
//! ```bash
//! # Rewrite SQL using Safe rules
//! metamorphosis rewrite query.sql --version 5.0 --schema schema.json
//!
//! # Generate suggestions (Manual rules)
//! metamorphosis suggest query.sql --version 5.0 -o json
//! ```

use clap::{Parser as ClapParser, Subcommand};
use metamorphosis_core::context::{RewriteConfig, RewriteContext};
use metamorphosis_core::engine::RewriteEngine;
use metamorphosis_core::registry::RuleRegistry;
use metamorphosis_core::types::SafetyLevel;
use ogsql_parser::analyzer::SchemaMap;
use ogsql_parser::formatter::SqlFormatter;
use ogsql_parser::{ParseOptions, Parser};
use std::path::PathBuf;
use tracing::info;

#[derive(ClapParser)]
#[command(name = "metamorphosis", version, about = "SQL semantic rewriting & data quality probe engine")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Rewrite SQL using Safe (and optionally Conditional) rules
    Rewrite {
        /// Path to SQL file
        file: PathBuf,
        /// Database version (e.g., 5.0)
        #[arg(long)]
        version: Option<String>,
        /// Path to JSON schema file
        #[arg(long)]
        schema: Option<PathBuf>,
        /// Comma-separated rule IDs to use
        #[arg(long)]
        rules: Option<String>,
    },
    /// Generate suggestions using Manual rules (never rewrites)
    Suggest {
        /// Path to SQL file
        file: PathBuf,
        /// Database version (e.g., 5.0)
        #[arg(long)]
        version: Option<String>,
        /// Path to JSON schema file
        #[arg(long)]
        schema: Option<PathBuf>,
        /// Output format: text (default) or json
        #[arg(short = 'o', default_value = "text")]
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
            rules,
        } => run_rewrite(file, version.as_deref(), schema, rules),
        Command::Suggest {
            file,
            version,
            schema,
            output,
        } => run_suggest(file, version.as_deref(), schema, &output),
    }
}

fn load_sql(file: &PathBuf) -> String {
    std::fs::read_to_string(file)
        .unwrap_or_else(|e| {
            eprintln!("Error: cannot read '{}': {}", file.display(), e);
            std::process::exit(1);
        })
}

fn load_schema(path: Option<PathBuf>) -> Option<SchemaMap> {
    let p = path?;
    let content = std::fs::read_to_string(&p)
        .unwrap_or_else(|e| {
            eprintln!("Error: cannot read schema '{}': {}", p.display(), e);
            std::process::exit(1);
        });
    serde_json::from_str(&content)
        .unwrap_or_else(|e| {
            eprintln!("Error: invalid schema JSON '{}': {}", p.display(), e);
            std::process::exit(1);
        })
}

fn build_engine(rules_opt: Option<String>) -> RewriteEngine {
    let all_rules = metamorphosis_rules::builtin_rules();

    let registry = if let Some(rules_str) = rules_opt {
        let enabled: std::collections::HashSet<String> = rules_str
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        let filtered: Vec<Box<dyn metamorphosis_core::registry::RewriteRule>> = all_rules
            .into_iter()
            .filter(|r| enabled.contains(r.id()))
            .collect();
        RuleRegistry::new(filtered)
    } else {
        RuleRegistry::new(all_rules)
    };

    RewriteEngine::new(registry)
}

fn run_rewrite(file: PathBuf, version: Option<&str>, schema_path: Option<PathBuf>, rules: Option<String>) {
    let sql = load_sql(&file);
    let schema = load_schema(schema_path);
    let engine = build_engine(rules);

    let config = RewriteConfig::default();
    let ctx = RewriteContext {
        version,
        schema: schema.as_ref(),
        config: &config,
        source_file: Some(file.to_str().unwrap_or("unknown")),
    };

    let parse_output = Parser::parse_sql_with_options(&sql, ParseOptions {
        preserve_comments: false,
        mybatis_params: false,
    });

    if !parse_output.errors.is_empty() {
        for err in &parse_output.errors {
            eprintln!("Parse error: {:?}", err);
        }
    }

    let stmts: Vec<_> = parse_output.statements.into_iter()
        .map(|si| si.statement)
        .collect();

    let result = engine.rewrite(&ctx, stmts);

    if result.changed {
        for stmt in &result.statements {
            println!("{};", SqlFormatter::new().format_statement(stmt));
        }
        if !result.suggestions.is_empty() {
            eprintln!("Suggestions produced (use `suggest` command to view):");
            for s in &result.suggestions {
                eprintln!("  - {}: {}", s.rule_id, s.rule_description);
            }
        }
    } else {
        println!("-- No rewrites applied");
    }
}

fn run_suggest(file: PathBuf, version: Option<&str>, schema_path: Option<PathBuf>, output: &str) {
    let sql = load_sql(&file);
    let schema = load_schema(schema_path);
    let engine = build_engine(None);

    let config = RewriteConfig::default();
    let ctx = RewriteContext {
        version,
        schema: schema.as_ref(),
        config: &config,
        source_file: Some(file.to_str().unwrap_or("unknown")),
    };

    let parse_output = Parser::parse_sql_with_options(&sql, ParseOptions::default());
    let stmts: Vec<_> = parse_output.statements.into_iter()
        .map(|si| si.statement)
        .collect();

    let result = engine.rewrite(&ctx, stmts);

    match output {
        "json" => {
            // Only output Manual-level suggestions (plus metadata)
            let suggestions_json = serde_json::to_string_pretty(&result.suggestions)
                .expect("Failed to serialize suggestions");
            println!("{}", suggestions_json);
        }
        _ => {
            if result.suggestions.is_empty() {
                println!("No suggestions.");
                return;
            }
            for s in &result.suggestions {
                println!("---");
                println!("Rule: {} — {}", s.rule_id, s.rule_description);
                if let metamorphosis_core::types::RewriteAction::Generate { ref stmt, ref purpose, ref confidence } = s.action {
                    println!("Purpose: {}", purpose);
                    println!("Confidence: {:?}", confidence);
                    println!("Probe SQL:");
                    println!("{};", SqlFormatter::new().format_statement(stmt));
                }
            }
        }
    }
}
```

**Step 2: Build the CLI**

Run: `cargo build -p metamorphosis-cli`
Expected: builds without errors

**Step 3: Manual smoke test**

Create a test SQL file and run:
```bash
echo "SELECT * FROM users WHERE id = 1 AND status = 'ACTIVE'" > /tmp/test.sql
cargo run -p metamorphosis-cli -- rewrite /tmp/test.sql --version 5.0
cargo run -p metamorphosis-cli -- suggest /tmp/test.sql
```

---

### Task 6: Integration Tests

**Files:**
- Create: `tests/integration_test.rs` (workspace-level integration test)

Since Cargo workspaces don't naturally support top-level `tests/`, create an integration test crate:

- Create: `tests/Cargo.toml`
- Create: `tests/tests/end_to_end.rs`

Alternative: Place integration tests inside `cli/tests/` since it depends on everything.

For MVP, add integration tests to `cli/`:

- Create: `cli/tests/integration_test.rs`

**Step 1: Create cli/tests/integration_test.rs**

```rust
use std::process::Command;
use std::str;

/// End-to-end: verify that the CLI binary can rewrite SQL.
#[test]
fn test_cli_rewrite_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("rewrite")
        .arg("--help")
        .output()
        .expect("Failed to run metamorphosis rewrite --help");

    assert!(output.status.success(), "rewrite --help should succeed");
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(stdout.contains("Rewrite"));
}

/// End-to-end: rewrite a simple SQL file with SELECT *.
#[test]
fn test_cli_rewrite_select_star() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let sql_path = dir.path().join("query.sql");
    let schema_path = dir.path().join("schema.json");

    let mut f = std::fs::File::create(&sql_path).unwrap();
    f.write_all(b"SELECT * FROM users WHERE id = 1").unwrap();

    let mut schema = std::collections::HashMap::new();
    let mut cols = std::collections::HashMap::new();
    cols.insert("id".to_string(), "integer".to_string());
    cols.insert("name".to_string(), "varchar".to_string());
    schema.insert("users".to_string(), cols);
    let schema_json = serde_json::to_string(&schema).unwrap();
    let mut f = std::fs::File::create(&schema_path).unwrap();
    f.write_all(schema_json.as_bytes()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_metamorphosis"))
        .arg("rewrite")
        .arg(sql_path.to_str().unwrap())
        .arg("--schema")
        .arg(schema_path.to_str().unwrap())
        .output()
        .expect("Failed to run metamorphosis rewrite");

    assert!(output.status.success());
    let stdout = str::from_utf8(&output.stdout).unwrap();
    assert!(stdout.contains("id"));
    assert!(stdout.contains("name"));
    assert!(!stdout.contains('*'));
}
```

Note: Add `tempfile` to `cli/Cargo.toml` dev-dependencies:
```toml
[dev-dependencies]
tempfile = "3"
```

**Step 2: Run integration tests**

Run: `cargo test -p metamorphosis-cli --test integration_test`
Expected: passes

---

### Task 7: Final Verification & Cleanup

**Step 1: Full workspace build**

Run: `cargo build --workspace`
Expected: clean build, no errors

**Step 2: Run all tests**

Run: `cargo test --workspace`
Expected: all tests pass

**Step 3: Check diagnostics**

Run: `cargo clippy --workspace`
Expected: no warnings (or minimal pre-existing ones)

Run: `cargo fmt --check`
Expected: all files formatted

**Step 4: Verify line counts**

Run: `wc -l core/src/*.rs rules/src/*.rs cli/src/main.rs`
Expected: no file exceeds 600 lines, `cli/src/main.rs` and `core/src/lib.rs` ≤200

---

### Summary of File Manifest

| File | Action | Lines (est.) |
|------|--------|-------------|
| `Cargo.toml` | Create | 8 |
| `core/Cargo.toml` | Create | 12 |
| `core/src/lib.rs` | Create | 15 |
| `core/src/types.rs` | Create | 120 |
| `core/src/context.rs` | Create | 45 |
| `core/src/registry.rs` | Create | 80 |
| `core/src/engine.rs` | Create | 120 |
| `rules/Cargo.toml` | Create | 10 |
| `rules/src/lib.rs` | Create | 22 |
| `rules/src/eliminate_select_star.rs` | Create | 130 |
| `rules/src/detect_duplicate_eq_keys.rs` | Create | 280 |
| `rules/tests/eliminate_select_star_test.rs` | Create | 90 |
| `rules/tests/detect_duplicate_eq_keys_test.rs` | Create | 85 |
| `cli/Cargo.toml` | Create | 16 |
| `cli/src/main.rs` | Create | 190 |
| `cli/tests/integration_test.rs` | Create | 75 |

### Execution Order

Tasks are sequential within a phase but can be parallelized:

```
Task 1 ──► Task 2 ──► Task 3 ──► Task 4 ──► Task 5 ──► Task 6 ──► Task 7
                      │                    │
                      └── parallel ────────┘
                    (Tasks 3 & 4 can be
                     implemented in parallel)
```

Tasks 3 and 4 (the two rules) have no dependency on each other — they can be implemented simultaneously by separate subagents.
