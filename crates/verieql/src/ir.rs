//! VeriEql Intermediate Representation — relational algebra tree.

/// A relational algebra expression (a "table" in VeriEQL terminology).
#[derive(Debug, Clone)]
pub enum Relation {
    BaseTable {
        name: String,
        alias: Option<String>,
        columns: Vec<String>,
        tuple_count: usize,
    },
    Filter {
        input: Box<Relation>,
        predicate: Expr,
    },
    Project {
        input: Box<Relation>,
        exprs: Vec<ProjectExpr>,
        distinct: bool,
    },
    Join {
        left: Box<Relation>,
        right: Box<Relation>,
        join_type: JoinType,
        condition: Option<Expr>,
    },
    GroupBy {
        input: Box<Relation>,
        keys: Vec<Expr>,
        aggregates: Vec<AggregateExpr>,
        having: Option<Expr>,
    },
    OrderBy {
        input: Box<Relation>,
        items: Vec<OrderByItem>,
        limit: Option<Expr>,
        offset: Option<Expr>,
    },
    Union {
        left: Box<Relation>,
        right: Box<Relation>,
        all: bool,
    },
    Intersect {
        left: Box<Relation>,
        right: Box<Relation>,
        all: bool,
    },
    Except {
        left: Box<Relation>,
        right: Box<Relation>,
        all: bool,
    },
    Distinct {
        input: Box<Relation>,
    },
    Values {
        rows: Vec<Vec<Expr>>,
    },
    Empty,
}

/// A projected expression in SELECT: column reference or aggregate.
#[derive(Debug, Clone)]
pub enum ProjectExpr {
    Column(Expr),
    Aggregate(AggregateExpr),
}

/// An aggregate function call.
#[derive(Debug, Clone)]
pub struct AggregateExpr {
    pub func: AggFunc,
    pub arg: Option<Expr>,
    pub distinct: bool,
    pub alias: Option<String>,
}

#[derive(Debug, Clone)]
pub enum AggFunc {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

/// A scalar expression.
#[derive(Debug, Clone)]
pub enum Expr {
    ColumnRef {
        table: Option<String>,
        column: String,
    },
    Literal(ExprValue),
    BinaryOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Case {
        operand: Option<Box<Expr>>,
        whens: Vec<(Expr, Expr)>,
        else_expr: Option<Box<Expr>>,
    },
    IsNull {
        expr: Box<Expr>,
        negated: bool,
    },
    InList {
        expr: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    InSubquery {
        expr: Box<Expr>,
        subquery: Box<Relation>,
        negated: bool,
    },
    Exists(Box<Relation>),
    ScalarSubquery(Box<Relation>),
    Between {
        expr: Box<Expr>,
        low: Box<Expr>,
        high: Box<Expr>,
        negated: bool,
    },
    Like {
        expr: Box<Expr>,
        pattern: Box<Expr>,
        negated: bool,
    },
    FunctionCall {
        name: String,
        args: Vec<Expr>,
    },
    SqlNull,
    Star,
}

#[derive(Debug, Clone)]
pub enum ExprValue {
    Integer(i64),
    Float(f64),
    String(String),
    Boolean(bool),
}

#[derive(Debug, Clone)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    Lt,
    Gt,
    Lte,
    Gte,
    And,
    Or,
    Concat,
}

#[derive(Debug, Clone)]
pub enum UnaryOp {
    Not,
    Neg,
}

#[derive(Debug, Clone)]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
}

#[derive(Debug, Clone)]
pub struct OrderByItem {
    pub expr: Expr,
    pub asc: bool,
    pub nulls_first: Option<bool>,
}
