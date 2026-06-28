# metamorphosis-regress

带**双重等价性守护**的回归测试框架：每个 case 同时验证
(1) 形式化证明（Z3 SMT，离线）
(2) 真实数据库执行结果集对比（openGauss）

当数据库不可达时，DB 维度自动 skip 并警告，verify 维度仍照常运行。

## 目录结构

```
tests/regress/
├── Cargo.toml
├── config.toml.example       # 复制为 config.toml 后填值（gitignored）
├── src/                      # harness 实现（lib + bin）
├── tests/regress.rs          # cargo test 入口
└── cases/
    └── 001_select_star_basic/
        ├── case.toml         # 元数据 + 双维度开关
        ├── original.sql
        ├── rewritten.sql
        ├── schema.sql        # CREATE TABLE DDL
        └── data.sql          # INSERT 初始数据
```

## case.toml 字段

| 字段 | 默认 | 用途 |
|------|------|------|
| `name` | (必填) | case 标识 |
| `description` | "" | 描述 |
| `rule` | nil | 关联的 metamorphosis 规则 ID |
| `verify.enabled` | true | 是否跑形式化验证 |
| `verify.engine` | "qed" | "qed" 或 "verieql" |
| `verify.bound` | 2 | VeriEQL bound（Qed 忽略；必须 ≥ 1） |
| `verify.expect` | "equivalent" | 期望的 verify verdict：`equivalent` / `not_equivalent` / `unknown` / `any` |
| `db.enabled` | true | 是否跑 DB 实测 |
| `db.compare` | "unordered" | "ordered" / "unordered" / "set" |
| `db.expect` | "equal" | 期望的 DB 结果：`equal` / `mismatch` / `any` |

## expect 字段语义

`expect` 让 case 声明**期望**的判定结果，harness 用实际结果与期望对比——
匹配则 PASS，不匹配则 FAIL。这样可以用反例验证 harness 真能识别错误：

| Case 类型 | verify.expect | db.expect | 说明 |
|-----------|---------------|-----------|------|
| 正例（改写正确） | `equivalent` | `equal` | 默认；改写确实等价 |
| 反例（故意破坏） | `not_equivalent` | `mismatch` | 验证 verify 真能识别错误 |
| 只测 verify | `not_equivalent` | `any` | 不关心 DB 结果（如 DML 改写） |
| Debug harness | `any` | `any` | 跳过判定（不建议入库） |

## compare 模式

- **ordered**：行序敏感（SQL 自带 `ORDER BY` 时用）
- **unordered**（默认）：两边各自 `sort()` 后对比
- **set**：两边各自 `sort()` + `dedup()` 后对比（适合 `DISTINCT` 场景）

## 运行

### 1. 配置数据库连接

```bash
cd tests/regress
cp config.toml.example config.toml
# 编辑 config.toml 填入真实连接串
```

或者用环境变量覆盖：

```bash
export DATABASE_URL=postgres://gaussdb:pwd@localhost:5432/postgres
```

未配置 / 连不上时，DB 维度自动 skip。

### 2. 运行方式

```bash
# A. CLI harness（推荐本地开发，输出更详细）
cargo run -p metamorphosis-regress

# B. cargo test 入口（CI 集成）
cargo test -p metamorphosis-regress -- --nocapture --test-threads=1

# C. 用环境变量指定 config 文件
REGRESS_CONFIG=/path/to/config.toml cargo run -p metamorphosis-regress
```

### 3. 添加新 case

1. 在 `cases/` 下新建 `<NNN>_<description>/` 目录
2. 至少放：`case.toml` + `original.sql` + `rewritten.sql` + `schema.sql`
3. 可选：`data.sql` 提供 INSERT seed
4. 无需改任何 Rust 代码

## DB 隔离策略

每个 case 创建独立 schema：`regress_<dir>_<hash>`，所有 DDL/数据/查询
都在该 schema 内执行；case 跑完后 `DROP SCHEMA CASCADE`，互不污染。

查询语句在 `BEGIN...ROLLBACK` 事务里执行（即使原 SQL 是 DML 也不留痕），
保证同一 case 可重复运行。

## 双重验证的语义

| verify | db | 结论 |
|--------|-----|------|
| Equivalent | equal | ✅ 改写正确 |
| NotEquivalent | mismatch | ⚠️ verify 与 DB 一致，改写确实不等价 |
| Equivalent | mismatch | ⚠️ 形式化证明说等价但实测不等 — 可能 schema 不完整或 DB 实现 quirk |
| NotEquivalent | equal | ⚠️ verify 误报（bound 太小）或两边碰巧在该数据集上等价 |
| Unknown | * | ❔ verify 维度软失败（见下） |

后两种情况建议人工审查 case 的 schema.sql / data.sql 是否覆盖足够场景。

## 已知限制

### 1. Unknown 软失败

Z3 返回 `Unknown`（超时 / 表达式太复杂）时，harness 报 `UNKNOWN` 状态，
不计入 `failed` 但单独计数。CI 若希望把 Unknown 视为失败，可改 reporter.rs
里的 `is_pass()` 判定逻辑。

### 2. DML 限制

DB 维度目前用 `prepare + query` 执行，要求 SQL 返回结果集。非 SELECT
语句（如 `DELETE`、`UPDATE` 无 `RETURNING`）会触发 `exec_query` 错误。
对于 `delete-to-truncate`、`reject-no-where-dml` 等改写 DML 的规则，
建议在 `case.toml` 里设 `[db] enabled = false`，仅用 verify 维度。

### 3. NUMERIC 类型精度

`NUMERIC` / `DECIMAL` 列经 `f64` 渲染后 `to_string()`，极端精度可能丢损
（如 `0.1 + 0.2` 类型路径差异）。如 case 对此敏感，可在 SQL 里用
`::TEXT` 或 `::FLOAT8` 显式转换。

### 4. VeriEQL 仅支持 Bag 语义

`compare = "set"` 仅影响 DB 维度的对比方式；verify 维度（VeriEQL）当前
硬编码 `Semantics::Bag`。若 case 涉及 `DISTINCT` 改写并使用 set 模式，
DB 维度与 verify 维度的语义基准不一致，需谨慎解读。
