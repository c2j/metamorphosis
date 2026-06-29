//! Conversion layer from [`QedInput`] IR to the qed-prover's native JSON format.
//!
//! The prover uses externally-tagged relations (`{"scan": 0}`), untagged
//! expressions (`{"column": 0, "type": "INTEGER"}`), nested schema keys
//! (`[[0]]`), and tuple-style `help`/`queries` (serialized as arrays).

use crate::ir::{QedAggArg, QedAggCall, QedExpr, QedInput, QedRelation, QedSchema, QedValue};
use crate::prover::ProverError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Transparent `usize` wrapper for column / schema indices.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VL(pub usize);

/// Prover data type — custom Serialize/Deserialize emits SQL-standard uppercase names.
#[derive(Debug, Clone, PartialEq)]
pub enum ProverDataType {
    Integer,
    Real,
    Boolean,
    String,
    Custom(String),
}

impl Serialize for ProverDataType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            ProverDataType::Integer => serializer.serialize_str("INTEGER"),
            ProverDataType::Real => serializer.serialize_str("REAL"),
            ProverDataType::Boolean => serializer.serialize_str("BOOLEAN"),
            ProverDataType::String => serializer.serialize_str("STRING"),
            ProverDataType::Custom(s) => serializer.serialize_str(s),
        }
    }
}

impl<'de> Deserialize<'de> for ProverDataType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(match s.to_uppercase().as_str() {
            "INTEGER" | "INT" | "BIGINT" | "SMALLINT" | "TINYINT" | "TIMESTAMP" | "DATE"
            | "TIME" => ProverDataType::Integer,
            "REAL" | "FLOAT" | "DOUBLE" | "DECIMAL" | "NUMERIC" => ProverDataType::Real,
            "BOOLEAN" | "BOOL" => ProverDataType::Boolean,
            "STRING" | "VARCHAR" | "CHAR" | "TEXT" | "NCHAR" | "NVARCHAR" | "BPCHAR" => {
                ProverDataType::String
            }
            other => ProverDataType::Custom(other.to_string()),
        })
    }
}

/// Join kind — serialized UPPERCASE: "INNER", "LEFT", etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ProverJoinKind {
    Inner,
    Left,
    Right,
    Full,
    Semi,
    Anti,
}

/// Prover aggregate call — mirrors `AggCall` in the prover's `relation` module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProverAggCall {
    #[serde(rename = "operator")]
    pub op: String,
    #[serde(rename = "operand")]
    pub args: Vec<ProverExpr>,
    #[serde(default)]
    pub distinct: bool,
    #[serde(default)]
    pub ignore_nulls: bool,
    #[serde(rename = "type")]
    pub ty: ProverDataType,
}

/// Prover expression — untagged enum: `Col` or `Op`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(untagged)]
pub enum ProverExpr {
    Col {
        column: VL,
        #[serde(rename = "type")]
        ty: ProverDataType,
    },
    Op {
        #[serde(rename = "operator")]
        op: String,
        #[serde(rename = "operand")]
        args: Vec<ProverExpr>,
        #[serde(rename = "type")]
        ty: ProverDataType,
        #[serde(rename = "query", skip_serializing_if = "Option::is_none")]
        rel: Option<Box<ProverRelation>>,
    },
}

/// Prover relation — externally tagged by camelCase variant name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProverRelation {
    Singleton,
    Scan(VL),
    Filter {
        condition: ProverExpr,
        source: Box<ProverRelation>,
    },
    Project {
        #[serde(rename = "target")]
        columns: Vec<ProverExpr>,
        source: Box<ProverRelation>,
    },
    Join {
        condition: ProverExpr,
        left: Box<ProverRelation>,
        right: Box<ProverRelation>,
        kind: ProverJoinKind,
    },
    Correlate {
        left: Box<ProverRelation>,
        right: Box<ProverRelation>,
        kind: ProverJoinKind,
    },
    Union(Vec<ProverRelation>),
    Intersect(Vec<ProverRelation>),
    Except(Box<ProverRelation>, Box<ProverRelation>),
    Distinct(Box<ProverRelation>),
    Values {
        schema: Vec<ProverDataType>,
        content: Vec<Vec<ProverExpr>>,
    },
    Aggregate {
        columns: Vec<ProverAggCall>,
        source: Box<ProverRelation>,
    },
    Group {
        keys: Vec<ProverExpr>,
        columns: Vec<ProverAggCall>,
        source: Box<ProverRelation>,
    },
    Sort {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        collation: Vec<(usize, ProverDataType, String)>,
        #[serde(skip_serializing_if = "Option::is_none")]
        offset: Option<ProverExpr>,
        #[serde(skip_serializing_if = "Option::is_none")]
        limit: Option<ProverExpr>,
        source: Box<ProverRelation>,
    },
}

