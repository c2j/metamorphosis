# delete-to-truncate

Converts full-table `DELETE` (no WHERE, USING, ORDER BY, LIMIT, RETURNING, or
CTE) to `TRUNCATE TABLE`.

- **Category**: Performance
- **Safety**: Conditional (TRUNCATE has different transactional semantics —
  cannot be rolled back in some databases, resets identity counters, and
  requires different lock levels)
- **Matches**: DELETE statements with exactly one base-table target and no
  WHERE / USING / ORDER BY / LIMIT / RETURNING / CTE clauses
- **Replacement scope**: single statement (one-shot, no iteration)

## Why

`TRUNCATE` is a bulk operation that is significantly faster than `DELETE` when
removing all rows from a table.

## Cases

| Case | Type | Description |
|------|------|-------------|
| `pos-001-basic` | positive | simple `DELETE FROM users` |
| `pos-002-qualified-table` | positive | schema-qualified table name preserved |
| `neg-001-with-where` | negative | WHERE clause present — blocked |
| `neg-002-with-returning` | negative | RETURNING clause present — blocked |
| `neg-003-non-delete` | negative | SELECT statement — not a DELETE |
