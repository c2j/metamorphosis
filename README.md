# Metamorphosis v0.1.19

[中文](#中文) | [English](#english)

---

## 中文

### 项目标题与简介

**Metamorphosis** 是一个 SQL 语义重写与数据质量探针引擎，构建于 [ogsql-parser](https://github.com/c2j/ogsql-parser) 之上。

它只消费 parser 输出的 AST，从不直接解析 SQL。引擎通过可插拔规则对 AST 进行改写或诊断，输出重写后的 SQL 或数据质量探针建议。所有输出均带有置信度（High / Medium / Low），并依据安全级别决定是自动执行、条件执行还是仅生成建议。

### 架构图

```
Layer 4: CLI / MCP Server
Layer 3: RuleRegistry → RewriteEngine → SuggestionEngine
Layer 2: Rules (4 built-in) / QED Verifier / VeriEQL Verifier
Layer 1: ogsql-parser (AST / Visitor / SchemaMap / Formatter)
```

### Workspace 结构

```
metamorphosis/
├── crates/
│   ├── core/          # 引擎与抽象层（类型、Trait、上下文、注册表）
│   ├── rules/         # 4 个内置重写规则
│   ├── cli/           # 命令行入口（5 个子命令）
│   ├── qed/           # QED 离线验证（嵌入式 Z3 SMT 求解器）
│   ├── verieql/       # 有界等价性验证（OOPSLA 2024 算法移植，零 metamorphosis 依赖）
│   └── mcp-server/    # MCP (Model Context Protocol) 服务器，5 个工具
└── docs/              # 设计文档、编码规范、实现计划
```

- `crates/core/`：引擎与抽象层，定义 `RewriteRule` trait、`RewriteEngine`、`RuleRegistry`、`RewriteContext` 等核心类型。
- `crates/rules/`：4 个内置规则，覆盖语义改写、性能优化与数据质量探针。
- `crates/cli/`：命令行二进制 `metamorphosis`，提供 5 个子命令。
- `crates/qed/`：QED 离线验证器，将 SQL 查询翻译为 SMT 公式，通过嵌入式 Z3 证明语义等价性。
- `crates/verieql/`：VeriEQL 有界等价性验证器，移植自 OOPSLA 2024 论文算法，不依赖 metamorphosis 其他 crate。
- `crates/mcp-server/`：基于 rmcp v0.16 的 MCP 服务器，使用 stdio 传输，暴露 5 个工具供 AI 助手调用。

### 快速开始

```bash
# 编译整个 workspace
cargo build --workspace

# 使用 Safe 规则重写 SQL
metamorphosis rewrite query.sql --schema schema.json

# 使用 Manual 规则生成数据质量探针建议
metamorphosis suggest query.sql --schema schema.json

# 启用指定规则
metamorphosis rewrite query.sql --schema schema.json \
  --rules eliminate-select-star,detect-duplicate-eq-keys

# JSON 输出
metamorphosis suggest query.sql --schema schema.json -o json > report.json
```

Schema JSON 格式为 `表名 → 列名 → 类型`：

```json
{"users": {"id": "integer", "name": "varchar", "email": "varchar"}}
```

### CLI 命令一览

| 命令 | 说明 |
|------|------|
| `rewrite` | 应用 Safe（以及可选 Conditional）规则重写 SQL |
| `suggest` | 应用 Manual 规则生成建议与探针 SQL，不会自动改写原 SQL |
| `show-rules` | 列出所有内置规则及其元数据 |
| `verify` | 使用 QED 或 VeriEQL 验证原始 SQL 与重写后 SQL 的语义等价性 |
| `mcp` | 启动 stdio 模式的 MCP 服务器，供 AI 助手集成 |

### 内置规则

| 规则 ID | 类别 | 安全级别 | 说明 |
|---------|------|----------|------|
| `eliminate-select-star` | Semantic | Safe | `SELECT *` → 显式列名（基于 Schema） |
| `detect-duplicate-eq-keys` | DataQuality | Manual | WHERE 等值条件 → GROUP BY 唯一性探针 |
| `subquery-to-join` | Performance | Conditional | WHERE 子查询（EXISTS/IN/NOT EXISTS/NOT IN）→ JOIN |
| `extract-candidate-values` | DataQuality | Manual | 参数化等值列 → 候选值探针 SQL |

### 安全级别

| 级别 | 行为 |
|------|------|
| **Safe** | 语义等价，引擎自动执行 |
| **Conditional** | 需要满足前置检查条件后才执行 |
| **Manual** | 仅生成建议，不会自动替换原 SQL |

### MCP 集成

运行 `metamorphosis mcp` 启动基于 stdio 的 MCP 服务器。服务器暴露 5 个工具：

1. `list_rules`：列出规则元数据
2. `rewrite_sql`：应用 Safe / Conditional 规则重写 SQL
3. `suggest_probes`：生成 Manual 级别建议与探针 SQL
4. `verify_equivalence`：调用 QED 或 VeriEQL 验证语义等价性
5. `extract_schema`：从 DDL SQL 目录抽取 Schema

### 验证引擎

Metamorphosis 提供双验证引擎，可在 `verify` 命令或 MCP `verify_equivalence` 工具中选择：

- **QED**：离线 SMT 证明器，基于嵌入式 Z3，将查询翻译为逻辑公式进行等价性证明。
- **VeriEQL**：有界模型检查器，移植自 OOPSLA 2024 论文算法，通过限定表大小快速发现反例。

### 依赖

- Rust 2021 edition，MSRV 1.75
- [ogsql-parser](https://github.com/c2j/ogsql-parser)：openGauss / GaussDB SQL parser
- Z3 SMT 求解器，通过 `vendored` feature 静态链接
- CLI 额外依赖：clap、serde、serde_json、tracing、tokio

### 构建与测试

```bash
cargo build --workspace
cargo test --workspace
```

### 相关文档

- `docs/metamorphosis_design_doc.md`：完整架构设计文档
- `docs/UserGuide.md`：用户手册
- `docs/DeveloperGuide.md`：开发者指南（MCP / API / 验证引擎集成）
- `docs/CONTRIBUTING.md`：贡献指南与编码规范
- `docs/BEST-PRATICE.md`：Rust 编码最佳实践
- `docs/QED.md`：QED 验证器理论说明

---

## English

### Overview

**Metamorphosis** is a SQL semantic rewriting and data quality probe engine built on [ogsql-parser](https://github.com/c2j/ogsql-parser). It consumes AST output from the parser, applies pluggable rewrite rules, and produces rewritten SQL or diagnostic probe suggestions. Every output carries a confidence level, and execution is governed by rule safety levels.

### Architecture

```
Layer 4: CLI / MCP Server
Layer 3: RuleRegistry → RewriteEngine → SuggestionEngine
Layer 2: Rules (4 built-in) / QED Verifier / VeriEQL Verifier
Layer 1: ogsql-parser (AST / Visitor / SchemaMap / Formatter)
```

The workspace contains six crates:

- `crates/core/` — engine abstractions and types
- `crates/rules/` — 4 built-in rewrite rules
- `crates/cli/` — CLI binary with 5 subcommands
- `crates/qed/` — offline equivalence prover with embedded Z3
- `crates/verieql/` — bounded equivalence checker ported from OOPSLA 2024
- `crates/mcp-server/` — MCP server over stdio with 5 tools

### Quick Start

```bash
cargo build --workspace

metamorphosis rewrite query.sql --schema schema.json
metamorphosis suggest query.sql --schema schema.json
metamorphosis rewrite query.sql --rules eliminate-select-star,subquery-to-join
```

Schema JSON format: `{"table": {"column": "type"}}`.

### CLI Commands

| Command | Description |
|---------|-------------|
| `rewrite` | Apply Safe / Conditional rules |
| `suggest` | Generate Manual suggestions and probes |
| `show-rules` | List built-in rules |
| `verify` | Equivalence checking with QED or VeriEQL |
| `mcp` | Start the stdio MCP server |

### Built-in Rules

| Rule ID | Category | Safety | Description |
|---------|----------|--------|-------------|
| `eliminate-select-star` | Semantic | Safe | `SELECT *` → explicit column list |
| `detect-duplicate-eq-keys` | DataQuality | Manual | Equality conditions → GROUP BY uniqueness probe |
| `subquery-to-join` | Performance | Conditional | Subqueries (EXISTS/IN/NOT EXISTS/NOT IN) → JOIN |
| `extract-candidate-values` | DataQuality | Manual | Parameterized equality columns → candidate value probe |

### Build & Test

```bash
cargo build --workspace
cargo test --workspace
```

### Documentation

- `docs/metamorphosis_design_doc.md` — architecture spec
- `docs/UserGuide.md` — user manual
- `docs/DeveloperGuide.md` — developer guide (MCP / API / verification)
- `docs/CONTRIBUTING.md` — contribution guide and coding standards
- `docs/BEST-PRATICE.md` — Rust coding best practices
- `docs/QED.md` — QED verifier theory
