# Metamorphosis 设计文档
## SQL 语义重写与数据质量探针引擎

**版本**：v1.0-draft  
**日期**：2026-06-02  
**状态**：可指导开发（Development-Ready）  

---

## 1. 项目概述

### 1.1 定位
Metamorphosis 是构建于 `ogsql-parser` (https://github.com/c2j/ogsql-parser/)之上的 **SQL 语义重写与数据质量探针引擎**。它不提供新的解析能力，而是消费 `ogsql-parser` 产出的 AST，通过可插拔的规则体系，将原始 SQL 转换为具有诊断、治理、优化价值的衍生 SQL。

### 1.2 核心设计原则

| 原则 | 说明 |
|------|------|
| **语义分层** | Java 控制流 / SQL AST / 重写结果三层严格分离，不污染核心 AST |
| **可扩展性优先** | 所有规则基于 Trait 注册，支持内置、配置、插件三种来源 |
| **安全分级** | 每条规则声明 `SafetyLevel`，引擎据此决定是否自动执行、人工确认或仅建议 |
| **置信度透明** | 改写结果必须携带 `Confidence` 与 `Path` 元数据，拒绝黑盒 |
| **版本兼容** | 与 `ogsql-parser` 的 `GaussVersion` 联动，规则可按版本启用/禁用 |

### 1.3 命名空间

```
metamorphosis/
├── core/          # 引擎与抽象
├── rules/         # 内置规则库
├── cli/           # 命令行入口
├── tests/         # 规则级与集成测试
└── docs/          # 设计文档
```

---

## 2. 架构设计

### 2.1 整体架构（四层）

```
┌─────────────────────────────────────────────────────────────┐
│  Layer 4: 应用接口层                                          │
│  CLI (metamorphosis rewrite/suggest) / HTTP API / MCP Tool   │
├─────────────────────────────────────────────────────────────┤
│  Layer 3: 规则编排层                                          │
│  RuleRegistry → RuleChain → RewriteEngine → SuggestionEngine │
├─────────────────────────────────────────────────────────────┤
│  Layer 2: 规则实现层                                          │
│  DetectDuplicateEqKeys / SubqueryToJoin / EliminateSelectStar │
├─────────────────────────────────────────────────────────────┤
│  Layer 1: 基础设施层（ogsql-parser 提供）                      │
│  AST / Visitor / SchemaMap / SemanticModel / Formatter        │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 核心数据流

```
原始 SQL
   │
   ▼
ogsql-parser (Tokenizer + Parser)
   │
   ▼
Vec<<Statement> + SchemaMap + SemanticModel
   │
   ▼
metamorphosis::RewriteEngine::rewrite(ctx, stmts)
   │
   ├──► 规则 A 匹配？→ 应用 → 产出新 AST
   ├──► 规则 B 匹配？→ 跳过（版本不支持）
   ├──► 规则 C 匹配？→ 产出 Suggestion（Manual 级别）
   │
   ▼
RewriteResult { statements, suggestions, metadata }
   │
   ▼
Formatter → 输出 SQL 文本 + JSON 报告
```

---

## 3. 核心抽象（Core Abstractions）

### 3.1 规则接口（RewriteRule）

所有规则必须实现此 Trait。引擎通过 Trait Object 动态分发，支持运行时注册。

```rust
/// 规则唯一标识，全局唯一
pub trait RewriteRule: Send + Sync + std::fmt::Debug {
    /// 规则 ID，如 "detect-duplicate-eq-keys"
    fn id(&self) -> &'static str;

    /// 人类可读描述
    fn description(&self) -> &'static str;

    /// 分类，用于 UI 分组与权限控制
    fn category(&self) -> RuleCategory;

    /// 默认是否启用
    fn default_enabled(&self) -> bool;

    /// 安全级别：决定引擎如何处理匹配结果
    fn safety_level(&self) -> SafetyLevel;

    /// 适用版本范围，None 表示全版本
    fn version_range(&self) -> Option<(GaussVersion, GaussVersion)>;

    /// 匹配检查：当前语句是否满足规则前提
    fn matches(&self, ctx: &RewriteContext, stmt: &Statement) -> bool;

    /// 执行重写，返回 RewriteAction
    fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Option<RewriteAction>;
}
```

### 3.2 安全级别（SafetyLevel）

```rust
pub enum SafetyLevel {
    /// 语义等价，引擎可自动执行，无需人工确认
    Safe,
    /// 需满足特定前提才等价，引擎执行前需验证前提
    Conditional,
    /// 非语义等价，仅生成建议或探针 SQL，绝不自动替换原语句
    Manual,
}
```

### 3.3 改写动作（RewriteAction）

规则执行后的产出不是简单的 `Statement`，而是带语义标记的动作：

```rust
pub enum RewriteAction {
    /// 语义等价替换：可直接替换原 SQL
    Replace(Box<<Statement>),

    /// 生成衍生 SQL（如数据质量探针），与原 SQL 并存
    Generate {
        stmt: Box<<Statement>,
        purpose: String,          // 如 "候选键重复检测"
        confidence: Confidence,
    },

    /// 仅文本建议，不产出 AST
    Suggest {
        message: String,
        severity: Severity,
    },
}
```

### 3.4 置信度（Confidence）

```rust
pub enum Confidence {
    /// 单表、无子查询、纯常量等值，改写确定无疑
    High,
    /// 穿透了派生表或移除了 EXISTS，结构变化但语义可追踪
    Medium,
    /// 涉及多表 JOIN、动态子查询，结果需人工复核
    Low,
}
```

### 3.5 重写上下文（RewriteContext）

```rust
pub struct RewriteContext<'a> {
    /// 数据库版本，决定规则可用性
    pub version: GaussVersion,

    /// 表结构信息（用于 SELECT * 展开、类型推断）
    pub schema: Option<&'a SchemaMap>,

    /// 语义分析结果（列绑定、作用域）
    pub semantic: Option<&'a SemanticModel>,

    /// 用户配置
    pub config: &'a RewriteConfig,

    /// 当前处理文件名（用于溯源）
    pub source_file: Option<&'a str>,
}

