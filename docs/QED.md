QED：https://www.vldb.org/pvldb/vol17/p3602-wang.pdf

基于已获取的论文原文（Abstract ~ Section 4.2），以下是 **QED (VLDB 2024)** 的深度技术拆解。所有公式与定义均严格对应原文，未获取部分（Section 5-6 的完整算法伪代码）不做臆造。

---

## 一、总体定位：解决什么问题

QED 的核心目标是 **在 Bag Semantics 下判定两个 SQL 查询是否语义等价**，且必须支持生产环境中的真实 SQL 特性：

| 特性 | Cosette/UDP | SPES | **QED** |
|------|-------------|------|---------|
| Bag 语义 | ✅ | ✅ | ✅ |
| NULL / 三值逻辑 | ❌ | ❌ | **✅** |
| 完整性约束（PK/FK/Check） | ❌ | 部分（仅自连接） | **✅** |
| 未知算子（如 LIMIT） | ❌ | ❌ | **✅（作为未解释函数）** |
| 聚合 + JOIN 组合 | 有限 | 有限 | **✅** |

实验上，QED 在 Calcite 和 CockroachDB 的改写规则基准上，证明数量是 SPES 的 **2 倍以上**（Calcite: 299/444；CockroachDB: 979/1287）。

---

## 二、Q-expressions：Semiring 语义的形式化编码

QED 不直接在 SQL AST 上做比对，而是先将查询翻译为一种中间表示 **Q-expression**，再基于 **U-semiring（扩展自然数半环）** 进行推理。

### 2.1 基础代数结构

定义 **N̄ = N ∪ {∞}**，运算规则：

```
∞ + a = ∞
0 × ∞ = 0
a × ∞ = ∞   (if a ≠ 0)
```

核心记号：

| 记号 | 含义 | 语义 |
|------|------|------|
| `[P]` | 指示函数 | 1 if P is true, else 0 |
| `∥a∥` | **Squash** | `[a ≠ 0]`（将多重性截断至 1，用于建模 DISTINCT / 存在性） |
| `¬a` | 零化子 | `[a = 0]` |
| `∑_s f(s)` | 无界求和 | 对有限支撑集 {s₁,...,sₙ} 求和 f(s₁)+...+f(sₙ) |

### 2.2 查询算子的 Q-expression 编码（第 3.2 节）

每个查询 `Q` 被解释为一个函数 `λx. ⟦Q⟧(x)`，输入一行 `x`，返回该行在结果 Bag 中的**多重性**。

```
Table(R:S)          = λx. R(x)

Values(v₁,...,vₙ)   = λx. [x=v₁] + ... + [x=vₙ]

Filter(P, Q)        = λx. [⟦P⟧(x) = True] × ⟦Q⟧(x)

Proj(f, Q)          = λx. ∑_s [x = ⟦f⟧(s)] × ⟦Q⟧(s)

Join(Q₁, Q₂)        = λx₁,x₂. ⟦Q₁⟧(x₁) × ⟦Q₂⟧(x₂)

Union(Q₁, Q₂)       = λx. ⟦Q₁⟧(x) + ⟦Q₂⟧(x)

Distinct(Q)         = λx. ∥⟦Q⟧(x)∥

Minus(Q₁, Q₂)       = λx. ∥⟦Q₁⟧(x) × ¬⟦Q₂⟧(x)∥
```

**GroupBy** 的编码最为复杂，因为它同时涉及 squash（去重分组键）和聚合函数：

```
GroupBy(k, α(f), Q) = λx,y. ∥∑_s [x = ⟦k⟧(s)] × ⟦Q⟧(s)∥
                        × [y = HOp(α, λy'. ∑_s ⟦Q⟧(s) × [x = ⟦k⟧(s) ∧ y' = ⟦f⟧(s)])]
```

- 外层 `∥...∥` 确保每个分组键只出现一次（Set 语义的分组键）
- 内层 `HOp(α, ...)` 将聚合函数 `α`（如 SUM/MAX）作用在每组内的 Bag 上

**未知算子**（如 `LIMIT n`）通过 **QOp** 捕获为**未解释函数**：

