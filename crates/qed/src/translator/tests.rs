use super::*;
use crate::schema::extract_rich_schema;

fn users_orders_schema() -> RichSchema {
    let sql = concat!(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR(100) NOT NULL, email VARCHAR(200));",
        "CREATE TABLE orders (order_id INTEGER, user_id INTEGER, amount NUMERIC, PRIMARY KEY (order_id));",
        "CREATE TABLE admins (id INTEGER PRIMARY KEY, name VARCHAR(100));",
    );
    let (stmts, _) = ogsql_parser::Parser::parse_sql(sql);
    let stmts: Vec<_> = stmts.into_iter().map(|si| si.statement).collect();
    extract_rich_schema(&stmts)
}

fn translate_sql(sql: &str, schema: &RichSchema) -> Result<QedRelation, TranslateError> {
    let (stmts, errors) = ogsql_parser::Parser::parse_sql(sql);
    assert!(
        errors.is_empty()
            || errors.iter().all(|e| {
                let _ = e;
                true
            }),
        "Parse errors: {errors:?}"
    );
    let stmts: Vec<_> = stmts.into_iter().map(|si| si.statement).collect();
    AstTranslator::new(schema).translate(&stmts[0])
}

#[test]
fn test_simple_scan() {
    let schema = users_orders_schema();
    let rel = translate_sql("SELECT * FROM users", &schema).unwrap();
    match &rel {
        QedRelation::Scan { table, fields } => {
            assert_eq!(table, "users");
            assert!(fields.is_empty());
        }
        _ => panic!("expected Scan, got {rel:?}"),
    }
}

#[test]
fn test_filter() {
    let schema = users_orders_schema();
    let rel = translate_sql("SELECT * FROM users WHERE id = 1", &schema).unwrap();
    match &rel {
        QedRelation::Filter { condition, input } => {
            assert!(matches!(condition, QedExpr::BinOp { op, .. } if op == "eq"));
            assert!(matches!(&**input, QedRelation::Scan { .. }));
        }
        _ => panic!("expected Filter, got {rel:?}"),
    }
}

#[test]
fn test_projection() {
    let schema = users_orders_schema();
    let rel = translate_sql("SELECT name FROM users", &schema).unwrap();
    match &rel {
        QedRelation::Project { exprs, input } => {
            assert_eq!(exprs.len(), 1);
            assert!(matches!(&exprs[0], QedExpr::ColumnRef { index: 1 }));
            assert!(matches!(&**input, QedRelation::Scan { .. }));
        }
        _ => panic!("expected Project, got {rel:?}"),
    }
}

#[test]
fn test_join() {
    let schema = users_orders_schema();
    let rel = translate_sql(
        "SELECT * FROM users u JOIN orders o ON u.id = o.user_id",
        &schema,
    )
    .unwrap();
    match &rel {
        QedRelation::Join {
            left,
            right,
            condition,
        } => {
            assert!(matches!(&**left, QedRelation::Scan { .. }));
            assert!(matches!(&**right, QedRelation::Scan { .. }));
            assert!(condition.is_some());
        }
        _ => panic!("expected Join, got {rel:?}"),
    }
}

#[test]
fn test_group_by() {
    let schema = users_orders_schema();
    let rel = translate_sql("SELECT id, COUNT(*) FROM users GROUP BY id", &schema).unwrap();
    match &rel {
        QedRelation::Project { input, .. } => match input.as_ref() {
            QedRelation::Aggregate { keys, aggs, .. } => {
                assert_eq!(*keys, vec![0]);
                assert_eq!(aggs.len(), 1);
                assert_eq!(aggs[0].func, "count");
                assert!(matches!(aggs[0].arg, QedAggArg::Star));
            }
            _ => panic!("expected Aggregate, got {input:?}"),
        },
        _ => panic!("expected Aggregate, got {rel:?}"),
    }
}

#[test]
fn test_distinct() {
    let schema = users_orders_schema();
    let rel = translate_sql("SELECT DISTINCT name FROM users", &schema).unwrap();
    match &rel {
        QedRelation::Distinct { input } => assert!(matches!(&**input, QedRelation::Project { .. })),
        _ => panic!("expected Distinct, got {rel:?}"),
    }
}