pub struct RewriteConfig {
    /// 显式启用的规则
    pub enabled_rules: HashSet<String>,
    /// 显式禁用的规则
    pub disabled_rules: HashSet<String>,
    /// 单条语句最大重写轮数，防止循环
    pub max_iterations: usize,
    /// 是否保留注释（依赖 ogsql-parser 的 Trivia 支持）
    pub preserve_comments: bool,
    /// 探针 SQL 默认 LIMIT
    pub probe_default_limit: usize,
}
```

---

## 4. 引擎设计（RewriteEngine）

### 4.1 引擎状态机

```
[Init] ──► 加载 RuleRegistry（内置 + 配置 + 插件）
   │
   ▼
[Filter] ──► 按 version / enabled_rules / disabled_rules 过滤可用规则
   │
   ▼
[Match] ──► 遍历语句，对每个规则调用 matches()
   │
   ▼
[Sort] ──► 按 SafetyLevel 排序：Safe → Conditional → Manual
   │
   ▼
[Apply] ──► 依次执行 apply()，Safe/Conditional 直接修改 AST，Manual 进入 Suggestions
   │
   ▼
[Validate] ──► 对 Replace/Generate 的 AST 重新解析验证，确保语法合法
   │
   ▼
[Output] ──► 产出 RewriteResult
```

### 4.2 循环防止机制

```rust
impl RewriteEngine {
    pub fn rewrite(&self, ctx: &RewriteContext, stmts: Vec<<Statement>) -> RewriteResult {
        let mut result = Vec::new();
        let mut suggestions = Vec::new();

        for stmt in stmts {
            let mut current = stmt;
            let mut iteration = 0;
            let mut changed = true;

            while changed && iteration < ctx.config.max_iterations {
                changed = false;
                iteration += 1;

                for rule in self.filtered_rules(ctx) {
                    if rule.safety_level() == SafetyLevel::Manual {
                        // Manual 规则不修改 AST，只收集建议
                        if rule.matches(ctx, &current) {
                            if let Some(action) = rule.apply(ctx, &current) {
                                suggestions.push(Suggestion {
                                    rule_id: rule.id().to_string(),
                                    original_stmt: current.clone(),
                                    action,
                                });
                            }
                        }
                        continue;
                    }

                    if rule.matches(ctx, &current) {
                        if let Some(RewriteAction::Replace(new_stmt)) = rule.apply(ctx, &current) {
                            if self.validate(&new_stmt) {
                                current = *new_stmt;
                                changed = true;
                                break; // 重新从头匹配，优先级高的先执行
                            }
                        }
                    }
                }
            }

            result.push(current);
        }

        RewriteResult {
            statements: result,
            suggestions,
            changed: !suggestions.is_empty() || /* 检测是否有 Safe 替换 */ false,
        }
    }

