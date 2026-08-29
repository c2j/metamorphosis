# AGENTS.md — Metamorphosis

## TDD 工作流（Red → Green → Refactor）

本仓库采用测试驱动开发。一次循环只锁定一个行为。探索草稿不得直接合入，必须按本文件用 TDD 重写。

### 先读再改

1. 确认改动落在哪个 crate（本仓库是 Cargo workspace，见仓库地图）。
2. 只用本文件列出的 cargo 命令；不要发明裸 `cargo update`。本仓库**没有** `rust-toolchain.toml`，toolchain 以 CI 使用的 `stable` 为准，不要擅自切换或加 `+nightly`。
3. 先跑与改动相关的最小测试；提交前再跑 workspace 门禁（fmt + clippy + test）。
4. 完成一个循环后按「完成标准与汇报」汇报，不要只说「做完了」。

### Never / Ask first / Always

**Never（不必请示，直接禁止）**
- 删除、注释、跳过已有测试：`#[ignore]`、注释掉 `#[test]`、把断言改成 `is_ok()` / `unwrap()` 了事
- 修改人类已有测试的断言来迁就实现
- 先提交无测试的业务行为，再「回头补」
- 写永真测试：无断言、只检查 `is_some()`、只 verify 调用次数不查参数与状态
- 用全量端到端测试覆盖本可单测完成的改动
- 提交半成品；每次对人类可见的结果必须能构建且相关测试为绿
- 把探索草稿、临时脚本、调试 `dbg!`/`println!` 留在主代码

**Ask first**
- 改人类已有测试（含断言、fixture、golden 期望值）
- 新增运行时依赖、`unsafe`、新的 workspace crate、新的外部服务
- 为不可测代码做超出当前改动路径的重构
- 接受/更新 golden file 或固定 fixture 的期望值，且行为含义发生变化
- 关闭 clippy lint、新增 `#[allow]`

**Always**
- 改遗留路径前：先写特征测试，锁定当前可观察行为（允许丑，必须可重复）
- 新行为：先有会失败的行为断言，再写最少实现
- 难以测试时：先造接缝，再写测试
- 测试名描述行为：`should_reject_negative_amount`
- 现有测试因你的改动失败：修实现，不修测试（除非人类明确要求）

测试权限：

| 测试来源 | 权限 |
|---|---|
| 人类已有测试 | 只读 |
| 本任务新建测试 | 可改，直到该行为稳定 |
| 过时或环境偶发失败 | 只报告，不擅自跳过 |

### 工作流

**Red** — 写生产行为之前先写测试；测试必须能被收集且必须失败（断言失败，或因缺失 API 导致编译失败，二者都算合法 Red）。修改已有功能先写特征测试锁定当前输出。一次只加一个行为的测试。

**Green** — 只写让当前失败测试通过的最少代码。禁止删掉/改掉失败测试、一次引入多个未验证变更、用更宽断言或 `unwrap()` 换绿。

**Refactor** — 相关测试全绿后才重构；重构后立刻跑同一组测试；范围限于当前 crate。

**探索 vs 实现** — 需求或方案不清可写草稿验证；草稿不得合并；方案确定后必须走 TDD 重写。

### 遗留代码与接缝

**特征测试** — 锁定现有行为，不是证明它正确。本仓库**未引入 `insta`**，用固定 fixture + 显式断言，或与 golden 文件逐字比对。更新期望值必须在汇报里写清 diff 含义；默认不接受「看起来差不多」。

**接缝（优先顺序，靠后的更差）**
1. trait + 泛型或 `impl Trait`，测试用假类型
2. 用类型去掉非法状态（enum / newtype），而不是在测试里补分支
3. 时钟、ID、熵、文件系统做成可注入依赖；测试用 `tempfile` / 内存实现
4. `unsafe` 不是接缝。新增 `unsafe` 必须 Ask first，并写 `SAFETY` 注释

只给即将修改的代码路径补测试，不要一次性「补全覆盖率」。

### 测试分层

| 层级 | 位置 | 测什么 |
|---|---|---|
| 单元 | `src` 内 `#[cfg(test)] mod tests` | 模块不变量、错误类型、状态转换 |
| 集成 | `tests/*.rs` | 公共 API；不可访问私有项 |
| 文档测试 | `///` 示例 | 公共 API 必须可运行；禁止滥用 `no_run` |
| CLI/二进制 | 项目惯用方式（如 `assert_cmd`） | 退出码与 stdout 契约 |
| 不变量 | `proptest`（项目已用时） | 往返解析、幂等、单调性 |
| 特征/golden | 固定 fixture（本仓库未引入 `insta`） | 遗留输出；更新期望值必须说明 |

不要把本该测公共契约的内容塞进 `#[cfg(test)]` 去读私有字段。

Rust 的 Red 允许是：测试引用了尚不存在的类型/函数导致编译失败。不要为了先编译而写空 `todo!()` 再补测试——可以留 `todo!()` 仅作为 Green 的最小占位，且下一步必须替换。

### Rust Never 补遗
- 库代码（非 main/example/测试）用 `unwrap` / `expect` / `panic!` 做控制流
- 无必要 `unsafe`；有则必须 `SAFETY` 注释
- 一次性 `cargo update` 整个 lockfile
- 用 `#[allow(...)]` 静默应修复的 lint
- 为绿而改 golden/期望值却不解释行为是否应该变

### 命令

```bash
# 单测（循环内）
cargo test -p metamorphosis-core <test_name>
cargo test -p metamorphosis-rules <test_name>

# 当前 crate
cargo test -p metamorphosis-core
cargo test -p metamorphosis-rules
cargo test -p metamorphosis-regress       # 回归 harness（正式证明 + DB 执行双验证）

# 提交前门禁（与 .github/workflows/qed-verify.yml 一致）
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

> CI 的 clippy 带 `-D warnings`——本地漏掉 `-D warnings` 会出现「本地绿、CI 红」。

> 库代码只用 `thiserror`（不用 `anyhow`）；`core` crate 零 IO 依赖；规则测试优先用 `#[rule_test]` DSL（若尚未实现则用普通单元测试，测试名描述行为）。

### 完成标准与汇报

提交或交还人类前，确认：
- [ ] 新行为有失败→通过的测试
- [ ] 修改的遗留路径有特征测试
- [ ] 未删除、跳过、改写人类已有测试
- [ ] 已跑与改动匹配的门禁（fmt + clippy + test）
- [ ] `cargo fmt` 与 clippy 干净
- [ ] 没有把草稿、调试输出、无主 lockfile 大面积变更带上

每个 TDD 循环汇报：
1. 测试了什么行为（测试函数名）
2. 最小实现改了哪些文件
3. 是否重构、边界在哪
4. 实际执行的命令和结果（通过 / 失败原因；不要只写「测过了」）

### 质量判断（自我检查）
- 这条测试在实现写错时会失败吗？
- 我是否在测行为，而不是私有实现细节？
- 我是否用 skip、更宽断言、unwrap、golden 盲收换绿？
- 命令是否来自本文件，而不是我编的？


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