/// Prover table schema — mirrors `Schema` in the prover's `shared` module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProverSchema {
    pub types: Vec<ProverDataType>,
    #[serde(rename = "key")]
    pub primary: Vec<Vec<usize>>,
    #[serde(rename = "nullable")]
    pub nullabilities: Vec<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guaranteed: Vec<ProverExpr>,
    pub name: String,
    pub fields: Vec<String>,
}

/// Top-level prover input — matches `Input` in the prover's `pipeline` module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProverInput {
    pub schemas: Vec<ProverSchema>,
    pub queries: (ProverRelation, ProverRelation),
    pub help: (String, String),
}

/// Map an internal SQL type string to a [`ProverDataType`].
pub fn map_data_type(our_type: &str) -> ProverDataType {
    match our_type.to_lowercase().as_str() {
        "integer" | "int" | "bigint" | "smallint" | "tinyint" | "timestamp" | "date" | "time"
        | "timestamptz" | "timetz" => ProverDataType::Integer,
        "real" | "float" | "double" | "decimal" | "numeric" | "float4" | "float8" => {
            ProverDataType::Real
        }
        "boolean" | "bool" => ProverDataType::Boolean,
        "string" | "varchar" | "char" | "text" | "nvarchar" | "nchar" | "bpchar" => {
            ProverDataType::String
        }
        other => ProverDataType::Custom(other.to_uppercase()),
    }
}

fn map_binary_operator(op: &str) -> String {
    match op.to_lowercase().as_str() {
        "eq" | "=" | "==" => "=",
        "neq" | "!=" | "<>" => "!=",
        "gt" | ">" => ">",
        "gte" | ">=" => ">=",
        "lt" | "<" => "<",
        "lte" | "<=" => "<=",
        "and" | "&&" => "and",
        "or" | "||" => "or",
        "add" | "+" => "+",
        "sub" | "-" => "-",
        "mul" | "*" => "*",
        "div" | "/" => "/",
        "mod" | "%" => "%",
        other => other,
    }
    .to_string()
}

/// Convert a [`QedExpr`] to a [`ProverExpr`].
///
/// `column_types` maps column indices to their [`ProverDataType`]. When `None`,
/// defaults to [`ProverDataType::Integer`] for all columns.
pub fn convert_expr(
    expr: &QedExpr,
    column_types: Option<&HashMap<usize, ProverDataType>>,
) -> ProverExpr {
    let col_ty = |idx: usize| -> ProverDataType {
        column_types
            .and_then(|m| m.get(&idx))
            .cloned()
            .unwrap_or(ProverDataType::Integer)
    };
    match expr {
        QedExpr::ColumnRef { index } => ProverExpr::Col {
            column: VL(*index),
            ty: col_ty(*index),
        },
        QedExpr::Literal { value } => match value {
            QedValue::Integer { value: v } => ProverExpr::Op {
                op: v.to_string(),
                args: vec![],
                ty: ProverDataType::Integer,
                rel: None,
            },
            QedValue::Float { value: v } => ProverExpr::Op {
                op: v.to_string(),
                args: vec![],
                ty: ProverDataType::Real,
                rel: None,
            },
            QedValue::String { value: v } => ProverExpr::Op {
                op: format!("\"{}\"", v),
                args: vec![],
                ty: ProverDataType::String,
                rel: None,
            },
            QedValue::Boolean { value: v } => ProverExpr::Op {
                op: v.to_string(),
                args: vec![],
                ty: ProverDataType::Boolean,
                rel: None,
            },
        },
        QedExpr::BinOp { op, left, right } => ProverExpr::Op {
            op: map_binary_operator(op),
            args: vec![
                convert_expr(left, column_types),
                convert_expr(right, column_types),
            ],
            ty: ProverDataType::Boolean,
            rel: None,
        },
        QedExpr::UnOp { op, expr: inner } => {
            let lower = op.to_lowercase();
            let mapped_op = match lower.as_str() {
                "not" => "not",
                "neg" | "negative" | "-" => "neg",
                other => other,
            };
            ProverExpr::Op {
                op: mapped_op.to_string(),
                args: vec![convert_expr(inner, column_types)],
                ty: ProverDataType::Boolean,
                rel: None,
            }
        }
        QedExpr::Function { name, args } => ProverExpr::Op {
            op: name.clone(),
            args: args.iter().map(|a| convert_expr(a, column_types)).collect(),
            ty: ProverDataType::Integer,
            rel: None,
        },
        QedExpr::Null => ProverExpr::Op {
            op: "null".to_string(),
            args: vec![],
            ty: ProverDataType::Custom("NULL".to_string()),
            rel: None,
        },
        QedExpr::Quantified {
            cmp,
            quantifier,
            subquery,
        } => ProverExpr::Op {
            op: format!("{}_{}", quantifier, cmp),
            args: vec![],
            ty: ProverDataType::Boolean,
            rel: convert_relation(subquery, &HashMap::new())
                .ok()
                .map(Box::new),
        },
    }
}