    fn validate(&self, stmt: &Statement) -> bool {
        let sql = SqlFormatter::new().format_statement(stmt);
        let (stmts, errors) = Parser::parse_sql(&sql);
        !stmts.is_empty() && errors.iter().all(|e| !e.is_fatal())
    }
}
```

### 4.3 规则注册表（RuleRegistry）

支持三种来源，按优先级合并：

```rust
pub struct RuleRegistry {
    builtin: Vec<<Box<dyn RewriteRule>>,
    config: Vec<<Box<dyn RewriteRule>>,   // 从 TOML/YAML 加载的轻量规则
    plugins: Vec<<Box<dyn RewriteRule>>,  // 动态链接库或 WASM 插件（未来）
}

impl RuleRegistry {
    pub fn load_builtin() -> Self {
        Self {
            builtin: vec![
                Box::new(DetectDuplicateEqKeys),
                Box::new(SubqueryToJoin),
                Box::new(PredicatePushdown),
                Box::new(EliminateSelectStar),
                Box::new(NormalizeImplicitCast),
                Box::new(DeduplicateOrderBy),
            ],
            config: Vec::new(),
            plugins: Vec::new(),
        }
    }

    pub fn load_config(path: &Path) -> Result<Vec<<Box<dyn RewriteRule>>, ConfigError> {
        // 加载 TOML 定义的轻量规则（基于 AST 路径匹配）
        // 见第 6 节"配置驱动规则"
    }
}
```

---

## 5. 内置规则详细设计

以 **DetectDuplicateEqKeys**（候选键重复检测）为范例，展示复杂规则的完整实现模式。

### 5.1 规则定位

| 属性 | 值 |
|------|-----|
| ID | `detect-duplicate-eq-keys` |
| 类别 | `RuleCategory::DataQuality` |
| 安全级别 | `SafetyLevel::Manual` |
| 说明 | 从 SELECT/SELECT INTO 语句中提取等值定位条件，生成 GROUP BY 聚合探针，用于检测组合唯一性 |

### 5.2 匹配条件（Matches）

```rust
fn matches(&self, _ctx: &RewriteContext, stmt: &Statement) -> bool {
    let query = match stmt {
        Statement::Select(s) => s,
        Statement::SelectInto(s) => &s.select,
        _ => return false,
    };

    // 1. 找到主表（支持穿透派生表）
    let Some((base_table, _)) = resolve_base_table(&query.from) else {
        return false;
    };

    // 2. 递归收集等值条件
    let mut collector = EqPredicateCollector::new(base_table);
    collector.visit_statement(stmt);

    // 3. 至少有两个 Tier1/Tier2 等值列才构成候选键
    collector.tier1.len() + collector.tier2.len() >= 2
}
```

### 5.3 等值条件分级（Tier System）

```rust
pub struct EqPredicateCollector {
    base_table: ObjectName,
    base_alias: Option<String>,

