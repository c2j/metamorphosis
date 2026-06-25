# Metamorphosis 用户手册

**版本**：v0.1.19

本手册面向使用 Metamorphosis 进行 SQL 语义重写、数据质量探针生成和语义等价验证的终端用户。阅读后，你可以完成安装、使用所有 CLI 命令、理解内置规则、配置 Schema、解读输出结果，以及将工具集成到 MCP 客户端。

---

## 目录

1. [安装与构建](#1-安装与构建)
2. [CLI 命令详解](#2-cli-命令详解)
   - 2.1 [rewrite](#21-rewrite--自动重写)
   - 2.2 [suggest](#22-suggest--生成建议与探针)
   - 2.3 [show-rules](#23-show-rules--列出规则)
   - 2.4 [verify](#24-verify--语义等价验证)
   - 2.5 [mcp](#25-mcp--启动-mcp-服务器)
   - 2.6 [inline](#26-inline--参数内联)
3. [内置规则详解](#3-内置规则详解)
4. [Schema 配置](#4-schema-配置)
5. [输入格式](#5-输入格式)
6. [安全级别与置信度](#6-安全级别与置信度)
7. [MCP 集成指南](#7-mcp-集成指南)
8. [常见问题](#8-常见问题)

---

## 1. 安装与构建

### 前置条件

- Rust 1.75 或更高版本
- git

### 从源码构建

```bash
git clone <仓库地址>
cd metamorphosis
cargo build --workspace --release
```

构建完成后，二进制文件位于：

```
target/release/metamorphosis
```

建议将其加入系统 PATH，或在调用时使用完整路径。

### 关于 Z3

`verify` 命令使用的 Z3 SMT 求解器采用静态链接方式嵌入二进制文件，运行时无需额外安装 Z3 动态库。

---

## 2. CLI 命令详解

Metamorphosis 提供六个子命令：`rewrite`、`suggest`、`show-rules`、`verify`、`mcp`、`inline`。所有命令都通过 `metamorphosis <COMMAND>` 调用。

### 2.1 `rewrite` - 自动重写

`rewrite` 自动执行 `Safe` 和 `Conditional` 级别的规则，输出重写后的 SQL。`Manual` 级别的规则不会自动替换原语句，因此不会在此命令中输出探针。

#### 参数表

| 参数 | 类型 | 说明 |
|------|------|------|
| `--file <PATH>` | 可选 | 输入 SQL 文件路径。省略或传入 `-` 时从 stdin 读取 |
| `--version <VERSION>` | 可选 | GaussDB/openGauss 版本字符串，例如 `5.0` |
| `--schema <PATH>` | 可选 | JSON Schema 文件路径，与 `--sql-dir` 互斥 |
| `--sql-dir <DIR>` | 可选 | 包含 DDL `.sql` 文件的目录，与 `--schema` 互斥 |
| `--rules <RULES>` | 可选 | 逗号分隔的规则 ID 列表，例如 `eliminate-select-star,subquery-to-join` |
| `--procedure <PATH>` | 可选 | 存储过程文件路径，用于提取变量名 |
| `--from-procedure` | Flag | 将输入文件视为存储过程文件，提取其中的 SQL 语句 |
| `--input-format <FORMAT>` | 可选 | 输入格式：`sql` 或 `csv`，默认按文件扩展名自动检测 |
| `--mybatis` | Flag | 启用 MyBatis 参数解析，支持 `#{param}` 语法 |

#### 示例

**基本文件输入**

```bash
metamorphosis rewrite --file query.sql --version 5.0 --schema schema.json
```

**从 stdin 读取**

```bash
echo "SELECT * FROM users WHERE id = 1;" | metamorphosis rewrite --schema schema.json
```

**CSV 输入**

```bash
metamorphosis rewrite --file queries.csv --input-format csv --schema schema.json
```

**从存储过程提取 SQL 并重写**

```bash
metamorphosis rewrite --from-procedure --file proc.sql --schema schema.json
```

**启用 MyBatis 参数解析**

```bash
metamorphosis rewrite --file mapper.xml --mybatis --schema schema.json
```

**使用 DDL 目录作为 Schema 来源**

```bash
metamorphosis rewrite --file query.sql --sql-dir ./ddl
```

### 2.2 `suggest` - 生成建议与探针

`suggest` 专门用于触发 `Manual` 级别的规则，生成数据质量探针 SQL 或文本建议。它不会修改原 SQL。

#### 参数表

`rewrite` 的全部参数均可用，额外增加：

| 参数 | 类型 | 说明 |
|------|------|------|
| `-o, --output <FORMAT>` | 可选 | 输出格式：`text`（默认）/ `json` / `tsv` / `csv` / `sql-only` |

#### 输出格式说明

**text（默认）**

每条语句前会打印来源头信息，随后列出命中的规则、置信度和探针 SQL。如果某条规则未命中，还会显示匹配失败原因。

示例输出：

```text
[1/1] SELECT @ query.sql:1-3
[detect-duplicate-eq-keys]  High
  Detect candidate keys from equality conditions and generate uniqueness probe
  ---------- PROBE ----------
  SELECT account_date, account_seqno, count(1) AS cnt
    FROM dat_clr_cash_dtl
   WHERE account_date = :date
   GROUP BY account_date, account_seqno
  HAVING count(1) > 1
   ORDER BY cnt DESC
   LIMIT 10;
  ----------------------------

  Rule match report:
    [X] eliminate-select-star
        No wildcard target (SELECT *) found in target list
    [X] subquery-to-join
        no rewritable subquery pattern (EXISTS/IN/NOT IN/NOT EXISTS) in WHERE
```

**json**

返回结构化的 JSON 对象，包含 `suggestions`、`match_failures` 和 `warnings` 字段，便于 CI/CD 流水线消费。

```json
{
  "suggestions": [
    {
      "rule_id": "detect-duplicate-eq-keys",
      "rule_description": "Detect candidate keys from equality conditions and generate uniqueness probe",
      "confidence": "High",
      "probe_sql": "SELECT account_date, account_seqno, count(1) AS cnt FROM dat_clr_cash_dtl WHERE account_date = :date GROUP BY account_date, account_seqno HAVING count(1) > 1 ORDER BY cnt DESC LIMIT 10;",
      "message": null,
      "purpose": "Candidate key duplicate detection: verify uniqueness of equality-condition columns"
    }
  ],
  "match_failures": [
    {
      "rule_id": "eliminate-select-star",
      "reason": "No wildcard target (SELECT *) found in target list"
    }
  ],
  "warnings": []
}
```

**tsv**

每行一条建议，字段之间用制表符分隔，格式为：

```
<rule_id>\t<confidence>\t<purpose>\t<probe_sql>
```

**csv**

第一行为表头，包含 `original_sql` 和所有规则 ID 列。后续每一行对应一条原始 SQL，单元格中填充对应规则生成的探针 SQL。无探针时为空字符串。

**sql-only**

仅输出探针 SQL，每条语句以分号结尾，适合直接复制到数据库客户端执行。

### 2.3 `show-rules` - 列出规则

列出所有内置规则及其元数据，无需任何参数。

```bash
metamorphosis show-rules
```

示例输出：

```text
4 built-in rules:

  1 eliminate-select-star
    Replace SELECT * with explicit column names using schema metadata
    category: semantic  safety: Safe

  2 detect-duplicate-eq-keys
    Detect candidate keys from equality conditions and generate uniqueness probe
    category: quality  safety: Manual

  3 extract-candidate-values
    Generate probe SQL showing existing values of parameterized WHERE equality columns
    category: quality  safety: Manual

  4 subquery-to-join
    Convert WHERE subqueries (EXISTS, IN, NOT EXISTS, NOT IN) to JOINs and suggest scalar subquery rewrites
    category: perf  safety: Conditional
```

### 2.4 `verify` - 语义等价验证

`verify` 使用嵌入的 Z3 SMT 求解器，证明或反驳两条 SQL 查询之间的语义等价性。验证前必须提供 Schema。

#### 参数表

| 参数 | 类型 | 说明 |
|------|------|------|
| `original` | 必填 | 原始 SQL 文件路径 |
| `rewritten` | 必填 | 重写后 SQL 文件路径 |
| `--schema <PATH>` | 可选 | JSON Schema 文件，与 `--sql-dir` 互斥 |
| `--sql-dir <DIR>` | 可选 | DDL 目录，与 `--schema` 互斥 |
| `--engine <ENGINE>` | 可选 | `qed`（默认）或 `verieql` |
| `--bound <N>` | 可选 | VeriEQL 的 bound，默认 2 |
| `-o, --output <FORMAT>` | 可选 | `text`（默认）或 `json` |

#### QED 与 VeriEQL 的区别

| 引擎 | 适用场景 | 特点 |
|------|----------|------|
| **qed** | 通用验证 | 基于 Q-expression 和 SMT 求解，支持主键、外键、Check 约束和 NULL 三值逻辑 |
| **verieql** | 有界验证 | 基于 OOPSLA 2024 的有界模型检测，通过 `--bound` 控制搜索范围，可能返回反例 |

#### 等价输出示例

```bash
metamorphosis verify original.sql rewritten.sql --schema schema.json --engine qed
```

```text
Equivalent (proven in 120ms)
  Original:  SELECT * FROM users WHERE id = 1
  Rewritten: SELECT id, name FROM users WHERE id = 1
```

#### 不等价输出示例

```text
Not Equivalent
  Column count: 3 (original) vs 2 (rewritten)
  Missing from rewrite: email
  Original:  SELECT id, name, email FROM users WHERE id = 1
  Rewritten: SELECT id, name FROM users WHERE id = 1
```

#### JSON 输出示例

```bash
metamorphosis verify original.sql rewritten.sql --schema schema.json -o json
```

```json
{
  "result": "Equivalent",
  "original": "SELECT * FROM users WHERE id = 1",
  "rewritten": "SELECT id, name FROM users WHERE id = 1",
  "elapsed_ms": 120,
  "engine": "qed"
}
```

### 2.5 `mcp` - 启动 MCP 服务器

启动基于 stdio 传输的 MCP 服务器，供 Claude Desktop、Cursor 等 AI 助手调用。

```bash
metamorphosis mcp
```

该命令无需参数，启动后持续监听标准输入，直到 stdin 关闭。MCP 客户端需要将命令配置为外部工具。

### 2.6 `inline` - 参数内联

`inline` 将 SQL 中的参数占位符替换为字面量，输出可直接执行的 SQL。支持以下占位符风格：

- JDBC 位置参数 `?`（用 `--val` 按顺序提供）
- PostgreSQL 编号参数 `$1`、`$2`（同样用 `--val` 提供）
- MyBatis 命名参数 `#{name}` / `${name}`（用 `--param` 提供）
- 存储过程变量（`--procedure` 提供变量白名单；或当列名匹配 `--param` 的 key 时作为隐式回退替换）

#### 参数表

| 参数 | 类型 | 说明 |
|------|------|------|
| `--file <PATH>` | 可选 | 输入 SQL 文件路径。省略或传入 `-` 时从 stdin 读取 |
| `--param <KEY=VALUE>` | 可重复 | 命名参数，值类型自动推断（见下表） |
| `--param-string <KEY=VALUE>` | 可重复 | 命名参数，强制为字符串类型，绕过类型推断 |
| `--val <VALUE>` | 可重复 | 位置参数值，按出现顺序对应 `?` / `$1`、`$2`…，类型推断同 `--param` |
| `--params-file <PATH>` | 可选 | 从 JSON 文件加载参数 |
| `--mybatis` | Flag | 启用 MyBatis `#{param}` / `${param}` 解析 |
| `--procedure <PATH>` | 可选 | 存储过程文件路径，用于提取已声明的变量名白名单 |
| `--from-procedure` | Flag | 将输入文件视为存储过程，提取其中的 SQL 语句逐条内联 |
| `-o, --output <FORMAT>` | 可选 | 输出格式：`sql-only`（默认）/ `text` / `json` |

#### 值类型推断规则

`--param` 与 `--val` 的值按以下顺序推断为 SQL 字面量类型（`--param-string` 永远推断为字符串）：

| 输入示例 | 推断结果 | 输出 SQL |
|----------|----------|----------|
| `NULL` / `TRUE` / `FALSE`（大小写不敏感） | NULL / 布尔 | `NULL` / `TRUE` / `FALSE` |
| `'O''Brien'`、`'001'`（单引号包裹） | 字符串，`''` 还原为 `'` | `'O''Brien'`、`'001'` |
| `001`、`010`（前导零全数字） | 字符串（保留数值码） | `'001'` |
| `42`、`-1` | 整数 | `42`、`-1` |
| `3.14` | 浮点 | `3.14` |
| 其他 | 字符串 | `'...'` |
| **`<base>::<type>` 类型转换** | Cast，`base` 递归推断 | `'20260101'::date` 等 |

**类型转换 `::` 的处理**：值中出现的 `<base>::<type>` 会被识别为 SQL 类型转换，`::` 必须位于引号之外（因此 `'a::b'` 不会被误判）。`base` 按上表递归推断，`type` 原样保留（含精度，如 `numeric(10,2)`）。

> ⚠️ **Shell 引号陷阱**：在 shell 中，`--param d='20260101'::date` 的单引号会被 shell 吃掉，程序实际收到 `d=20260101::date`，此时 `20260101` 会被推断为**整数**，输出 `20260101::date`（在 openGauss 中 integer→date 无隐式转换会报错）。要得到正确的 `'20260101'::date`，请用**双引号包裹整个参数**，让内层单引号存活：

```bash
--param "d='20260101'::date"
```

#### 示例

**命名参数（MyBatis `#{}` / 变量）**

```bash
metamorphosis inline --file query.sql --mybatis --param status=active --param level=3
```

**JDBC 位置参数 `?`**

```bash
metamorphosis inline --file query.sql --val ACC001 --val 100
```

**存储过程变量内联**

```bash
metamorphosis inline --file proc.sql --from-procedure \
  --param p_i_coincode=001 --param-string p_i_scdm=1 \
  --param "v_date='20260101'::date"
```

上述 `v_date` 会输出 `'20260101'::date`，而非把 `::date` 错误吞进字符串。

**从 JSON 文件加载参数**

```bash
metamorphosis inline --file query.sql --params-file params.json
```

`params.json` 格式：

```json
{
  "status": "active",
  "level": 3,
  "positional": ["ACC001", 100]
}
```

---

## 3. 内置规则详解

### 3.1 `eliminate-select-star`

| 属性 | 值 |
|------|-----|
| ID | `eliminate-select-star` |
| 类别 | `Semantic` |
| 安全级别 | `Safe` |
| 置信度 | `High` |

#### 功能

将 `SELECT *` 或 `SELECT t.*` 展开为显式列名列表。展开依据是 `--schema` 或 `--sql-dir` 提供的 Schema 元数据。

#### 匹配条件

- 语句为 `SELECT`
- 目标列表中包含 `*` 或限定通配符 `t.*`
- 提供了 Schema，且能解析出基表名

#### 输入输出示例

输入：

```sql
SELECT * FROM users WHERE id = 1;
```

输出：

```sql
SELECT id, name, email FROM users WHERE id = 1;
```

#### 注意事项

- 必须提供 Schema，否则规则无法命中。
- 如果 FROM 子句包含 JOIN 或多个表，该规则仅解析第一个基表。
- 展开的列名按 Schema 中的顺序输出。

### 3.2 `detect-duplicate-eq-keys`

| 属性 | 值 |
|------|-----|
| ID | `detect-duplicate-eq-keys` |
| 类别 | `DataQuality` |
| 安全级别 | `Manual` |
| 置信度 | `High` 或 `Medium` |

#### 功能

从 `WHERE` 子句的等值条件中提取候选键，生成 `GROUP BY` 聚合探针 SQL，用于验证这些列的组合是否具有唯一性。该规则不会替换原 SQL，仅作为数据质量检查建议输出。

#### 匹配条件

- 语句为 `SELECT`
- `WHERE` 子句中至少包含两个 Tier 1 等值条件

#### Tier 分级系统

等值条件按右侧表达式的稳定性分为不同层级，决定是否纳入 `GROUP BY`：

| 层级 | 右侧表达式类型 | 示例 | 处理方式 |
|------|---------------|------|----------|
| **Tier 1** | 参数、占位符、MyBatis 参数、已知变量 | `= :date` / `= ?` / `= #{id}` / `= in_date` | 纳入 `GROUP BY`，作为候选键 |
| 保留条件 | 字面量常量 | `= '20250101'` | 保留为 `WHERE` 过滤条件 |
| 保留条件 | 列与列之间的等值（关联条件） | `t2.id = t.id` | 保留为 `WHERE` 或 `JOIN` 条件 |
| 保留条件 | 子查询、函数、动态表达式 | `= (SELECT MAX(date) FROM config)` | 保留为 `WHERE` 过滤条件 |

当前实现主要基于 Tier 1 等值条件生成探针。字面量等值不会被当作候选键，因为它们的值固定，不适合用于验证组合唯一性。

#### 输入输出示例

输入：

```sql
SELECT trade_code
  FROM dat_clr_cash_dtl
 WHERE account_date = :date
   AND account_seqno = :seq
   AND clear_type = '4';
```

输出探针：

```sql
SELECT account_date, account_seqno, count(1) AS cnt
  FROM dat_clr_cash_dtl
 WHERE clear_type = '4'
 GROUP BY account_date, account_seqno
HAVING count(1) > 1
 ORDER BY cnt DESC
 LIMIT 10;
```

#### 注意事项

- 如果 `HAVING count(1) > 1` 返回结果，说明候选键组合存在重复，需要检查业务逻辑或数据约束。
- 探针默认 `LIMIT 10`，可通过配置文件中的 `probe_default_limit` 调整。
- 当 `WHERE` 中存在子查询时，置信度为 `Medium`。

### 3.3 `subquery-to-join`

| 属性 | 值 |
|------|-----|
| ID | `subquery-to-join` |
| 类别 | `Performance` |
| 安全级别 | `Conditional` |
| 置信度 | `Medium` |

#### 功能

将 `WHERE` 子句中的相关子查询或 `IN`/`NOT IN` 子查询改写为 `JOIN`，从而提升查询性能。

#### 匹配条件

- 语句为 `SELECT`
- `WHERE` 子句中存在以下任意模式：`EXISTS`、`NOT EXISTS`、`IN (SELECT ...)`、`NOT IN (SELECT ...)`
- 子查询必须足够简单：单表、`FROM` 中无 JOIN、无 `GROUP BY`、无 `HAVING`、无集合运算、无聚合函数

#### 四种改写模式

| 原模式 | 改写结果 | 等价前提 |
|--------|----------|----------|
| `EXISTS (SELECT ...)` | `INNER JOIN` | 语义等价 |
| `NOT EXISTS (SELECT ...)` | `LEFT JOIN + WHERE right_col IS NULL` | 子查询侧连接列非 NULL |
| `expr IN (SELECT ...)` | `INNER JOIN` | 语义等价 |
| `expr NOT IN (SELECT ...)` | `LEFT JOIN + WHERE right_col IS NULL` | 子查询侧连接列非 NULL |

#### 输入输出示例

**EXISTS 转 INNER JOIN**

输入：

```sql
SELECT o.id
  FROM orders o
 WHERE EXISTS (SELECT 1 FROM order_items i WHERE i.order_id = o.id);
```

输出：

```sql
SELECT o.id
  FROM orders o
  JOIN order_items i ON i.order_id = o.id;
```

**NOT EXISTS 转 LEFT JOIN + IS NULL**

输入：

```sql
SELECT c.id
  FROM customers c
 WHERE NOT EXISTS (SELECT 1 FROM orders o WHERE o.customer_id = c.id);
```

输出：

```sql
SELECT c.id
  FROM customers c
  LEFT JOIN orders o ON o.customer_id = c.id
 WHERE o.id IS NULL;
```

**IN 转 INNER JOIN**

输入：

```sql
SELECT u.name
  FROM users u
 WHERE u.id IN (SELECT user_id FROM orders);
```

输出：

```sql
SELECT u.name
  FROM users u
  JOIN orders ON orders.user_id = u.id;
```

**NOT IN 转 LEFT JOIN + IS NULL**

输入：

```sql
SELECT p.name
  FROM products p
 WHERE p.id NOT IN (SELECT product_id FROM order_items);
```

输出：

```sql
SELECT p.name
  FROM products p
  LEFT JOIN order_items ON order_items.product_id = p.id
 WHERE order_items.product_id IS NULL;
```

#### 注意事项

- `NOT EXISTS` 和 `NOT IN` 的改写仅在子查询侧连接列不存在 NULL 时与原 SQL 语义等价。如果列可能为 NULL，建议先用 `verify` 验证。
- `SELECT` 目标列表中的标量子查询不会被自动改写，仅会输出文本建议。

### 3.4 `extract-candidate-values`

| 属性 | 值 |
|------|-----|
| ID | `extract-candidate-values` |
| 类别 | `DataQuality` |
| 安全级别 | `Manual` |
| 置信度 | `High` 或 `Medium` |

#### 功能

从参数化的等值条件中提取参数对应的列，生成探针 SQL，展示这些列在现有数据中的实际取值。当业务 SQL 因参数值不存在而返回空结果时，可用该探针快速找到有效取值。

#### 匹配条件

- 语句为 `SELECT`
- `WHERE` 子句中至少存在一个参数化等值条件，例如 `col = :param` 或 `col = #{param}`

#### 输入输出示例

输入：

```sql
SELECT special_sql
  FROM t
 WHERE clear_type = '4'
   AND task_status = :ts;
```

输出探针：

```sql
SELECT task_status, count(1) AS cnt
  FROM t
 WHERE clear_type = '4'
 GROUP BY task_status
 ORDER BY cnt DESC
 LIMIT 10;
```

#### 注意事项

- 仅对参数化等值列生成 `GROUP BY`，字面量等值保留为过滤条件。
- 当有多个参数化列时，探针会展示这些列的组合取值分布。
- 该规则仅生成建议，不会改写原 SQL。

---

## 4. Schema 配置

Schema 是多项规则正常工作的前提，尤其是 `eliminate-select-star` 和验证命令。

### JSON Schema 格式

JSON Schema 是一个嵌套映射：表名 -> 列名 -> 类型。

```json
{
  "users": {
    "id": "integer",
    "name": "varchar",
    "email": "varchar"
  },
  "orders": {
    "id": "integer",
    "user_id": "integer",
    "amount": "decimal(10,2)"
  }
}
```

保存为 `schema.json` 后使用：

```bash
metamorphosis rewrite --file query.sql --schema schema.json
```

### DDL 目录模式

也可以使用 `--sql-dir` 指定一个包含 DDL 文件的目录。Metamorphosis 会自动解析目录下所有 `.sql` 文件中的 `CREATE TABLE` 和 `ALTER TABLE ... ADD COLUMN` 语句，提取表结构。

目录示例：

```
ddl/
├── 001_users.sql
├── 002_orders.sql
└── 003_alter_users.sql
```

```bash
metamorphosis rewrite --file query.sql --sql-dir ./ddl
```

解析成功后，命令行会提示提取到的表数量：

```text
Extracted schema from 2 table(s) in './ddl'
```

### 何时需要 Schema

| 场景 | 是否需要 Schema |
|------|----------------|
| 使用 `eliminate-select-star` | 必须 |
| 使用 `verify` | 必须 |
| 仅使用 `detect-duplicate-eq-keys` 或 `extract-candidate-values` | 可选，不提供也能命中 |
| 使用 `subquery-to-join` | 可选 |

---

## 5. 输入格式

### SQL 文件

普通文本文件，可包含多条 SQL 语句，语句之间用分号分隔。Metamorphosis 会逐条解析并处理。

```sql
SELECT * FROM users WHERE id = 1;
SELECT * FROM orders WHERE user_id = 2;
```

### CSV 文件

CSV 文件遵循 RFC 4180 规范，每行一条 SQL 语句。支持以下特性：

- 引号字段 `"..."`
- 字段内嵌双引号通过 `""` 转义
- 引号字段内可包含换行符
- 自动跳过 UTF-8 BOM
- 跳过空行和以 `#` 或 `--` 开头的注释行

示例：

```csv
"SELECT * FROM users WHERE id = 1"
"SELECT * FROM orders WHERE user_id = 2"
```

多列 CSV 时，工具会读取第一个非空字段作为 SQL 文本。

### Stdin 管道

省略 `--file` 或传入 `--file -` 时从标准输入读取：

```bash
cat query.sql | metamorphosis rewrite --schema schema.json
```

```bash
echo "SELECT * FROM users;" | metamorphosis rewrite --schema schema.json
```

### 存储过程提取

使用 `--from-procedure` 时，Metamorphosis 会从 PL/SQL 存储过程文件中提取 SQL 语句，并保留行号等来源信息。

```bash
metamorphosis rewrite --from-procedure --file proc.sql --schema schema.json
```

结合 `--procedure <PATH>` 可在解析普通 SQL 时加载另一个存储过程文件中的变量名，帮助规则识别已知变量。

```bash
metamorphosis rewrite --file query.sql --procedure proc.sql --schema schema.json
```

### MyBatis 参数

启用 `--mybatis` 后，解析器会识别 `#{paramName}` 形式的参数，并在等值分析中将其视为参数化条件。

```bash
metamorphosis suggest --file mapper.xml --mybatis --schema schema.json
```

---

## 6. 安全级别与置信度

### SafetyLevel

每条规则都声明了安全级别，决定引擎如何处理其输出。

| 级别 | 含义 | 用户影响 |
|------|------|----------|
| **Safe** | 语义等价改写 | 引擎自动执行，可直接使用输出 |
| **Conditional** | 在满足前提条件时语义等价 | 引擎执行前会检查前提，用户应确认前提成立 |
| **Manual** | 非语义等价 | 仅生成建议或探针，绝不自动替换原 SQL |

### Confidence

每个改写或探针结果都携带置信度，用于提示用户是否需要人工复核。

| 级别 | 含义 |
|------|------|
| **High** | 单表、无子查询、纯参数等值，结果确定 |
| **Medium** | 结构发生变化但语义可追踪，例如穿透派生表或移除 `EXISTS` |
| **Low** | 涉及多表 JOIN 或动态子查询，建议人工复核 |

---

## 7. MCP 集成指南

Metamorphosis 提供 MCP 服务器，可通过 stdio 与 Claude Desktop、Cursor 等客户端集成。

### 配置示例（Claude Desktop）

编辑 Claude Desktop 配置文件：

- macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`
- Windows: `%APPDATA%\Claude\claude_desktop_config.json`

添加如下内容：

```json
{
  "mcpServers": {
    "metamorphosis": {
      "command": "/Users/c2j/Projects/Desktop_Projects/DB/metamorphosis/target/release/metamorphosis",
      "args": ["mcp"]
    }
  }
}
```

请将 `command` 替换为实际二进制文件的绝对路径。

### MCP 工具

服务器暴露 5 个工具：

#### 1. `rewrite_sql`

应用 `Safe` 和 `Conditional` 规则重写 SQL。

参数：

| 参数 | 类型 | 说明 |
|------|------|------|
| `sql` | string | 输入 SQL |
| `version` | string (可选) | 数据库版本 |
| `schema_json` | string (可选) | 内联 JSON Schema |
| `schema_path` | string (可选) | JSON Schema 文件路径 |
| `sql_dir` | string (可选) | DDL 目录 |
| `rules` | string (可选) | 逗号分隔的规则 ID |

响应字段：

```json
{
  "changed": true,
  "rewritten_sql": ["SELECT id, name FROM users WHERE id = 1;"],
  "match_failures": [],
  "warnings": []
}
```

#### 2. `suggest_probes`

生成 `Manual` 级别的数据质量探针和建议。参数与 `rewrite_sql` 相同。

响应字段：

```json
{
  "suggestions": [
    {
      "rule_id": "detect-duplicate-eq-keys",
      "rule_description": "...",
      "confidence": "High",
      "probe_sql": "SELECT ...",
      "message": null,
      "purpose": "..."
    }
  ],
  "match_failures": [],
  "warnings": []
}
```

#### 3. `list_rules`

无参数，返回所有内置规则的元数据。

响应字段：

```json
{
  "rules": [
    {
      "id": "eliminate-select-star",
      "description": "...",
      "category": "Semantic",
      "safety_level": "Safe",
      "default_enabled": true
    }
  ]
}
```

#### 4. `verify_equivalence`

验证两条 SQL 的语义等价性。

参数：

| 参数 | 类型 | 说明 |
|------|------|------|
| `original_sql` | string | 原始 SQL |
| `rewritten_sql` | string | 重写后 SQL |
| `engine` | string (可选) | `qed`（默认）或 `verieql` |
| `bound` | number (可选) | VeriEQL bound，默认 2 |
| `schema_json` | string (可选) | 内联 JSON Schema |
| `schema_path` | string (可选) | JSON Schema 文件路径 |
| `sql_dir` | string (可选) | DDL 目录 |

响应字段：

```json
{
  "result": "Equivalent",
  "engine": "qed",
  "original_sql": "SELECT ...",
  "rewritten_sql": "SELECT ...",
  "elapsed_ms": 120,
  "bound": null,
  "counterexample": null,
  "column_details": null
}
```

#### 5. `extract_schema`

从 DDL 目录提取 Schema。

参数：

| 参数 | 类型 | 说明 |
|------|------|------|
| `sql_dir` | string | DDL 目录路径 |

响应字段：

```json
{
  "table_count": 2,
  "schema": {
    "users": {
      "id": "INTEGER",
      "name": "VARCHAR"
    }
  }
}
```

---

## 8. 常见问题

### 编码问题（GBK/UTF-8）

Metamorphosis 在读取 SQL 和 DDL 文件时会自动检测编码。支持的编码包括 UTF-8、GB18030/GBK、EUC-JP、EUC-KR、BIG5、UTF-16 LE/BE。如果文件以 BOM 开头，也会自动去除。如果所有编码检测都失败，会回退到有损 UTF-8 替换，确保不会直接崩溃。

### Schema 缺失时的行为

- `eliminate-select-star` 会直接跳过，并在 `suggest` 的匹配失败报告中说明原因。
- `verify` 会报错退出，因为验证必须依赖 Schema。
- `detect-duplicate-eq-keys`、`extract-candidate-values`、`subquery-to-join` 不依赖 Schema 也能工作。

### 多语句文件处理

SQL 文件可以包含多条语句。`rewrite` 和 `suggest` 会逐条处理，并分别输出每条语句的结果。`verify` 要求每个文件只包含一条语句，否则会报错。

### CSV 格式注意事项

- 每条记录建议只包含一个 SQL 字段，多余列会被忽略。
- 字段内包含逗号、换行或双引号时，必须整体用双引号包裹。
- 双引号字符在字段内必须写成两个双引号 `""`。
- CSV 输入不支持从 stdin 读取，必须通过 `--file` 指定文件。

---

**文档结束**
