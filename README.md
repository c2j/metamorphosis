# Metamorphosis

SQL semantic rewriting & data quality probe engine built on `ogsql-parser`.

Consumes AST output (never parses SQL directly), applies pluggable rewrite rules, and produces rewritten SQL or diagnostic suggestions.

## Architecture

```
Layer 4: CLI / HTTP API / MCP Tool
Layer 3: RuleRegistry → RuleChain → RewriteEngine → SuggestionEngine
Layer 2: Individual rules (DetectDuplicateEqKeys, EliminateSelectStar, …)
Layer 1: ogsql-parser (AST / Visitor / SchemaMap / SemanticModel / Formatter)
```

## Project Layout (Cargo Workspace)

```
metamorphosis/
├── crates/
│   ├── core/        # Engine + abstractions (types, traits, context, registry)
│   ├── rules/       # Built-in rewrite rules
│   └── cli/         # CLI entrypoint
└── docs/            # Design doc, coding standards, implementation plans
```

## Quick Start

```bash
# Rewrite SQL using Safe rules
metamorphosis rewrite query.sql --schema schema.json

# Generate suggestions (Manual rules)
metamorphosis suggest query.sql

# Enable specific rules
metamorphosis rewrite query.sql --rules eliminate-select-star,detect-duplicate-eq-keys

# JSON output
metamorphosis suggest query.sql -o json > report.json
```

The schema JSON is a map of table name → column name → type:

```json
{"users": {"id": "integer", "name": "varchar", "email": "varchar"}}
```

## Safety Levels

| Level | Behavior |
|-------|----------|
| **Safe** | Semantically equivalent — engine auto-executes |
| **Conditional** | Requires prerequisite checks before execution |
| **Manual** | Generates suggestions only, never auto-replaces |

## Project Status

**MVP** — engine skeleton + 2 built-in rules:

- `eliminate-select-star` (Safe): `SELECT *` → explicit column list via schema
- `detect-duplicate-eq-keys` (Manual): WHERE equality conditions → GROUP BY uniqueness probe

## Dependencies

- Rust 2021 edition, MSRV 1.75
- [ogsql-parser](https://github.com/c2j/ogsql-parser) — SQL parser for openGauss/GaussDB
- CLI: clap, serde, serde_json, tracing

## Build & Test

```bash
cargo build --workspace
cargo test --workspace
```

## Design

Complete architecture specification: `docs/metamorphosis_design_doc.md`
