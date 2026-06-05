# AGENTS.md — Metamorphosis

## What This Is

SQL semantic rewriting & data quality probe engine built on top of `ogsql-parser`. Consumes AST output (never parses SQL directly), applies pluggable rules to produce diagnostic/rewritten SQL.

**Current state**: MVP — engine skeleton + 3 built-in rules + QED offline verification (embedded Z3). Source code in `crates/`.

## Architecture (4 layers)

```
Layer 4: CLI / HTTP API / MCP Tool
Layer 3: RuleRegistry → RuleChain → RewriteEngine → SuggestionEngine
Layer 2: Individual rules (EliminateSelectStar, DetectDuplicateEqKeys, SubqueryToJoin, …)
Layer 1: ogsql-parser (AST / Visitor / SchemaMap / Formatter)
```

Workspace: `crates/core/` (engine + abstractions), `crates/rules/` (built-in rules), `crates/cli/` (CLI), `crates/qed/` (QED verification with embedded Z3).

## Key Design Constraints

- **No SQL parsing here** — all parsing delegated to `ogsql-parser`. Metamorphosis only rewrites.
- **SafetyLevel determines behavior**: `Safe` rules auto-execute, `Conditional` needs prerequisite checks, `Manual` only generates suggestions (never auto-replaces).
- **Confidence is mandatory** on every rewrite output (High / Medium / Low).
- **Loop prevention**: `max_iterations` cap in `RewriteConfig`; after each replace, re-match from top.
- **Version-aware**: rules declare `version_range` tied to `GaussVersion` from ogsql-parser.

## Coding Standards (from docs/CONTRIBUTING.md)

These are **mandatory**, not suggestions:

- **Workspace layout**: `crates/core` / `crates/rules` / `crates/cli`, no reverse deps. `core` has zero IO deps (except ogsql-parser).
- **File size**: max 600 lines per `.rs`, ideal ≤400. Entry files (`main.rs`, `lib.rs`) ≤200.
- **Formatting**: `rustfmt` enforced. No tab indentation. No bare `as` casts — use `try_from`/`into`.
- **Error handling**: Library code must use `thiserror` (not `anyhow`). No `unwrap()` in lib. `expect()` only with justification.
- **Unsafe**: Every `unsafe` block needs a `SAFETY:` comment. No bare `as` pointer casts. Use `assert!` (not `debug_assert!`) in unsafe functions.
- **Logging**: `tracing` only (not `log`). Structured JSON in production. No sensitive data in logs.
- **Public API**: All `pub` items need doc comments. `#[non_exhaustive]` on exported structs/enums.
- **Dependencies**: No wildcard versions. Commit `Cargo.lock`. Declare `rust-version` (MSRV).
- **Naming**: No `get_` prefix on getters. `as_`/`to_`/`into_` by ownership semantics. Consistent word order project-wide.

Full details: `docs/CONTRIBUTING.md` (mandatory) and `docs/BEST-PRATICE.md` (recommended).

## Testing

Test DSL planned via `#[rule_test]` macro — declarative input/expect/confidence spec.
Version matrix testing across GaussVersion variants.

Pyramid: 50% rule unit tests, 30% engine unit tests, 20% integration (end-to-end SQL→probe).

## CLI Commands (planned)

```bash
metamorphosis rewrite query.sql --version 5.0 --schema schema.json
metamorphosis suggest query.sql --version 5.0 -o json
metamorphosis rewrite query.sql --rules detect-duplicate-eq-keys,subquery-to-join
```

## Rule Extension

Three sources by priority: builtin (Rust), config (TOML), plugins (WASM/dylib — future).
Rules implement the `RewriteRule` trait: `id`, `description`, `category`, `safety_level`, `version_range`, `matches`, `apply`.

## Design Doc

Complete architecture spec: `docs/metamorphosis_design_doc.md`.