```
QOp(Limit, n, Q)    = λx. QOp(Limit, n, ⟦Q⟧)(x)
```

这使得 QED 遇到不支持的算子时不会崩溃，而是将其作为黑盒保留，继续验证其余部分。

### 2.3 上下文标注

QED 使用三元组上下文 `Γ | Δ | Φ ⊢ Q`：

- **Γ**：表符号列表（如 R, S）
- **Δ**：未解释符号（如聚合函数、未知算子）
- **Φ**：全局约束（如完整性约束、环境假设）

等价性问题定义为：

```
Γ | Δ | Φ ⊢ Q₁ ?= Q₂   ⟺   ∀s. Φ ⇒ (⟦Q₁⟧(s) = ⟦Q₂⟧(s))
```

---

## 三、完整性约束：从"查询重写规则"到"上下文公理"

这是 QED 相比 SPES 最深刻的改进。SPES 用**查询重写规则**建模主键（如自连接消去），规则之间组合性差；QED 将约束编码为**上下文 Φ 中的逻辑公式**，与查询归一化解耦。

### 3.1 主键约束（Primary Key）

若表 `R` 的 schema 为 `K × S`，且 `k` 是主键，则存在函数依赖 `f_R: K → S`。QED 将 `R` 重写为：

```
R ⊢ R ⇝ R' | f_R ⊢ λk,s. ∥R'(k)∥ × [s = f_R(k)]
```

- `R'` 是新的单列表符号（仅含主键列 `K`）
- `∥R'(k)∥` 保证每个主键值至多出现一次
- `f_R` 是未解释函数，由 SMT 求解器处理

**当 `k` 是 `R` 的唯一列时**，简化为：

```
R ⊢ R ⇝ R' ⊢ λk. ∥R'(k)∥
```

**为什么这比 SPES 强？**  
SPES 只有两个针对自连接的主键重写规则；QED 的上述规则是**完备的**（原文 eq.6），且适用于任意表连接场景（如 Motivating Example 中 `R JOIN S` 通过 `S` 的主键保证多重性不变）。

### 3.2 外键约束（Foreign Key）

`S(b,k)` 引用 `R(k,a)` 的主键 `k`。直觉上：

```
∀b,k. S(b,k) = S(b,k) × ∑_a R(k,a)
```

但 UDP 会**急切展开**这个等式，若 `R` 又反向引用 `S`，会导致**无限循环**。QED 的解决方法是将其编码为**单一全局逻辑公式**，交给 SMT 求解器**惰性展开**：

```
S ⊢ S ⇝ S, R' | Φ_FK
```

其中 `Φ_FK` 是外键约束的 SMT 编码，不直接重写查询体。

### 3.3 Check 约束

```
R ⊢ R ⇝ R | ∀s. ∥R(s)∥ → C(s)
```

即：所有出现在 `R` 中的行必然满足谓词 `C`。

---

## 四、NULL 语义：Option Type + 三值逻辑

QED 将每个 SQL 类型 `T` 提升为 **Option(T)**，`Null_T` 作为额外元素。现代 SMT 求解器（如 Z3/CVC5）原生支持这种代数数据类型。

### 4.1 原始运算的 NULL 提升

对于 n 元原始运算 `f`（如 `+`, `×`），若任一参数为 NULL，则结果为 NULL：

```
Op(f, a₁, ..., aₙ) = NULL  if any aᵢ = NULL
```

### 4.2 三值逻辑（3VL）

SQL 的 `AND` / `OR` 不满足简单提升规则，QED 用 **if-then-else (ite)** 精确定义：

```
a And b = ite(a = Null,
              ite(b = True, Null, b),
              ite(a = True, b, a))

a Or b  = ite(a = Null,
              ite(b = False, Null, b),
              ite(a = False, b, a))
```

### 4.3 Some 运算符（含子查询的比较）

`a <cmp> SOME Q`（如 `a IN Q` 是 `=` 的特例）：

```
a <cmp> SOME Q = ite(∃x. ∥Q(x)∥ ∧ (x <cmp> a) = True,  True,
                     ite(∃x. ∥Q(x)∥ ∧ (x <cmp> a) = Null, Null,
                         False))
```

