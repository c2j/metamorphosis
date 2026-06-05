//! QED Intermediate Representation types.
//!
//! These types map directly to the QED prover's JSON input format.
//! Serialize with `serde_json` to produce valid prover input.

use serde::{Deserialize, Serialize};

/// Top-level input to the QED prover.
///
/// Contains schema definitions and exactly two query relations (source and rewritten)
/// to be verified for equivalence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QedInput {
    /// Table schemas available to the queries.
    pub schemas: Vec<QedSchema>,
    /// Exactly two relations: the original query and the rewritten query.
    pub queries: [QedRelation; 2],
    /// Human-readable description of the verification task.
    pub help: String,
}

/// Schema definition for a single table.
///
/// Describes column names, types, primary key, nullability, and CHECK constraints
/// for a table referenced in the queries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QedSchema {
    /// Table name.
    pub name: String,
    /// SQL type strings, one per column (parallel to `fields`).
    pub types: Vec<String>,
    /// 0-based column indices forming the primary key. Empty means no PK.
    pub key: Vec<usize>,
    /// Nullability flags, one per column (parallel to `fields`).
    pub nullable: Vec<bool>,
    /// CHECK constraint expressions. Serialized only when non-empty.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub guaranteed: Vec<String>,
    /// Column names in definition order.
    pub fields: Vec<String>,
}

/// Recursive relation tree representing a query plan.
///
/// Uses `#[serde(tag = "type")]` for the tagged enum format expected by
/// the QED prover: `{"type": "Scan", "table": "R", "fields": [0, 1]}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum QedRelation {
    /// Full or partial table scan. Empty `fields` means all columns.
    Scan { table: String, fields: Vec<usize> },

    /// Filter rows by a condition.
    Filter {
        condition: QedExpr,
        input: Box<QedRelation>,
    },

    /// Project a subset of expressions from the input.
    Project {
        exprs: Vec<QedExpr>,
        input: Box<QedRelation>,
    },

    /// Join two relations. `condition: None` means cross join.
    Join {
        left: Box<QedRelation>,
        right: Box<QedRelation>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        condition: Option<QedExpr>,
    },

    /// Set union of two relations.
    Union {
        left: Box<QedRelation>,
        right: Box<QedRelation>,
    },

    /// Set intersection of two relations.
    Intersect {
        left: Box<QedRelation>,
        right: Box<QedRelation>,
    },

    /// Set difference (left minus right).
    Except {
        left: Box<QedRelation>,
        right: Box<QedRelation>,
    },

    /// Eliminate duplicate rows.
    Distinct { input: Box<QedRelation> },

    /// Inline value constructor (`VALUES (...)`).
    Values { rows: Vec<Vec<QedExpr>> },

    /// Grouped aggregation.
    Aggregate {
        /// 0-based column indices for the GROUP BY keys.
        keys: Vec<usize>,
        /// Aggregate function calls.
        aggs: Vec<QedAggCall>,
        input: Box<QedRelation>,
    },

    /// Uninterpreted operator (LIMIT, ORDER BY, GaussDB-specific functions).
    QOp {
        name: String,
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        args: Vec<QedExpr>,
        input: Box<QedRelation>,
    },
}

/// Expression tree used in filters, projections, and join conditions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum QedExpr {
    /// Reference to a column by 0-based index.
    ColumnRef { index: usize },

    /// Typed literal value.
    Literal { value: QedValue },

    /// Binary operator: `"eq"`, `"gt"`, `"lt"`, `"and"`, `"or"`, `"add"`, `"mul"`, etc.
    BinOp {
        op: String,
        left: Box<QedExpr>,
        right: Box<QedExpr>,
    },

    /// Unary operator: `"not"`, `"neg"`.
    UnOp { op: String, expr: Box<QedExpr> },

    /// Interpreted or uninterpreted function call.
    Function {
        name: String,
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        args: Vec<QedExpr>,
    },

    /// SQL NULL literal.
    Null,

    /// Quantified comparison: `SOME`, `ALL`, `EXISTS` subqueries.
    Quantified {
        /// Comparison operator: `"eq"`, `"gt"`, etc.
        cmp: String,
        /// Quantifier: `"some"`, `"all"`, `"exists"`.
        quantifier: String,
        subquery: Box<QedRelation>,
    },
}