    /// Tier 1: 基表列 = 外部参数 / 常量 / 绑定变量（高置信度）
    pub tier1: Vec<String>,

    /// Tier 2: 基表列 = 同表关联列（如 EXISTS 子查询中的等值）
    pub tier2: Vec<String>,

    /// Tier 3: 基表列 = 标量子查询 / 动态表达式（不纳入 GROUP BY，保留为 WHERE）
    pub tier3: Vec<Expr>,

    /// 非等值条件：BETWEEN、LIKE、范围、IN 等（保留为 WHERE）
    pub non_eq: Vec<Expr>,
}
```

分级逻辑：

| 右侧表达式类型 | 示例 | 分级 | 原因 |
|-------------|------|------|------|
| `Literal` / `Placeholder` / `Variable` | `= '20250101'` / `= ?` / `= in_date` | Tier 1 | 值固定或外部传入，列本身是稳定维度 |
| 同基表列（关联子查询） | `EXISTS (SELECT 1 FROM t2 WHERE t2.id = t.id AND t2.status = 'A')` | Tier 2 | `t2.status = 'A'` 是等值约束，关联条件 `t2.id = t.id` 仅用于绑定 |
| 标量子查询 | `= (SELECT MAX(date) FROM config)` | Tier 3 | 值动态变化，列不适合作为稳定候选键维度 |
| 函数调用 | `= UPPER(name)` | Tier 3 | 动态计算，不纳入 GROUP BY |

### 5.4 子查询穿透策略

```rust
impl Visitor for EqPredicateCollector {
    fn visit_expr(&mut self, expr: &Expr) -> VisitorResult {
        match expr {
            // 穿透派生表：FROM (SELECT * FROM base WHERE ...) t
            Expr::Subquery(sub) if self.is_derived_table(sub) => {
                self.visit_subquery(sub)?;
            }

            // EXISTS 同表关联：提取子查询内等值，移除关联条件
            Expr::Exists(sub) => {
                if self.is_same_base_table_in_subquery(sub) {
                    self.extract_eq_from_subquery(sub, /* include_correlated */ false);
                }
            }

            // 标量子查询：保留为 Tier 3
            Expr::Subquery(sub) if self.is_scalar_subquery(sub) => {
                self.tier3.push(expr.clone());
            }

            _ => walk_expr(self, expr)?,
        }
        Ok(())
    }
}
```

### 5.5 改写动作（Apply）

```rust
fn apply(&self, ctx: &RewriteContext, stmt: &Statement) -> Option<RewriteAction> {
    let query = extract_query(stmt)?;
    let (base_table, _) = resolve_base_table(&query.from)?;

    let mut collector = EqPredicateCollector::new(base_table.clone());
    collector.visit_statement(stmt);

    let group_cols: Vec<String> = collector.tier1.into_iter()
        .chain(collector.tier2.into_iter())
        .collect::<HashSet<_>>()  // 去重
        .into_iter()
        .collect();

    if group_cols.is_empty() {
        return None;
    }

    // 构建探针 SQL
    let probe_stmt = build_probe_statement(
        base_table,
        &group_cols,
        collector.non_eq,
        collector.tier3,
        ctx.config.probe_default_limit,
    );

    Some(RewriteAction::Generate {
        stmt: Box::new(probe_stmt),
        purpose: "候选键重复检测：验证等值定位列的组合唯一性".to_string(),
        confidence: if collector.has_subquery {
            Confidence::Medium
        } else {
            Confidence::High
        },
    })
}
```

### 5.6 探针 SQL 构建模板

```rust
fn build_probe_statement(
    table: ObjectName,
    group_cols: &[String],
    non_eq_where: Vec<Expr>,
    tier3_where: Vec<Expr>,
    limit: usize,
) -> Statement {
    // SELECT col1, col2, ..., count(1) AS cnt
    let mut projection = Projection::List(
        group_cols.iter()
            .map(|c| Expr::ColumnRef(ObjectName::from(c.clone())))
            .collect()
    );
    projection.add_item(count_star_as_cnt());

    // WHERE = non_eq + tier3
    let where_clause = merge_and_conditions(non_eq_where.into_iter().chain(tier3_where));

    // GROUP BY 所有候选键列
    let group_by = Some(
        group_cols.iter()
            .map(|c| Expr::ColumnRef(ObjectName::from(c.clone())))
            .collect()
    );

    // HAVING count(1) > 1
    let having = Some(Expr::BinaryOp {
        op: BinaryOp::Gt,
        left: Box::new(count_star_expr()),
        right: Box::new(Expr::Literal(Literal::Integer(1))),
    });

    // ORDER BY cnt DESC
    let order_by = Some(vec![OrderByExpr {
        expr: Expr::ColumnRef(ObjectName::from("cnt")),
        direction: OrderDirection::Desc,
    }]);

    // LIMIT N
    let limit = Some(Expr::Literal(Literal::Integer(limit as i64)));

    Statement::Select(SelectStatement {
        projection,
        from: Some(FromClause::Table { name: table, alias: None }),
        where_clause,
        group_by,
        having,
        order_by,
        limit,
        ..Default::default()
    })
}
```

---

## 6. 扩展机制

### 6.1 配置驱动规则（轻量扩展）

对于简单的模式匹配规则，无需编写 Rust 代码，通过 TOML 配置即可注册。

```toml
# metamorphosis.toml
[[rules]]
id = "no-select-star-on-prod"
category = "Style"
safety = "Safe"