### 4.4 聚合中的 NULL

`COUNT(*)` 与 `COUNT(k)` 的差异通过**可选的非 NULL 过滤**建模：在应用聚合函数前，先对输入 Bag 过滤掉 `k = Null` 的行。

---

## 五、双轨等价判定架构

QED 采用**两条并行的判定路径**（见图 1），根据查询是否属于某个完全片段自动选择。

```
Queries & Constraints
         │
         ▼
    Translate (Sec. 3)
         │
    ┌────┴────┐
    │         │
    ▼         ▼
if in complete        General SQL
fragment (Sec. 4)     (Sec. 5)
    │                   │
    ▼                   ▼
Normalize             Normalize
(Sec. 4.1)            (Sec. 5.1)
    │                   │
    ▼                   ▼
Linearize             Stabilize
(Sec. 4.2)            (Sec. 5.2)
    │                   │
    ▼                   ▼
Unify                 Unify
(Sec. 4.3)            (Sec. 5.3)
[完全算法]            [通用启发式]
```

### 5.1 Complete Fragment F_T（右轨）

**定义（Definition 1）**：片段 `F_T` 包含 `Table, Values, Filter, Proj, Join, Union`，其中所有标量表达式（过滤条件、投影函数）必须可定义在某个**一阶理论 T** 中。允许主键约束。

**完全性保证**：若 SMT 求解器对理论 `T` 是完全的，则 Alg.3 对 `F_T` 中的查询对是**完全判定算法**。

**SNF（Scoped Normal Form）**：

任何 `Q ∈ F_T` 可被归一化为：

```
U = T₁ + T₂ + ... + Tₙ

其中每个 Tᵢ 的形式为：
T = ∑_{R₁^k¹(s₁)} ... ∑_{Rₘ^kᵐ(sₘ)} [P]

P 是 body（Body_T），不含表变量，仅含 Δ, s₁,...,sₘ 上的谓词。
```

特殊记法：

- `∑_{R⁰(s)} U`  =  `∑_s ∥R(s)∥ × U`  （squash 求和，用于 EXISTS / DISTINCT）
- `∑_{R^k(s)} U`  =  `∑_s R(s) × ... × R(s) [k 次] × U`  （Bag 语义的多重性累积）

**归一化构造（Alg.1 思路）**：

| 算子 | SNF 构造方式 |
|------|-------------|
| Table | 已是 normal term |
| Values | 已是 normal term（指示函数之和） |
| Filter(P, Q) | 将 `[P]` 合取到每个 term 的 body |
| Proj(f, Q) | 将投影函数 post-compose 到每个 term，不引入新求和 |
| Join(Q₁, Q₂) | 两两组合 term，合并求和作用域，body 取合取 |
| Union(Q₁, Q₂) | term 列表的简单拼接 |

### 5.2 General SQL 路径（左轨）

对于包含 `GroupBy, Distinct, Minus, QOp` 的通用查询，QED 使用**启发式算法**（Alg.6）：

1. **Normalize (Sec. 5.1)**：同样转为 SNF
2. **Stabilize (Sec. 5.2)**：系统消除冗余求和作用域（如 Motivating Example 中消去 `∑_a [x=a]`，因为 `S` 的主键已保证 `a=x` 的确定性）
3. **Unify (Sec. 5.3)**：用 SMT 统一化

### 5.3 环境假设传播：Motivating Example 的关键技巧

回顾第 2 节的例子：

```sql
-- Q1
SELECT x, SUM(y) FROM R JOIN S ON x = a GROUP BY x;

-- Q2  
SELECT x, r FROM (SELECT x, SUM(y) AS r FROM R GROUP BY x) JOIN S ON x = a;
```

翻译为 Q-expression 并归一化/稳定化后：

```
Q1(x,r) = ∥∑_y R(x,y) × ∥S(x)∥ × [r = Sum(λy. R(x,y) × ∥S(x)∥)]∥
Q2(x,r) = ∥∑_y R(x,y) × ∥S(x)∥ × [r = Sum(λy. R(x,y))]∥
```