/// Aggregate function call within an [`QedRelation::Aggregate`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QedAggCall {
    /// Aggregate function name: `"sum"`, `"count"`, `"max"`, `"min"`, `"avg"`.
    pub func: String,
    /// Argument to the aggregate function.
    pub arg: QedAggArg,
    /// Whether the aggregate uses `DISTINCT`.
    pub distinct: bool,
}

/// Argument to an aggregate function call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum QedAggArg {
    /// `COUNT(*)` — no specific column argument.
    Star,
    /// Aggregate over a specific expression.
    Expr(QedExpr),
}

/// Typed literal value used in expressions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum QedValue {
    /// Integer literal.
    Integer { value: i64 },
    /// Floating-point literal.
    Float { value: f64 },
    /// String literal.
    String { value: String },
    /// Boolean literal.
    Boolean { value: bool },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_simple_scan() {
        let scan = QedRelation::Scan {
            table: "users".to_string(),
            fields: vec![0, 1],
        };
        let json = serde_json::to_string(&scan).unwrap();
        assert!(json.contains(r#""type":"Scan""#));
        assert!(json.contains(r#""table":"users""#));
        assert!(json.contains(r#""fields":[0,1]"#));
    }

    #[test]
    fn test_serialize_filter_project() {
        let scan = QedRelation::Scan {
            table: "orders".to_string(),
            fields: vec![0, 1, 2],
        };
        let filter = QedRelation::Filter {
            condition: QedExpr::BinOp {
                op: "gt".to_string(),
                left: Box::new(QedExpr::ColumnRef { index: 1 }),
                right: Box::new(QedExpr::Literal {
                    value: QedValue::Integer { value: 100 },
                }),
            },
            input: Box::new(scan),
        };
        let project = QedRelation::Project {
            exprs: vec![
                QedExpr::ColumnRef { index: 0 },
                QedExpr::ColumnRef { index: 2 },
            ],
            input: Box::new(filter),
        };

        let json = serde_json::to_string_pretty(&project).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["type"], "Project");
        assert_eq!(parsed["input"]["type"], "Filter");
        assert_eq!(parsed["input"]["input"]["type"], "Scan");
        assert_eq!(parsed["input"]["input"]["table"], "orders");
        assert_eq!(parsed["input"]["condition"]["type"], "BinOp");
        assert_eq!(parsed["input"]["condition"]["op"], "gt");
    }

    #[test]
    fn test_serialize_aggregate() {
        let scan = QedRelation::Scan {
            table: "sales".to_string(),
            fields: vec![0, 1, 2],
        };
        let agg = QedRelation::Aggregate {
            keys: vec![0],
            aggs: vec![
                QedAggCall {
                    func: "sum".to_string(),
                    arg: QedAggArg::Expr(QedExpr::ColumnRef { index: 2 }),
                    distinct: false,
                },
                QedAggCall {
                    func: "count".to_string(),
                    arg: QedAggArg::Star,
                    distinct: true,
                },
            ],
            input: Box::new(scan),
        };

        let json = serde_json::to_string_pretty(&agg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["type"], "Aggregate");
        assert_eq!(parsed["keys"], serde_json::json!([0]));
        assert_eq!(parsed["aggs"][0]["func"], "sum");
        assert_eq!(parsed["aggs"][0]["arg"]["Expr"]["type"], "ColumnRef");
        assert_eq!(parsed["aggs"][0]["arg"]["Expr"]["index"], 2);
        assert_eq!(parsed["aggs"][0]["distinct"], false);
        assert_eq!(parsed["aggs"][1]["func"], "count");
        assert_eq!(parsed["aggs"][1]["arg"], "Star");
        assert_eq!(parsed["aggs"][1]["distinct"], true);
    }

    #[test]
    fn test_roundtrip() {
        let input = QedInput {
            schemas: vec![QedSchema {
                name: "R".to_string(),
                types: vec!["integer".to_string(), "integer".to_string()],
                key: vec![0],
                nullable: vec![false, true],
                guaranteed: vec!["x > 0".to_string()],
                fields: vec!["x".to_string(), "y".to_string()],
            }],
            queries: [
                QedRelation::Scan {
                    table: "R".to_string(),
                    fields: vec![0, 1],
                },
                QedRelation::Filter {
                    condition: QedExpr::BinOp {
                        op: "gt".to_string(),
                        left: Box::new(QedExpr::ColumnRef { index: 0 }),
                        right: Box::new(QedExpr::Literal {
                            value: QedValue::Integer { value: 0 },
                        }),
                    },
                    input: Box::new(QedRelation::Scan {
                        table: "R".to_string(),
                        fields: vec![0, 1],
                    }),
                },
            ],
            help: "test equivalence".to_string(),
        };

        let json = serde_json::to_string(&input).unwrap();
        let deserialized: QedInput = serde_json::from_str(&json).unwrap();
        assert_eq!(input, deserialized);
    }

    #[test]
    fn test_qed_schema_json_format() {
        let schema = QedSchema {
            name: "users".to_string(),
            types: vec![
                "integer".to_string(),
                "varchar".to_string(),
                "boolean".to_string(),
            ],
            key: vec![0],
            nullable: vec![false, false, true],
            guaranteed: vec!["id > 0".to_string()],
            fields: vec!["id".to_string(), "name".to_string(), "active".to_string()],
        };

        let json = serde_json::to_string_pretty(&schema).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["name"], "users");
        assert_eq!(
            parsed["types"],
            serde_json::json!(["integer", "varchar", "boolean"])
        );
        assert_eq!(parsed["key"], serde_json::json!([0]));
        assert_eq!(parsed["nullable"], serde_json::json!([false, false, true]));
        assert_eq!(parsed["guaranteed"], serde_json::json!(["id > 0"]));
        assert_eq!(
            parsed["fields"],
            serde_json::json!(["id", "name", "active"])
        );
    }

    #[test]
    fn test_schema_skips_empty_guaranteed() {
        let schema = QedSchema {
            name: "t".to_string(),
            types: vec!["integer".to_string()],
            key: vec![],
            nullable: vec![false],
            guaranteed: vec![],
            fields: vec!["a".to_string()],
        };

        let json = serde_json::to_string(&schema).unwrap();
        assert!(!json.contains("guaranteed"));
    }

    #[test]
    fn test_join_without_condition_serializes_correctly() {
        let join = QedRelation::Join {
            left: Box::new(QedRelation::Scan {
                table: "a".to_string(),
                fields: vec![],
            }),
            right: Box::new(QedRelation::Scan {
                table: "b".to_string(),
                fields: vec![],
            }),
            condition: None,
        };

        let json = serde_json::to_string(&join).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "Join");
        assert!(parsed.get("condition").is_none());
    }

    #[test]
    fn test_quantified_expr_serializes() {
        let expr = QedExpr::Quantified {
            cmp: "gt".to_string(),
            quantifier: "some".to_string(),
            subquery: Box::new(QedRelation::Scan {
                table: "t".to_string(),
                fields: vec![0],
            }),
        };

        let json = serde_json::to_string(&expr).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "Quantified");
        assert_eq!(parsed["cmp"], "gt");
        assert_eq!(parsed["quantifier"], "some");
        assert_eq!(parsed["subquery"]["type"], "Scan");
    }

    #[test]
    fn test_null_expr_serializes() {
        let expr = QedExpr::Null;
        let json = serde_json::to_string(&expr).unwrap();
        assert_eq!(json, r#"{"type":"Null"}"#);
    }

    #[test]
    fn test_all_value_types() {
        let values = vec![
            QedValue::Integer { value: 42 },
            QedValue::Float { value: 3.14 },
            QedValue::String {
                value: "hello".to_string(),
            },
            QedValue::Boolean { value: true },
        ];
        for v in &values {
            let json = serde_json::to_string(v).unwrap();
            let back: QedValue = serde_json::from_str(&json).unwrap();
            assert_eq!(v, &back);
        }
    }
}