[rules.match]
kind = "SelectStatement"
projection.has_wildcard = true

[rules.condition]
table_in = ["orders", "users", "transactions"]

[rules.action]
type = "Replace"
# 使用模板语法引用 SchemaMap 中的列
projection = "{{schema.columns}}"
```

引擎在启动时将 TOML 规则编译为轻量的 `TemplateRule` 实现，动态加入注册表。

### 6.2 插件规则（重量级扩展）

未来支持 WASM 或动态库加载：

```rust
pub trait PluginRule: RewriteRule {
    fn load(path: &Path) -> Result<<Box<dyn RewriteRule>, PluginError>;
}
```

### 6.3 规则链编排

支持规则间的依赖与互斥：

```rust
pub struct RuleChain {
    pub sequence: Vec<String>,  // 如 ["predicate-pushdown", "subquery-to-join"]
    pub mutex: Vec<(String, String)>, // 互斥规则对
}
```

---

## 7. 与 ogsql-parser 的集成契约

### 7.1 输入契约

Metamorphosis 不直接解析 SQL，而是接收 `ogsql-parser` 的产出：

```rust
pub struct ParserOutput {
    pub statements: Vec<<Statement>,
    pub comments: Vec<CommentInfo>,      // 未来 Trivia 支持
    pub errors: Vec<ParserError>,
}
```

### 7.2 输出契约

```rust
pub struct RewriteResult {
    /// 重写后的语句（Safe / Conditional 级别）
    pub statements: Vec<<Statement>,

    /// Manual 级别的建议（需人工确认）
    pub suggestions: Vec<Suggestion>,

    /// 是否发生了任何改写
    pub changed: bool,

    /// 元数据：规则触发次数、置信度分布
    pub metadata: RewriteMetadata,
}