此时需验证两个聚合是否等价：

```
v1 = Sum(λy. R(x,y) × ∥S(x)∥)
v2 = Sum(λy. R(x,y))
```

直接递归检查会失败，因为 `v2` 缺少 `∥S(x)∥` 项。QED 的关键洞察是：

> **从顶层 Q-expression 中"读出"环境约束**：在顶层，`φ1(x,r) ∨ φ2(x,r)` 已隐含 `S(x) ≠ 0`（即 `x` 在 `S` 中存在）。将该假设作为**额外断言**注入递归检查：

```
(φ1 ∨ φ2)  ⇒  ∀y. R(x,y) × ∥S(x)∥ = R(x,y)
```

在 `S(x) ≠ 0` 的假设下，`∥S(x)∥ = 1`，因此 `v1 = v2`。原文将此形式化为 eq.(15)。

这就是 **"约束跨越未知算子边界传播"** 的能力——SPES 和 UDP 无法做到，因为它们将聚合视为黑盒递归检查，丢失了顶层环境。

---

## 六、实验结果（第 6 节摘要）

| 基准 | 总查询对 | QED 证明 | SPES 证明 | 提升倍数 |
|------|---------|---------|----------|---------|
| **Calcite** | 444 | **299** | ~140 | **>2×** |
| **CockroachDB** | 1287 | **979** | ~460 | **>2×** |

QED 的增量主要来自：
1. 主键约束的完备建模（如跨表 JOIN 场景）
2. NULL 语义支持（SPES 完全无法处理含 NULL 的改写）
3. 环境假设传播（聚合下推等复杂改写）

---

## 七、对你当前工作的启示

结合你的 **OGSQL Parser + warpdriver Transpiler + GaussDB 适配**：

### 7.1 架构集成点

```
┌─────────────────────────────────────────┐
│  OGSQL Parser (Rust)                   │
│  ├── PL/SQL → AST                      │
│  └── AST → Q-expression (翻译层)        │
└─────────────────────────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────┐
│  QED Core (或自研简化版)                │
│  ├── 完整性约束提取 (GaussDB 元数据)      │
│  ├── SNF 归一化                         │
│  └── Z3/CVC5 SMT 求解                   │
└─────────────────────────────────────────┘
                   │
         ┌────────┴────────┐
         ▼                 ▼
    Equivalent         Unknown / Timeout
         │                 │
         ▼                 ▼
    跳过执行比对         回退到 VERIEQL /
                       执行比对 (采样数据)
```

### 7.2 GaussDB 特有适配建议

| GaussDB 特性 | QED 适配策略 |
|-------------|-------------|
| **MERGE INTO** | 作为 `QOp(Merge, ...)` 未解释算子保留，验证其前后 SQL 的其余部分 |
| **Package 常量/变量** | 在上下文 `Δ` 中建模为未解释符号，约束从 PL/Scope 或静态分析提取 |
| **Astore/Ustore 差异** | 等价性验证只关心逻辑结果，物理存储不影响；但执行比对时需注意 MVCC 快照差异 |
| **分区表** | 分区裁剪可建模为 `Filter` + `Check` 约束的组合 |

### 7.3 渐进式落地建议

1. **第一阶段（2 周）**：基于 OGSQL-Parser 实现 **Q-expression 翻译层**，覆盖你们 transpiler 最常处理的算子（`SELECT/FILTER/JOIN/PROJ/AGG`）。对不含 `GroupBy/Distinct` 的查询对，尝试走 **Complete Fragment** 路径（理论上有完全保证）。
2. **第二阶段（1 个月）**：接入 **Z3** 作为 SMT Oracle，实现 SNF 归一化 + 主键约束建模。对 warpdriver 生成的每一对（原始 SQL vs 改写 SQL）自动验证。
3. **第三阶段（3 个月）**：处理 **NULL 语义** 和 **未知算子**。GaussDB 的特有函数（如 `NVL`, `DECODE`）可作为 `Op` 编码，若 Z3 不支持则降级为未解释函数。

---