/// Convert a [`QedAggCall`] to a [`ProverAggCall`].
pub fn convert_agg_call(
    call: &QedAggCall,
    column_types: Option<&HashMap<usize, ProverDataType>>,
) -> ProverAggCall {
    let arg_expr = match &call.arg {
        QedAggArg::Star => ProverExpr::Op {
            op: "star".to_string(),
            args: vec![],
            ty: ProverDataType::Integer,
            rel: None,
        },
        QedAggArg::Expr(e) => convert_expr(e, column_types),
    };
    ProverAggCall {
        op: call.func.clone(),
        args: vec![arg_expr],
        distinct: call.distinct,
        ignore_nulls: false,
        ty: ProverDataType::Integer,
    }
}

/// Convert a [`QedRelation`] to a [`ProverRelation`].
///
/// `schema_index` maps table names to schemas array positions.
/// See mapping rules in variant match arms.
#[allow(clippy::too_many_lines)]
pub fn convert_relation(
    rel: &QedRelation,
    schema_index: &HashMap<String, usize>,
) -> Result<ProverRelation, ProverError> {
    convert_relation_with_types(rel, schema_index, None)
}

/// Like [`convert_relation`] but provides column type information for correct
/// expression type annotations.
pub fn convert_relation_with_types(
    rel: &QedRelation,
    schema_index: &HashMap<String, usize>,
    column_types: Option<&HashMap<usize, ProverDataType>>,
) -> Result<ProverRelation, ProverError> {
    match rel {
        QedRelation::Scan { table, .. } => {
            let idx = schema_index.get(table).copied().ok_or_else(|| {
                ProverError::Io(format!(
                    "convert_relation: table '{table}' not found in schema_index"
                ))
            })?;
            Ok(ProverRelation::Scan(VL(idx)))
        }
        QedRelation::Filter { condition, input } => Ok(ProverRelation::Filter {
            condition: convert_expr(condition, column_types),
            source: Box::new(convert_relation_with_types(
                input,
                schema_index,
                column_types,
            )?),
        }),
        QedRelation::Project { exprs, input } => Ok(ProverRelation::Project {
            columns: exprs
                .iter()
                .map(|e| convert_expr(e, column_types))
                .collect(),
            source: Box::new(convert_relation_with_types(
                input,
                schema_index,
                column_types,
            )?),
        }),
        QedRelation::Join {
            left,
            right,
            condition,
        } => {
            let cond = match condition {
                Some(c) => convert_expr(c, column_types),
                None => ProverExpr::Op {
                    op: "true".to_string(),
                    args: vec![],
                    ty: ProverDataType::Boolean,
                    rel: None,
                },
            };
            Ok(ProverRelation::Join {
                condition: cond,
                left: Box::new(convert_relation_with_types(
                    left,
                    schema_index,
                    column_types,
                )?),
                right: Box::new(convert_relation_with_types(
                    right,
                    schema_index,
                    column_types,
                )?),
                kind: ProverJoinKind::Inner,
            })
        }
        QedRelation::Union { left, right } => Ok(ProverRelation::Union(vec![
            convert_relation_with_types(left, schema_index, column_types)?,
            convert_relation_with_types(right, schema_index, column_types)?,
        ])),
        QedRelation::Intersect { left, right } => Ok(ProverRelation::Intersect(vec![
            convert_relation_with_types(left, schema_index, column_types)?,
            convert_relation_with_types(right, schema_index, column_types)?,
        ])),
        QedRelation::Except { left, right } => Ok(ProverRelation::Except(
            Box::new(convert_relation_with_types(
                left,
                schema_index,
                column_types,
            )?),
            Box::new(convert_relation_with_types(
                right,
                schema_index,
                column_types,
            )?),
        )),
        QedRelation::Distinct { input } => Ok(ProverRelation::Distinct(Box::new(
            convert_relation_with_types(input, schema_index, column_types)?,
        ))),
        QedRelation::Aggregate { keys, aggs, input } => Ok(ProverRelation::Group {
            keys: keys
                .iter()
                .map(|k| convert_expr(&QedExpr::ColumnRef { index: *k }, column_types))
                .collect(),
            columns: aggs
                .iter()
                .map(|a| convert_agg_call(a, column_types))
                .collect(),
            source: Box::new(convert_relation_with_types(
                input,
                schema_index,
                column_types,
            )?),
        }),
        QedRelation::Values { rows } => Ok(ProverRelation::Values {
            schema: vec![],
            content: rows
                .iter()
                .map(|row| row.iter().map(|e| convert_expr(e, column_types)).collect())
                .collect(),
        }),
        QedRelation::QOp { name, args, input } => match name.to_lowercase().as_str() {
            "sort" | "order" | "orderby" => Ok(ProverRelation::Sort {
                collation: vec![],
                offset: None,
                limit: None,
                source: Box::new(convert_relation_with_types(
                    input,
                    schema_index,
                    column_types,
                )?),
            }),
            "limit" => Ok(ProverRelation::Sort {
                collation: vec![],
                offset: None,
                limit: args.first().map(|a| convert_expr(a, column_types)),
                source: Box::new(convert_relation_with_types(
                    input,
                    schema_index,
                    column_types,
                )?),
            }),
            _ => {
                tracing::warn!(
                    "convert_relation: unknown QOp '{}', treating as passthrough",
                    name
                );
                convert_relation_with_types(input, schema_index, column_types)
            }
        },
    }
}