pub struct Suggestion {
    pub rule_id: String,
    pub rule_description: String,
    pub original_stmt: Statement,
    pub action: RewriteAction,
    pub confidence: Confidence,
    pub notes: Vec<String>,         // 人工审查提示
    pub source_location: Option<SourceLocation>,
}
```

### 7.3 版本联动

```rust
impl RewriteContext<'_> {
    pub fn is_rule_applicable(&self, rule: &dyn RewriteRule) -> bool {
        let version_ok = rule.version_range().map_or(true, |(min, max)| {
            self.version >= min && self.version <= max
        });
        version_ok && self.config.is_enabled(rule.id())
    }
}
```

---

## 8. 测试策略

### 8.1 测试金字塔

```
┌─────────────────────────────────────┐
│  集成测试（端到端 SQL → 探针 SQL）   │  20%
│  tests/integration/                 │
├─────────────────────────────────────┤
│  规则单元测试（单规则匹配/应用）      │  50%
│  rules/detect_duplicate_eq_keys/    │
├─────────────────────────────────────┤
│  引擎单元测试（注册表/排序/循环防止）  │  30%
│  core/engine_tests.rs               │
└─────────────────────────────────────┘
```

### 8.2 规则测试 DSL

为降低测试编写成本，提供声明式测试宏：

```rust
#[rule_test]
fn test_detect_duplicate_with_derived_table() {
    input = r#"
        SELECT t.trade_code INTO v_trade_code
          FROM (SELECT * FROM dat_clr_cash_dtl
                 WHERE account_date = :date AND account_seqno = :seq) t
         WHERE t.account_id = :id
    "#;

    expect_generate = r#"
        SELECT account_date, account_seqno, account_id, count(1) AS cnt
          FROM dat_clr_cash_dtl
         GROUP BY account_date, account_seqno, account_id
        HAVING count(1) > 1
         ORDER BY cnt DESC
         LIMIT 10
    "#;

    confidence = "Medium";
    notes_contains = ["穿透了 1 层派生表"];
}
```

### 8.3 版本矩阵测试

```rust
#[test_matrix]
fn test_duplicate_detect_version_matrix() {
    versions = [V2_0, V3_0, V5_0];
    sql = "SELECT * INTO v FROM t WHERE a = 1 AND b = 2";
    expect_all_versions = "生成探针 SQL";
}
```

---

## 9. CLI 设计

```bash
# 基本重写（Safe + Conditional 级别自动执行）
metamorphosis rewrite query.sql --version 5.0 --schema schema.json

# 建议模式（包含 Manual 级别建议）
metamorphosis suggest query.sql --version 5.0 --schema schema.json

# 指定规则子集
metamorphosis rewrite query.sql --rules detect-duplicate-eq-keys,subquery-to-join

# 输出 JSON 报告（供 CI/CD 消费）
metamorphosis suggest query.sql -o json > report.json

# 与 ogexplain-analyzer 联动
ogexplain analyze plan.txt --rewrite --version 5.0
```

---

## 10. 演进路线

| 阶段 | 目标 | 关键交付 |
|------|------|---------|
| **MVP** | 引擎骨架 + 第一条规则 | `RewriteEngine` + `DetectDuplicateEqKeys` + CLI `rewrite` |
| **v0.2** | 规则库扩展 | `SubqueryToJoin` + `PredicatePushdown` + `EliminateSelectStar` |
| **v0.3** | 配置驱动 | TOML 规则配置 + `RuleRegistry` 动态加载 |
| **v0.4** | 数据质量集成 | 与 `ogexplain-analyzer` 的 `--rewrite` 联动 |
| **v0.5** | 语义深化 | 依赖 `SemanticModel` 的 `NormalizeImplicitCast` |
| **v1.0** | 生产就绪 | 完整测试覆盖 + 文档 + 性能基准 |

---

## 11. 附录：术语表

| 术语 | 定义 |
|------|------|
| **探针 SQL** | 不替代原业务逻辑，用于检测数据质量、验证约束的衍生查询 |
| **Tier 分级** | 等值条件按右侧表达式稳定性分为 Tier1/2/3，决定是否纳入 GROUP BY |
| **穿透** | 递归展开派生表/子查询，找到底层基表和条件 |
| **置信度** | 改写结果的可信程度，High=无子查询，Medium=有子查询穿透，Low=多表动态 |
| **Manual 级别** | 非语义等价改写，仅生成建议，绝不自动执行 |

---

**文档结束**