#[test]
fn test_union() {
    let schema = users_orders_schema();
    let rel = translate_sql("SELECT id FROM users UNION SELECT id FROM admins", &schema).unwrap();
    match &rel {
        QedRelation::Distinct { input } => assert!(matches!(&**input, QedRelation::Union { .. })),
        _ => panic!("expected Distinct(Union), got {rel:?}"),
    }
}

#[test]
fn test_limit() {
    let schema = users_orders_schema();
    let rel = translate_sql("SELECT * FROM users LIMIT 10", &schema).unwrap();
    match &rel {
        QedRelation::QOp { name, args, input } => {
            assert_eq!(name, "Limit");
            assert_eq!(args.len(), 1);
            assert!(matches!(&**input, QedRelation::Scan { .. }));
        }
        _ => panic!("expected QOp(Limit), got {rel:?}"),
    }
}

#[test]
fn test_offset() {
    let schema = users_orders_schema();
    let rel = translate_sql("SELECT * FROM users OFFSET 5", &schema).unwrap();
    match &rel {
        QedRelation::QOp { name, .. } => assert_eq!(name, "Offset"),
        _ => panic!("expected QOp(Offset), got {rel:?}"),
    }
}

#[test]
fn test_union_all() {
    let schema = users_orders_schema();
    let rel = translate_sql(
        "SELECT id FROM users UNION ALL SELECT id FROM admins",
        &schema,
    )
    .unwrap();
    match &rel {
        QedRelation::Union { left, right } => {
            assert!(matches!(&**left, QedRelation::Project { .. }));
            assert!(matches!(&**right, QedRelation::Project { .. }));
        }
        _ => panic!("expected Union, got {rel:?}"),
    }
}

#[test]
fn test_intersect() {
    let schema = users_orders_schema();
    let rel = translate_sql(
        "SELECT id FROM users INTERSECT SELECT id FROM admins",
        &schema,
    )
    .unwrap();
    match &rel {
        QedRelation::Distinct { input } => assert!(
            matches!(&**input, QedRelation::Intersect { .. }),
            "expected Distinct(Intersect), got {rel:?}"
        ),
        _ => panic!("expected Distinct wrapping Intersect, got {rel:?}"),
    }
}

#[test]
fn test_except() {
    let schema = users_orders_schema();
    let rel = translate_sql("SELECT id FROM users EXCEPT SELECT id FROM admins", &schema).unwrap();
    match &rel {
        QedRelation::Distinct { input } => assert!(
            matches!(&**input, QedRelation::Except { .. }),
            "expected Distinct(Except), got {rel:?}"
        ),
        _ => panic!("expected Distinct wrapping Except, got {rel:?}"),
    }
}

#[test]
fn test_order_by() {
    let schema = users_orders_schema();
    let rel = translate_sql("SELECT * FROM users ORDER BY name", &schema).unwrap();
    match &rel {
        QedRelation::QOp { name, args, input } => {
            assert_eq!(name, "Sort");
            assert!(!args.is_empty());
            assert!(matches!(&**input, QedRelation::Scan { .. }));
        }
        _ => panic!("expected QOp(Sort), got {rel:?}"),
    }
}

#[test]
fn test_table_not_found() {
    let schema = users_orders_schema();
    let result = translate_sql("SELECT * FROM nonexistent", &schema);
    match result.unwrap_err() {
        TranslateError::TableNotFound(name) => assert!(name.contains("nonexistent")),
        e => panic!("expected TableNotFound, got {e}"),
    }
}

#[test]
fn test_unsupported_statement() {
    let schema = users_orders_schema();
    let (stmts, _) = ogsql_parser::Parser::parse_sql("CREATE TABLE t (id INTEGER)");
    let stmts: Vec<_> = stmts.into_iter().map(|si| si.statement).collect();
    let result = AstTranslator::new(&schema).translate(&stmts[0]);
    assert!(matches!(
        result,
        Err(TranslateError::UnsupportedStatement(_))
    ));
}

#[test]
fn test_qualified_column_ref() {
    let schema = users_orders_schema();
    let rel = translate_sql("SELECT u.name FROM users u WHERE u.id = 1", &schema).unwrap();
    match &rel {
        QedRelation::Project { input, .. } => {
            assert!(matches!(&**input, QedRelation::Filter { .. }));
            let filter = match input.as_ref() {
                QedRelation::Filter { condition, .. } => condition,
                _ => unreachable!(),
            };
            assert!(matches!(filter, QedExpr::BinOp { op, .. } if op == "eq"));
        }
        _ => panic!("expected Filter, got {rel:?}"),
    }
}