/// Convert a [`QedSchema`] to a [`ProverSchema`] (flat key → nested, guaranteed empty).
pub fn convert_schema(schema: &QedSchema) -> ProverSchema {
    ProverSchema {
        name: schema.name.clone(),
        types: schema.types.iter().map(|t| map_data_type(t)).collect(),
        primary: if schema.key.is_empty() {
            vec![]
        } else {
            vec![schema.key.clone()]
        },
        nullabilities: schema.nullable.clone(),
        guaranteed: vec![],
        fields: schema.fields.clone(),
    }
}

/// Convert a [`QedInput`] to a [`ProverInput`].
///
/// `schema_name_map` maps table names to qualified names (e.g., `"emp"` → `"PUBLIC.emp"`).
/// Schema index is built automatically; `help` is duplicated for both queries.
pub fn convert_input(
    our: &QedInput,
    schema_name_map: &HashMap<String, String>,
) -> Result<ProverInput, ProverError> {
    let mut schema_index: HashMap<String, usize> = HashMap::with_capacity(our.schemas.len());
    let mut prover_schemas: Vec<ProverSchema> = Vec::with_capacity(our.schemas.len());
    let mut column_types: HashMap<usize, ProverDataType> = HashMap::new();
    let mut col_offset: usize = 0;
    for (i, schema) in our.schemas.iter().enumerate() {
        let qualified = schema_name_map
            .get(&schema.name)
            .cloned()
            .unwrap_or_else(|| schema.name.clone());
        schema_index.insert(qualified.clone(), i);
        schema_index.insert(schema.name.clone(), i);
        for (col_idx, ty) in schema.types.iter().enumerate() {
            column_types.insert(col_offset + col_idx, map_data_type(ty));
        }
        col_offset += schema.types.len();
        prover_schemas.push(ProverSchema {
            name: qualified,
            types: schema.types.iter().map(|t| map_data_type(t)).collect(),
            primary: if schema.key.is_empty() {
                vec![]
            } else {
                vec![schema.key.clone()]
            },
            nullabilities: schema.nullable.clone(),
            guaranteed: vec![],
            fields: schema.fields.clone(),
        });
    }
    Ok(ProverInput {
        schemas: prover_schemas,
        queries: (
            convert_relation_with_types(&our.queries[0], &schema_index, Some(&column_types))?,
            convert_relation_with_types(&our.queries[1], &schema_index, Some(&column_types))?,
        ),
        help: (our.help.clone(), our.help.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;

    fn idx() -> HashMap<String, usize> {
        let mut m = HashMap::new();
        m.insert("t".to_string(), 0);
        m
    }

    fn qed() -> QedInput {
        QedInput {
            schemas: vec![QedSchema {
                name: "R".to_string(),
                types: vec!["integer".to_string(), "integer".to_string()],
                key: vec![0],
                nullable: vec![false, true],
                guaranteed: vec![],
                fields: vec!["x".to_string(), "y".to_string()],
            }],
            queries: [
                QedRelation::Scan {
                    table: "R".to_string(),
                    fields: vec![],
                },
                QedRelation::Scan {
                    table: "R".to_string(),
                    fields: vec![],
                },
            ],
            help: "test".to_string(),
        }
    }

    #[test]
    fn test_convert_scan() {
        let rel = convert_relation(
            &QedRelation::Scan {
                table: "t".to_string(),
                fields: vec![],
            },
            &idx(),
        )
        .unwrap();
        assert_eq!(rel, ProverRelation::Scan(VL(0)));
    }

    #[test]
    fn test_convert_filter() {
        let rel = convert_relation(
            &QedRelation::Filter {
                condition: QedExpr::BinOp {
                    op: "gt".to_string(),
                    left: Box::new(QedExpr::ColumnRef { index: 0 }),
                    right: Box::new(QedExpr::Literal {
                        value: QedValue::Integer { value: 10 },
                    }),
                },
                input: Box::new(QedRelation::Scan {
                    table: "t".to_string(),
                    fields: vec![],
                }),
            },
            &idx(),
        )
        .unwrap();
        assert_eq!(
            rel,
            ProverRelation::Filter {
                condition: ProverExpr::Op {
                    op: ">".to_string(),
                    args: vec![
                        ProverExpr::Col {
                            column: VL(0),
                            ty: ProverDataType::Integer
                        },
                        ProverExpr::Op {
                            op: "10".to_string(),
                            args: vec![],
                            ty: ProverDataType::Integer,
                            rel: None
                        }
                    ],
                    ty: ProverDataType::Boolean,
                    rel: None
                },
                source: Box::new(ProverRelation::Scan(VL(0)))
            }
        );
    }

    #[test]
    fn test_convert_project() {
        let rel = convert_relation(
            &QedRelation::Project {
                exprs: vec![
                    QedExpr::ColumnRef { index: 0 },
                    QedExpr::Literal {
                        value: QedValue::Integer { value: 1 },
                    },
                ],
                input: Box::new(QedRelation::Scan {
                    table: "t".to_string(),
                    fields: vec![],
                }),
            },
            &idx(),
        )
        .unwrap();
        assert_eq!(
            rel,
            ProverRelation::Project {
                columns: vec![
                    ProverExpr::Col {
                        column: VL(0),
                        ty: ProverDataType::Integer
                    },
                    ProverExpr::Op {
                        op: "1".to_string(),
                        args: vec![],
                        ty: ProverDataType::Integer,
                        rel: None
                    }
                ],
                source: Box::new(ProverRelation::Scan(VL(0)))
            }
        );
    }

    #[test]
    fn test_convert_join() {
        let mut m = HashMap::new();
        m.insert("a".to_string(), 0usize);
        m.insert("b".to_string(), 1usize);
        let rel = convert_relation(
            &QedRelation::Join {
                left: Box::new(QedRelation::Scan {
                    table: "a".to_string(),
                    fields: vec![],
                }),
                right: Box::new(QedRelation::Scan {
                    table: "b".to_string(),
                    fields: vec![],
                }),
                condition: Some(QedExpr::BinOp {
                    op: "eq".to_string(),
                    left: Box::new(QedExpr::ColumnRef { index: 0 }),
                    right: Box::new(QedExpr::ColumnRef { index: 1 }),
                }),
            },
            &m,
        )
        .unwrap();
        assert_eq!(
            rel,
            ProverRelation::Join {
                condition: ProverExpr::Op {
                    op: "=".to_string(),
                    args: vec![
                        ProverExpr::Col {
                            column: VL(0),
                            ty: ProverDataType::Integer
                        },
                        ProverExpr::Col {
                            column: VL(1),
                            ty: ProverDataType::Integer
                        }
                    ],
                    ty: ProverDataType::Boolean,
                    rel: None
                },
                left: Box::new(ProverRelation::Scan(VL(0))),
                right: Box::new(ProverRelation::Scan(VL(1))),
                kind: ProverJoinKind::Inner
            }
        );
    }

    #[test]
    fn test_convert_union() {
        let rel = convert_relation(
            &QedRelation::Union {
                left: Box::new(QedRelation::Scan {
                    table: "t".to_string(),
                    fields: vec![],
                }),
                right: Box::new(QedRelation::Scan {
                    table: "t".to_string(),
                    fields: vec![],
                }),
            },
            &idx(),
        )
        .unwrap();
        assert_eq!(
            rel,
            ProverRelation::Union(vec![
                ProverRelation::Scan(VL(0)),
                ProverRelation::Scan(VL(0))
            ])
        );
    }

    #[test]
    fn test_convert_schema_key() {
        let s = QedSchema {
            name: "t".to_string(),
            types: vec!["integer".to_string()],
            key: vec![0],
            nullable: vec![false],
            guaranteed: vec![],
            fields: vec!["a".to_string()],
        };
        assert_eq!(convert_schema(&s).primary, vec![vec![0]]);
        let s2 = QedSchema { key: vec![], ..s };
        let expected: Vec<Vec<usize>> = vec![];
        assert_eq!(convert_schema(&s2).primary, expected);
    }

    #[test]
    fn test_convert_help_is_tuple() {
        assert_eq!(
            convert_input(&qed(), &HashMap::new()).unwrap().help,
            ("test".to_string(), "test".to_string())
        );
    }

    #[test]
    fn test_convert_aggregate_with_group_by() {
        let rel = convert_relation(
            &QedRelation::Aggregate {
                keys: vec![0],
                aggs: vec![QedAggCall {
                    func: "sum".to_string(),
                    arg: QedAggArg::Expr(QedExpr::ColumnRef { index: 2 }),
                    distinct: false,
                }],
                input: Box::new(QedRelation::Scan {
                    table: "t".to_string(),
                    fields: vec![],
                }),
            },
            &idx(),
        )
        .unwrap();
        assert_eq!(
            rel,
            ProverRelation::Group {
                keys: vec![ProverExpr::Col {
                    column: VL(0),
                    ty: ProverDataType::Integer
                }],
                columns: vec![ProverAggCall {
                    op: "sum".to_string(),
                    args: vec![ProverExpr::Col {
                        column: VL(2),
                        ty: ProverDataType::Integer
                    }],
                    distinct: false,
                    ignore_nulls: false,
                    ty: ProverDataType::Integer
                }],
                source: Box::new(ProverRelation::Scan(VL(0)))
            }
        );
    }

    #[test]
    fn test_map_data_type_variants() {
        assert_eq!(map_data_type("integer"), ProverDataType::Integer);
        assert_eq!(map_data_type("BIGINT"), ProverDataType::Integer);
        assert_eq!(map_data_type("timestamp"), ProverDataType::Integer);
        assert_eq!(map_data_type("date"), ProverDataType::Integer);
        assert_eq!(map_data_type("real"), ProverDataType::Real);
        assert_eq!(map_data_type("DOUBLE"), ProverDataType::Real);
        assert_eq!(map_data_type("decimal"), ProverDataType::Real);
        assert_eq!(map_data_type("boolean"), ProverDataType::Boolean);
        assert_eq!(map_data_type("bool"), ProverDataType::Boolean);
        assert_eq!(map_data_type("varchar"), ProverDataType::String);
        assert_eq!(map_data_type("TEXT"), ProverDataType::String);
        assert_eq!(map_data_type("char"), ProverDataType::String);
        assert_eq!(
            map_data_type("uuid"),
            ProverDataType::Custom("UUID".to_string())
        );
        assert_eq!(
            map_data_type("jsonb"),
            ProverDataType::Custom("JSONB".to_string())
        );
    }

    #[test]
    fn test_roundtrip_simple_query() {
        let name_map: HashMap<String, String> = [("R".to_string(), "PUBLIC.R".to_string())].into();
        let prover = convert_input(&qed(), &name_map).unwrap();
        assert_eq!(prover.schemas[0].name, "PUBLIC.R");
        let json = serde_json::to_value(&prover).expect("serialize");
        assert!(json.is_object());
        assert!(json.get("schemas").is_some());
        assert_eq!(json["queries"].as_array().unwrap().len(), 2);
        assert_eq!(json["help"].as_array().unwrap().len(), 2);
        assert_eq!(json["schemas"][0]["key"], serde_json::json!([[0]]));
        assert_eq!(json["queries"][0]["scan"], serde_json::json!(0));
    }

    #[test]
    fn test_prover_input_serialization_format() {
        let prover = ProverInput {
            schemas: vec![ProverSchema {
                name: "PUBLIC.EMP".to_string(),
                types: vec![ProverDataType::Integer, ProverDataType::String],
                primary: vec![vec![0]],
                nullabilities: vec![false, false],
                guaranteed: vec![],
                fields: vec!["EMPNO".to_string(), "ENAME".to_string()],
            }],
            queries: (ProverRelation::Scan(VL(0)), ProverRelation::Scan(VL(0))),
            help: ("SELECT *".to_string(), "SELECT ENAME".to_string()),
        };
        let json = serde_json::to_value(&prover).expect("serialize");
        assert_eq!(
            json["schemas"][0]["types"],
            serde_json::json!(["INTEGER", "STRING"])
        );
        assert_eq!(json["schemas"][0]["key"], serde_json::json!([[0]]));
        assert_eq!(
            json["schemas"][0]["nullable"],
            serde_json::json!([false, false])
        );
        assert!(json["schemas"][0].get("guaranteed").is_none());
        assert_eq!(json["queries"][0]["scan"], serde_json::json!(0));
        assert_eq!(
            json["help"],
            serde_json::json!(["SELECT *", "SELECT ENAME"])
        );
    }

    #[test]
    fn test_convert_expr_types() {
        assert_eq!(
            convert_expr(&QedExpr::ColumnRef { index: 3 }, None),
            ProverExpr::Col {
                column: VL(3),
                ty: ProverDataType::Integer
            }
        );
        let mut types = HashMap::new();
        types.insert(3, ProverDataType::String);
        assert_eq!(
            convert_expr(&QedExpr::ColumnRef { index: 3 }, Some(&types)),
            ProverExpr::Col {
                column: VL(3),
                ty: ProverDataType::String
            }
        );
        assert_eq!(
            convert_expr(
                &QedExpr::Literal {
                    value: QedValue::Integer { value: 42 }
                },
                None
            ),
            ProverExpr::Op {
                op: "42".to_string(),
                args: vec![],
                ty: ProverDataType::Integer,
                rel: None
            }
        );
        assert_eq!(
            convert_expr(
                &QedExpr::Literal {
                    value: QedValue::Boolean { value: true }
                },
                None
            ),
            ProverExpr::Op {
                op: "true".to_string(),
                args: vec![],
                ty: ProverDataType::Boolean,
                rel: None
            }
        );
        assert_eq!(
            convert_expr(&QedExpr::Null, None),
            ProverExpr::Op {
                op: "null".to_string(),
                args: vec![],
                ty: ProverDataType::Custom("NULL".to_string()),
                rel: None
            }
        );
    }

    #[test]
    fn test_convert_set_operations() {
        let sc = || QedRelation::Scan {
            table: "t".to_string(),
            fields: vec![],
        };
        assert_eq!(
            convert_relation(
                &QedRelation::Intersect {
                    left: Box::new(sc()),
                    right: Box::new(sc())
                },
                &idx()
            )
            .unwrap(),
            ProverRelation::Intersect(vec![
                ProverRelation::Scan(VL(0)),
                ProverRelation::Scan(VL(0))
            ])
        );
        assert_eq!(
            convert_relation(
                &QedRelation::Except {
                    left: Box::new(sc()),
                    right: Box::new(sc())
                },
                &idx()
            )
            .unwrap(),
            ProverRelation::Except(
                Box::new(ProverRelation::Scan(VL(0))),
                Box::new(ProverRelation::Scan(VL(0)))
            )
        );
        assert_eq!(
            convert_relation(
                &QedRelation::Distinct {
                    input: Box::new(sc())
                },
                &idx()
            )
            .unwrap(),
            ProverRelation::Distinct(Box::new(ProverRelation::Scan(VL(0))))
        );
    }
}
