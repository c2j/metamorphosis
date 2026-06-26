# or-to-union-all

Splits a top-level `WHERE OR` into multiple SELECTs connected by `UNION ALL`.

- **Category**: Performance
- **Safety**: Conditional (UNION ALL may produce duplicates that OR would not
  if conditions overlap; engine verifies preconditions)
- **Matches**: SELECT with a top-level `WHERE col1 = x OR col2 = y` and no
  DISTINCT / GROUP BY / HAVING / ORDER BY / LIMIT / JOIN / existing set operation
- **Replacement scope**: splits one OR at a time; engine re-runs for chained ORs

## Why

A query with `WHERE a = 1 OR b = 2` often forces a full table scan.
`SELECT ... WHERE a = 1 UNION ALL SELECT ... WHERE b = 2` can use separate
indexes for each branch, potentially improving performance.

## Cases

| Case | Type | Description |
|------|------|-------------|
| `pos-001-basic` | positive | two-condition top-level OR |
| `pos-002-three-conditions` | positive | three-condition chained OR — engine iterates |
| `pos-003-column-projection` | positive | specific column projection preserved in branches |
| `neg-001-and-only` | negative | no OR — AND only, no match |
| `neg-002-no-where` | negative | no WHERE clause at all |
| `neg-003-distinct` | negative | SELECT DISTINCT blocks the rewrite |
| `neg-004-join` | negative | JOIN in FROM blocks the rewrite |