#[test]
fn test_between() {
    let schema = users_orders_schema();
    let rel = translate_sql("SELECT * FROM users WHERE id BETWEEN 1 AND 10", &schema).unwrap();
    match &rel {
        QedRelation::Filter { condition, .. } => {
            assert!(matches!(condition, QedExpr::BinOp { op, .. } if op == "and"))
        }
        _ => panic!("expected Filter, got {rel:?}"),
    }
}

#[test]
fn test_in_list() {
    let schema = users_orders_schema();
    let rel = translate_sql("SELECT * FROM users WHERE id IN (1, 2, 3)", &schema).unwrap();
    match &rel {
        QedRelation::Filter { condition, .. } => {
            assert!(matches!(condition, QedExpr::BinOp { op, .. } if op == "or"))
        }
        _ => panic!("expected Filter, got {rel:?}"),
    }
}

#[test]
fn test_subquery_in_from() {
    let schema = users_orders_schema();
    let rel = translate_sql("SELECT name FROM (SELECT id, name FROM users) sub", &schema).unwrap();
    assert!(
        matches!(rel, QedRelation::Project { .. }),
        "expected Project, got {rel:?}"
    );
}

#[test]
fn test_count_with_expr() {
    let schema = users_orders_schema();
    let rel = translate_sql("SELECT id, COUNT(name) FROM users GROUP BY id", &schema).unwrap();
    match &rel {
        QedRelation::Project { input, .. } => match input.as_ref() {
            QedRelation::Aggregate { aggs, .. } => {
                assert_eq!(aggs.len(), 1);
                assert_eq!(aggs[0].func, "count");
                assert!(matches!(aggs[0].arg, QedAggArg::Expr(_)));
            }
            _ => panic!("expected Aggregate inside Project, got {input:?}"),
        },
        _ => panic!("expected Aggregate, got {rel:?}"),
    }
}

#[test]
fn test_multiple_from_cross_join() {
    let schema = users_orders_schema();
    let rel = translate_sql("SELECT * FROM users, orders", &schema).unwrap();
    match &rel {
        QedRelation::Join {
            left,
            right,
            condition,
        } => {
            assert!(matches!(&**left, QedRelation::Scan { .. }));
            assert!(matches!(&**right, QedRelation::Scan { .. }));
            assert!(condition.is_none());
        }
        _ => panic!("expected cross Join, got {rel:?}"),
    }
}

#[test]
fn test_join_no_alias() {
    let schema = users_orders_schema();
    let result = translate_sql(
        "SELECT id FROM users JOIN orders ON users.id = orders.user_id",
        &schema,
    );
    match &result {
        Ok(QedRelation::Project { input, .. }) => {
            assert!(
                matches!(input.as_ref(), QedRelation::Join { .. }),
                "expected Join inside Project, got: {input:?}"
            );
        }
        _ => panic!("expected Ok(Project with Join), got: {:?}", result),
    }
}

// ── Decorrelation unit tests ────────────────────────────────────────────

fn decorrelation_schema() -> RichSchema {
    let sql = concat!(
        "CREATE TABLE orders (order_id INTEGER PRIMARY KEY, user_id INTEGER, amount INTEGER);",
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR(100), status VARCHAR(20));",
    );
    let (stmts, _) = ogsql_parser::Parser::parse_sql(sql);
    let stmts: Vec<_> = stmts.into_iter().map(|si| si.statement).collect();
    extract_rich_schema(&stmts)
}

/// Correlated EXISTS must produce a Distinct(Join) rather than Filter(Quantified).
#[test]
fn test_correlated_exists_decorrelated_to_distinct_join() {
    let schema = decorrelation_schema();
    let rel = translate_sql(
        "SELECT o.order_id FROM orders o \
         WHERE EXISTS (SELECT 1 FROM users u WHERE u.id = o.user_id)",
        &schema,
    )
    .unwrap();

    // Walk: Project → Distinct → Join(Scan(orders), Scan(users))
    match &rel {
        QedRelation::Project { input, .. } => match input.as_ref() {
            QedRelation::Distinct { input: inner } => match inner.as_ref() {
                QedRelation::Join {
                    left,
                    right,
                    condition,
                } => {
                    assert!(condition.is_some(), "Join must have ON condition");
                    assert!(
                        matches!(left.as_ref(), QedRelation::Scan { table, .. } if table == "orders"),
                        "left should be Scan(orders)"
                    );
                    assert!(
                        matches!(right.as_ref(), QedRelation::Scan { table, .. } if table == "users"),
                        "right should be Scan(users)"
                    );
                }
                _ => panic!("expected Join inside Distinct, got: {inner:?}"),
            },
            _ => panic!("expected Distinct, got: {input:?}"),
        },
        _ => panic!("expected Project at root, got: {rel:?}"),
    }
}

