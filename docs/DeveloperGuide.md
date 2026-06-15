# Metamorphosis 开发者指南

**版本**：v0.1.19

本指南面向希望将 Metamorphosis 作为库、MCP 服务或验证引擎集成的开发者。内容包括架构概览、核心 API、MCP 工具协议、QED / VeriEQL 验证引擎、库集成方式以及内置规则实现参考。

---

## 目录

1. [架构概览](#1-架构概览)
2. [核心 API crates/core](#2-核心-api-cratescore)
3. [MCP 服务器开发 crates/mcp-server](#3-mcp-服务器开发-cratesmcp-server)
4. [QED 验证引擎 crates/qed](#4-qed-验证引擎-cratesqed)
5. [VeriEQL 验证引擎 crates/verieql](#5-verieql-验证引擎-cratesverieql)
6. [作为库使用](#6-作为库使用)
7. [内置规则实现参考](#7-内置规则实现参考)
8. [相关文档](#8-相关文档)

---

## 1. 架构概览

Metamorphosis 采用四层架构，所有 SQL 解析工作都交给底层的 `ogsql-parser`，自身只消费 AST 并执行重写或诊断。

```
Layer 4: CLI (crates/cli) / MCP Server (crates/mcp-server)
Layer 3: RewriteEngine / RuleRegistry / SuggestionEngine (crates/core)
Layer 2: Rules (crates/rules) / QED Verifier (crates/qed) / VeriEQL (crates/verieql)
Layer 1: ogsql-parser (AST / Visitor / SchemaMap / Formatter)
```

### 1.1 各层职责

| 层级 | Crate | 职责 |
|------|-------|------|
| Layer 4 | `crates/cli` | 命令行二进制 `metamorphosis`，提供 `rewrite`、`suggest`、`verify`、`mcp` 等子命令。 |
| Layer 4 | `crates/mcp-server` | 基于 rmcp 的 MCP 服务器，通过 stdio 暴露 5 个工具。 |
| Layer 3 | `crates/core` | 引擎抽象与编排：`RewriteRule` trait、`RewriteEngine`、`RuleRegistry`、`RewriteContext`、`RewriteConfig`、Schema 提取。 |
| Layer 2 | `crates/rules` | 4 条内置重写规则的实现。 |
| Layer 2 | `crates/qed` | QED 离线验证器，将查询翻译为 SMT 公式并调用 Z3 证明语义等价性。 |
| Layer 2 | `crates/verieql` | VeriEQL 有界等价验证器，移植自 OOPSLA 2024 论文算法。 |
| Layer 1 | `ogsql-parser` | openGauss / GaussDB SQL 解析器，提供 AST、Visitor、SchemaMap、Formatter。 |

### 1.2 Crate 依赖图

```
cli ──► rules, qed, verieql, mcp-server ──► core ──► ogsql-parser
verieql ──► ogsql-parser (standalone)
```

`verieql` 是唯一不依赖 `metamorphosis-core` 的 crate，仅依赖 `ogsql-parser` 和 `z3`，可独立使用。`core` 除 `ogsql-parser` 外无 IO 依赖，适合作为库嵌入到其他系统。

### 1.3 数据流

```
原始 SQL
   │
   ▼
ogsql-parser (Tokenizer + Parser)
   │
   ▼
Vec<Statement> + SchemaMap
   │
   ▼
metamorphosis_core::RewriteEngine::rewrite(ctx, stmts)
   │
   ├──► Safe / Conditional 规则匹配 → 替换 AST
   ├──► Manual 规则匹配 → 生成 Suggestion / Probe
   │
   ▼
RewriteResult { statements, suggestions, changed, match_failures }
   │
   ▼
Formatter → 输出 SQL 文本 / JSON 报告
```

---

## 2. 核心 API crates/core

`crates/core` 定义了所有规则的接口、引擎类型、上下文和 Schema 提取函数。它是扩展 Metamorphosis 的入口。

### 2.1 RewriteRule Trait

所有规则都必须实现 `RewriteRule`。引擎通过 trait object 动态分发，支持运行时注册。

```rust
pub trait RewriteRule: Debug + Send + Sync {
    fn id(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn category(&self) -> RuleCategory;
    fn safety_level(&self) -> SafetyLevel;
    fn matches(&self, ctx: &RewriteContext, stmt: &Statement) -> MatchResult;
    fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Option<RewriteAction>;
    fn default_enabled(&self) -> bool { true }
}
```

方法说明：

| 方法 | 说明 |
|------|------|
| `id` | 规则唯一标识，全局唯一。例如 `"eliminate-select-star"`。 |
| `description` | 人类可读描述，用于 UI 展示和报告。 |
| `category` | 规则分类，用于分组和权限控制。 |
| `safety_level` | 安全级别，决定引擎是否自动执行、条件执行或仅建议。 |
| `matches` | 检查当前语句是否满足规则前提。返回 `MatchResult` 而非 `bool`，以便在匹配失败时给出诊断原因。 |
| `apply` | 执行重写，返回 `RewriteAction`。若未产生动作可返回 `None`。 |
| `default_enabled` | 规则默认是否启用，默认为 `true`。 |

### 2.2 核心类型

#### SafetyLevel

```rust
pub enum SafetyLevel {
    Safe,
    Conditional,
    Manual,
}
```

| 变体 | 行为 |
|------|------|
| `Safe` | 语义等价，引擎自动执行替换。 |
| `Conditional` | 仅在满足前置条件时语义等价，引擎执行前需验证前提。 |
| `Manual` | 非语义等价，仅生成建议或探针 SQL，绝不自动替换原语句。 |

#### Confidence

```rust
pub enum Confidence {
    High,
    Medium,
    Low,
}
```

| 变体 | 含义 |
|------|------|
| `High` | 单表、无子查询、纯常量等值，改写确定无疑。 |
| `Medium` | 穿透了派生表或移除了 EXISTS，结构变化但语义可追踪。 |
| `Low` | 涉及多表 JOIN、动态子查询，结果需人工复核。 |

#### RewriteAction

```rust
pub enum RewriteAction {
    Replace(Box<Statement>),
    Generate {
        stmt: Box<Statement>,
        purpose: String,
        confidence: Confidence,
    },
    Suggest { message: String, severity: Severity },
}
```

| 变体 | 说明 |
|------|------|
| `Replace` | 语义等价替换，可直接替换原 SQL。 |
| `Generate` | 生成衍生 SQL，例如数据质量探针，与原 SQL 并存。 |
| `Suggest` | 仅文本建议，不产出 AST。 |

#### Severity

```rust
pub enum Severity {
    Info,
    Warning,
    Critical,
}
```

#### RuleCategory

```rust
pub enum RuleCategory {
    Performance,
    DataQuality,
    Style,
    Semantic,
    Safety,
}
```

#### MatchResult

```rust
pub enum MatchResult {
    Matched,
    NotMatched { reason: String },
}
```

`matches()` 返回 `MatchResult` 的目的是在规则未命中时提供可读原因。这些原因会进入 `RewriteResult.match_failures`，方便调试和审计。

#### MatchFailure

```rust
pub struct MatchFailure {
    pub rule_id: String,
    pub reason: String,
}
```

#### RewriteResult

```rust
pub struct RewriteResult {
    pub statements: Vec<Statement>,
    pub suggestions: Vec<Suggestion>,
    pub changed: bool,
    pub match_failures: Vec<MatchFailure>,
}
```

| 字段 | 说明 |
|------|------|
| `statements` | 重写后的语句集合（Safe / Conditional 级别）。 |
| `suggestions` | Manual 级别的建议集合。 |
| `changed` | 是否发生了任何改写。 |
| `match_failures` | 规则未命中的原因列表。 |

#### Suggestion

```rust
pub struct Suggestion {
    pub rule_id: String,
    pub rule_description: String,
    pub action: RewriteAction,
    pub confidence: Confidence,
    pub notes: Vec<String>,
}
```

### 2.3 RewriteEngine

```rust
let registry = RuleRegistry::new(builtin_rules());
let engine = RewriteEngine::new(registry);
let result = engine.rewrite(&ctx, stmts);
```

`RewriteEngine::rewrite` 对输入语句列表逐条处理，核心行为如下：

1. **规则过滤**：根据 `RewriteConfig.enabled_rules`、`disabled_rules` 以及 `default_enabled` 过滤可用规则。
2. **优先级分组**：将规则按 `SafetyLevel` 分为 `Safe / Conditional` 组与 `Manual` 组。
3. **自动执行**：对 `Safe / Conditional` 规则循环匹配，每次成功替换后从头开始重新匹配，确保高优先级规则先执行。
4. **循环防止**：单条语句最多迭代 `RewriteConfig.max_iterations` 次，默认 10 次，超过则停止并记录日志。
5. **语法验证**：每次 `Replace` 后，使用 `SqlFormatter` 格式化并重新解析，确认 AST 仍然合法。
6. **建议收集**：在自动改写结束后，对最终 AST 运行 `Manual` 规则，收集建议但不修改 AST。

### 2.4 RewriteContext & RewriteConfig

#### RewriteConfig

```rust
pub struct RewriteConfig {
    pub enabled_rules: HashSet<String>,
    pub disabled_rules: HashSet<String>,
    pub max_iterations: usize,
    pub preserve_comments: bool,
    pub probe_default_limit: usize,
}
```

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `enabled_rules` | 空集合 | 显式启用的规则。空集合表示启用所有规则。 |
| `disabled_rules` | 空集合 | 显式禁用的规则。 |
| `max_iterations` | `10` | 单条语句最大重写轮数，防止循环。 |
| `preserve_comments` | `false` | 是否保留注释，依赖 `ogsql-parser` 的 Trivia 支持。 |
| `probe_default_limit` | `10` | 生成探针 SQL 时的默认 LIMIT。 |

#### RewriteContext

```rust
pub struct RewriteContext<'a> {
    pub version: Option<&'a str>,
    pub schema: Option<&'a SchemaMap>,
    pub config: &'a RewriteConfig,
    pub source_file: Option<&'a str>,
    pub known_variables: Option<&'a HashSet<String>>,
}
```

| 字段 | 说明 |
|------|------|
| `version` | 数据库版本字符串，用于版本门控规则。 |
| `schema` | 表结构信息，用于 `SELECT *` 展开和类型推断。 |
| `config` | 重写配置引用。 |
| `source_file` | 当前处理文件名，用于溯源。 |
| `known_variables` | 已知的 PL/pgSQL 变量名集合，帮助规则区分变量与表列。 |

### 2.5 Schema 提取

```rust
pub fn extract_schema_from_dir(dir: &Path) -> Result<SchemaMap, ExtractionError>
```

`extract_schema_from_dir` 扫描指定目录下所有 `.sql` 文件，解析 DDL 并构建 `SchemaMap`。支持的语句：

- `CREATE TABLE`
- `CREATE TABLE AS`（列类型标记为 `unknown`）
- `ALTER TABLE ADD COLUMN`
- `ALTER TABLE DROP COLUMN`
- `ALTER TABLE RENAME COLUMN`

解析过程中遇到警告会跳过，遇到真实错误会跳过当前文件。只有当所有文件都无法处理时才会返回错误。表名和列名会被小写化，以支持大小写不敏感查找。

`SchemaMap` 的类型为 `HashMap<String, HashMap<String, String>>`，即 `表名 → 列名 → 类型字符串`。

---

## 3. MCP 服务器开发 crates/mcp-server

`crates/mcp-server` 基于 [rmcp](https://crates.io/crates/rmcp) v0.16 实现，通过 stdio 传输与 AI 助手通信。

### 3.1 概览

| 属性 | 值 |
|------|-----|
| Transport | stdio（stdin / stdout） |
| 入口 | `metamorphosis_mcp::run_stdio()` |
| 宏 | `#[rmcp::tool_router]` / `#[rmcp::tool]` |
| Handler | `MetamorphosisServer` |

服务器是无状态的，每个请求都会新建引擎实例并执行完整流程。启动方式：

```bash
metamorphosis mcp
```

### 3.2 五个 MCP 工具详细 API

#### Tool 1: rewrite_sql

重写 SQL，应用所有匹配 Safe / Conditional 规则。

**参数**：`SqlParams`

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `sql` | `String` | 是 | 原始 SQL。 |
| `version` | `Option<String>` | 否 | 数据库版本。 |
| `schema_json` | `Option<String>` | 否 | 内联 Schema JSON。与 `schema_path`、`sql_dir` 互斥。 |
| `schema_path` | `Option<String>` | 否 | Schema JSON 文件路径。 |
| `sql_dir` | `Option<String>` | 否 | DDL SQL 目录路径。 |
| `rules` | `Option<String>` | 否 | 逗号分隔的规则 ID 列表，用于显式启用指定规则。 |

**响应**：`RewriteResponse`

```json
{
  "changed": bool,
  "rewritten_sql": [string],
  "match_failures": [
    { "rule_id": string, "reason": string }
  ],
  "warnings": [string]
}
```

**错误响应**：

```json
{ "error": string }
```

#### Tool 2: suggest_probes

生成 Manual 级别建议和数据质量探针 SQL。

**参数**：与 `rewrite_sql` 相同，使用 `SqlParams`。

**响应**：`SuggestResponse`

```json
{
  "suggestions": [
    {
      "rule_id": string,
      "rule_description": string,
      "confidence": "High" | "Medium" | "Low",
      "probe_sql": string | null,
      "message": string | null,
      "purpose": string | null
    }
  ],
  "match_failures": [
    { "rule_id": string, "reason": string }
  ],
  "warnings": [string]
}
```

字段含义：

- `probe_sql`：当 `RewriteAction::Generate` 或 `Replace` 时存在。
- `message`：当 `RewriteAction::Suggest` 时存在。
- `purpose`：当 `RewriteAction::Generate` 时存在，描述探针用途。

#### Tool 3: list_rules

列出所有可用规则及其元数据。

**参数**：无。

**响应**：

```json
{
  "rules": [
    {
      "id": string,
      "description": string,
      "category": "Performance" | "DataQuality" | "Style" | "Semantic" | "Safety",
      "safety_level": "Safe" | "Conditional" | "Manual",
      "default_enabled": bool
    }
  ]
}
```

#### Tool 4: verify_equivalence

验证原始 SQL 与重写后 SQL 是否语义等价。

**参数**：`VerifyParams`

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `original_sql` | `String` | 是 | 原始 SQL。 |
| `rewritten_sql` | `String` | 是 | 重写后的 SQL。 |
| `engine` | `Option<String>` | 否 | `"qed"`（默认）或 `"verieql"。 |
| `bound` | `Option<usize>` | 否 | VeriEQL 的每表元组上限，默认 2。 |
| `schema_json` | `Option<String>` | 否 | 内联 Schema JSON。 |
| `schema_path` | `Option<String>` | 否 | Schema JSON 文件路径。 |
| `sql_dir` | `Option<String>` | 否 | DDL SQL 目录路径。 |

`schema_json`、`schema_path`、`sql_dir` 三者互斥，且 QED / VeriEQL 都需要 Schema。

**响应**：`VerifyResponse`

```json
{
  "result": "Equivalent" | "NotEquivalent" | "Unknown",
  "engine": string,
  "original_sql": string,
  "rewritten_sql": string,
  "elapsed_ms": number | null,
  "bound": number | null,
  "counterexample": string | null,
  "column_details": object | null
}
```

- QED 验证失败时，`result` 可能被映射为 `"Unknown"`，`counterexample` 包含原因。
- VeriEQL 的 `NotEquivalent` 结果中，`column_details` 可能包含结构化反例。

#### Tool 5: extract_schema

从 DDL SQL 目录提取 Schema。

**参数**：`ExtractSchemaParams`

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `sql_dir` | `String` | 是 | DDL SQL 目录路径。 |

**响应**：

```json
{
  "table_count": number,
  "schema": { "table": { "column": "type" } }
}
```

### 3.3 MCP 客户端配置

以 Claude Desktop 为例，配置文件中添加：

```json
{
  "mcpServers": {
    "metamorphosis": {
      "command": "/path/to/metamorphosis",
      "args": ["mcp"]
    }
  }
}
```

---

## 4. QED 验证引擎 crates/qed

### 4.1 架构

QED 验证器由五个阶段组成：

```
Schema Extraction → IR → Translator → Prover (Z3/binary) → Verify Pipeline
```

| 阶段 | 模块 | 说明 |
|------|------|------|
| Schema Extraction | `qed::schema` | 从 DDL 提取主键、外键、NOT NULL、UNIQUE、CHECK 约束，生成 `RichSchema`。 |
| IR | `qed::ir` | QED 中间表示类型，Rust 结构体用于构造 QED 输入。 |
| Translator | `qed::translator` | 将 `ogsql-parser` AST 翻译为 QED Relation。 |
| Prover | `qed::prover` / `qed::z3_solver` | 优先调用嵌入式 Z3，失败时回退到外部 `qed-prover` 二进制。 |
| Verify Pipeline | `qed::verify` | 端到端封装：翻译、输出列归一化、调用证明器、返回结果。 |

### 4.2 核心 API

```rust
pub fn verify_rewrite(
    rule_id: &str,
    original: &Statement,
    rewritten: &Statement,
    schema: &RichSchema,
    prover_config: &ProverConfig,
) -> Result<VerificationResult, VerifyError>

pub fn verify_batch(
    rule_id: &str,
    test_pairs: &[(Statement, Statement)],
    schema: &RichSchema,
    prover_config: &ProverConfig,
) -> Vec<Result<VerificationResult, VerifyError>>
```

`verify_rewrite` 的工作流程：

1. 使用 `AstTranslator` 将两条 SQL 翻译为 QED Relation。
2. 构建 `QedSchema` 列表，包含表字段、主键、可空性、CHECK 约束。
3. 若两条查询输出列集合相同但顺序不同，自动在 AST 外层包装 `Project` 进行列序归一化。
4. 调用证明器。
5. 返回 `VerificationResult`，包含证明结果、耗时、输出列名等信息。

### 4.3 ProofResult

```rust
pub enum ProofResult {
    Equivalent,
    NotEquivalent { counterexample: Option<String> },
    Unknown { reason: String },
    Timeout { seconds: u64 },
}
```

| 变体 | 含义 |
|------|------|
| `Equivalent` | 已证明语义等价。 |
| `NotEquivalent` | 已证明不等价，可能附带反例文本。 |
| `Unknown` | 求解器无法判定，附带原因。 |
| `Timeout` | 证明过程超时。 |

### 4.4 RichSchema

```rust
pub struct RichSchema {
    pub tables: HashMap<String, TableInfo>,
}

pub struct TableInfo {
    pub columns: Vec<ColumnInfo>,
    pub constraints: TableConstraints,
}

pub struct TableConstraints {
    pub primary_key: Vec<String>,
    pub unique: Vec<Vec<String>>,
    pub not_null: Vec<String>,
    pub check: Vec<CheckConstraint>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
}
```

`RichSchema` 从 `CREATE TABLE` 语句提取以下约束：

- 主键（列级和表级 `PRIMARY KEY`）
- 唯一约束（`UNIQUE`）
- 非空约束（`NOT NULL`）
- CHECK 约束（`CHECK (expr)`）
- 外键约束（`REFERENCES`）

### 4.5 嵌入式 Z3

QED 通过 `z3` crate 的 `vendored` feature 静态链接 Z3，运行时无需外部库或二进制。证明流程优先使用 `qed::z3_solver` 中的嵌入式求解器；当嵌入式求解器失败时，才会回退到配置中的 `qed-prover` 外部二进制。

```rust
pub struct ProverConfig {
    pub binary_path: PathBuf,
    pub timeout_secs: u64,
    pub workdir: Option<PathBuf>,
}
```

默认配置：

- `binary_path`：`"qed-prover"`
- `timeout_secs`：`60`

---

## 5. VeriEQL 验证引擎 crates/verieql

### 5.1 特点

VeriEQL 是一个完全独立的有界等价验证器：

- 零 `metamorphosis` 依赖，仅依赖 `ogsql-parser` 和 `z3`。
- 有界模型检查：每表配置 `B` 个符号元组。
- 支持 Bag（默认）和 List 两种语义。
- 使用三值逻辑处理 NULL。
- 失败时自动生成结构化反例。

### 5.2 核心 API

```rust
VeriEql::verify(
    sql1: &str,
    sql2: &str,
    schema: &[TableSchema],
    constraints: &serde_json::Value,
    bound: Bound,
    semantics: Semantics,
) -> Result<ProofReport, VeriEqlError>
```

`verify` 的工作流程：

1. 分别解析两条 SQL。
2. 将 AST 翻译为 VeriEQL IR。
3. 创建包含 `B` 个符号元组的数据库环境。
4. 应用完整性约束。
5. 声明输出元组变量，分别编码两条查询对该元组的成员关系谓词。
6. 断言对称差 `Q1(output) XOR Q2(output)`，调用 Z3 检查。
7. 若 UNSAT 则等价；若 SAT 则提取反例。

### 5.3 约束格式（JSON）

VeriEQL 通过 JSON 数组声明约束，每条约束是一个单键对象。

**主键约束**：

```json
[{"primary": [["EMP__ID"]]}]
```

**外键约束**：

```json
[{"foreign": [["EMP__DEPTNO"], ["DEPT__DEPTNO"]]}]
```

**非空约束**：

```json
[{"not_null": ["EMP__NAME"]}]
```

列名格式为 `表名__列名`，使用双下划线分隔。

### 5.4 QED vs VeriEQL 对比表

| 方面 | QED | VeriEQL |
|------|-----|---------|
| 定位 | Metamorphosis 规则验证 | 独立有界等价验证 |
| 方法 | 嵌入式 Z3 + 外部 prover 回退 | 直接 Z3 编码 |
| 完备性 | 支持片段内完备 | 有界（可配置 bound） |
| 依赖 | metamorphosis-core, ogsql-parser, z3 | ogsql-parser, z3 |
| 语义 | Bag | Bag + List |
| 反例 | 可选字符串 | 结构化 Counterexample |

---

## 6. 作为库使用

### 6.1 添加依赖

在 `Cargo.toml` 中：

```toml
[dependencies]
metamorphosis-core = { path = "../core" }  # 或 git 依赖
metamorphosis-rules = { path = "../rules" }
```

### 6.2 编程式重写示例

```rust
use metamorphosis_core::*;
use metamorphosis_rules::builtin_rules;

let registry = RuleRegistry::new(builtin_rules());
let engine = RewriteEngine::new(registry);
let config = RewriteConfig::default();

let ctx = RewriteContext {
    version: None,
    schema: Some(&schemas),
    config: &config,
    source_file: None,
    known_variables: None,
};

let result = engine.rewrite(&ctx, statements);

for stmt in &result.statements {
    println!("{}", SqlFormatter::new().format_statement(stmt));
}

for s in &result.suggestions {
    println!("suggestion from {}: {:?}", s.rule_id, s.action);
}
```

### 6.3 注册自定义规则

实现 `RewriteRule` 后即可加入注册表：

```rust
use metamorphosis_core::{RewriteRule, RewriteContext, RewriteAction, MatchResult, SafetyLevel, RuleCategory};
use ogsql_parser::ast::Statement;

#[derive(Debug)]
struct MyRule;

impl RewriteRule for MyRule {
    fn id(&self) -> &'static str { "my-rule" }
    fn description(&self) -> &'static str { "示例规则" }
    fn category(&self) -> RuleCategory { RuleCategory::Style }
    fn safety_level(&self) -> SafetyLevel { SafetyLevel::Manual }

    fn matches(&self, _ctx: &RewriteContext, _stmt: &Statement) -> MatchResult {
        MatchResult::Matched
    }

    fn apply(&self, _ctx: &RewriteContext, _stmt: &Statement) -> Option<RewriteAction> {
        Some(RewriteAction::Suggest {
            message: "这是一个建议".to_string(),
            severity: metamorphosis_core::Severity::Info,
        })
    }
}

let mut rules = builtin_rules();
rules.push(Box::new(MyRule));
let engine = RewriteEngine::new(RuleRegistry::new(rules));
```

---

## 7. 内置规则实现参考

`crates/rules` 包含 4 条内置规则，均实现 `metamorphosis_core::RewriteRule`。

| 规则 ID | 源文件 | 关键技术 |
|---------|--------|---------|
| `eliminate-select-star` | `crates/rules/src/eliminate_select_star.rs` | 基于 `SchemaMap` 将 `SELECT *` 展开为显式列列表。 |
| `detect-duplicate-eq-keys` | `crates/rules/src/detect_duplicate_eq_keys.rs` | `EqPredicateCollector` 收集 WHERE 等值条件，按 Tier 分级后生成 GROUP BY 探针。 |
| `subquery-to-join` | `crates/rules/src/subquery_to_join.rs` | AST 模式匹配，覆盖 EXISTS / IN / NOT EXISTS / NOT IN 四类子查询。 |
| `extract-candidate-values` | `crates/rules/src/extract_candidate_values.rs` | 参数检测 + 探针生成，提取参数化等值列的候选值。 |

共享模块：

- `crates/rules/src/eq_analyzer.rs`：提供 `EqPredicateCollector`，用于区分表列、参数、变量和字面量之间的等值关系。

---

## 8. 相关文档

| 文档 | 路径 | 说明 |
|------|------|------|
| 设计文档 | `docs/metamorphosis_design_doc.md` | 完整架构设计、数据流、规则范例、测试策略。 |
| QED 理论背景 | `docs/QED.md` | VLDB 2024 QED 论文技术拆解，包括 Q-expression、完整性约束、NULL 语义等。 |
| 贡献指南 | `docs/CONTRIBUTING.md` | 编码规范、提交规范、PR 流程。 |
| 最佳实践 | `docs/BEST-PRATICE.md` | Rust 编码最佳实践与项目约定。 |

---

*文档结束*