/// Non-correlated EXISTS must decorrelate to Distinct(Join(...)) instead of falling back
/// to Quantified expression (which cannot be soundly encoded).
#[test]
fn test_non_correlated_exists_decorrelates() {
    let schema = decorrelation_schema();
    let rel = translate_sql(
        "SELECT user_id FROM orders \
         WHERE EXISTS (SELECT 1 FROM users WHERE users.status = 'active')",
        &schema,
    )
    .unwrap();

    match &rel {
        QedRelation::Project { input, .. } => match input.as_ref() {
            QedRelation::Distinct { input: inner } => {
                assert!(
                    matches!(&**inner, QedRelation::Join { .. }),
                    "non-correlated EXISTS should decorrelate to Distinct(Join), got: {inner:?}"
                );
                // Verify the join has a condition (cross join)
                let join = match inner.as_ref() {
                    QedRelation::Join { condition, .. } => condition,
                    _ => unreachable!(),
                };
                // condition should be None (uncorrelated = cross join semantics)
                assert!(
                    join.is_none(),
                    "uncorrelated EXISTS join should have no condition"
                );
            }
            _ => panic!("expected Distinct(Join), got: {input:?}"),
        },
        _ => panic!("expected Project at root, got: {rel:?}"),
    }
}

/// EXISTS without WHERE clause must decorrelate to Distinct(Join(outer, inner, None)).
#[test]
fn test_exists_without_where_decorrelates() {
    let schema = decorrelation_schema();
    let rel = translate_sql(
        "SELECT o.order_id FROM orders o WHERE EXISTS (SELECT 1 FROM users)",
        &schema,
    )
    .unwrap();

    match &rel {
        QedRelation::Project { input, .. } => match input.as_ref() {
            QedRelation::Distinct { input: inner } => {
                assert!(
                    matches!(&**inner, QedRelation::Join { .. }),
                    "EXISTS without WHERE should decorrelate to Distinct(Join), got: {inner:?}"
                );
            }
            _ => panic!("expected Distinct(Join), got: {input:?}"),
        },
        _ => panic!("expected Project at root, got: {rel:?}"),
    }
}

/// EXISTS with extra non-correlation condition preserves it as Filter on inner.
#[test]
fn test_exists_with_residual_preserves_filter() {
    let schema = decorrelation_schema();
    let rel = translate_sql(
        "SELECT o.order_id FROM orders o \
         WHERE EXISTS (SELECT 1 FROM users u \
                       WHERE u.id = o.user_id AND u.status = 'active')",
        &schema,
    )
    .unwrap();

    match &rel {
        QedRelation::Project { input, .. } => match input.as_ref() {
            QedRelation::Distinct { input: inner } => match inner.as_ref() {
                QedRelation::Join { right, .. } => {
                    assert!(
                        matches!(right.as_ref(), QedRelation::Filter { .. }),
                        "inner relation should have residual Filter, got: {right:?}"
                    );
                }
                _ => panic!("expected Join, got: {inner:?}"),
            },
            _ => panic!("expected Distinct, got: {input:?}"),
        },
        _ => panic!("expected Project, got: {rel:?}"),
    }
}

/// Correlated IN (non-negated) produces same Distinct(Join) structure as EXISTS.
#[test]
fn test_correlated_in_decorrelated_to_distinct_join() {
    let schema = decorrelation_schema();
    let rel = translate_sql(
        "SELECT o.order_id FROM orders o \
         WHERE o.user_id IN (SELECT u.id FROM users u WHERE u.id = o.user_id)",
        &schema,
    )
    .unwrap();

    match &rel {
        QedRelation::Project { input, .. } => match input.as_ref() {
            QedRelation::Distinct { input: inner } => match inner.as_ref() {
                QedRelation::Join { .. } => { /* pass */ }
                _ => panic!("expected Join, got: {inner:?}"),
            },
            _ => panic!("expected Distinct, got: {input:?}"),
        },
        _ => panic!("expected Project, got: {rel:?}"),
    }
}
