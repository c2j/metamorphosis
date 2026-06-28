# subquery-to-join

Converts WHERE subqueries (EXISTS, IN, NOT EXISTS, NOT IN) to JOINs.

- **Category**: Performance
- **Safety**: Conditional (EXISTS/IN → INNER JOIN; NOT EXISTS/NOT IN → LEFT JOIN;
  scalar subqueries in SELECT → Manual suggestion only)
- **Matches**: SELECT statements containing rewritable subquery patterns in WHERE
  EXISTS → INNER JOIN
  NOT EXISTS → LEFT JOIN + IS NULL
  `expr IN (SELECT ...)` → INNER JOIN
  `expr NOT IN (SELECT ...)` → LEFT JOIN + IS NULL
- **Safety guards**: Only rewrites subqueries with a single table, no GROUP BY,
  no HAVING, no JOINs, and no set operations inside the subquery.

## Why

JOINs are generally more efficient than correlated subqueries because the
optimiser can choose better join orders, use index access paths, and apply
hash/merge join strategies that are unavailable to iterative subquery execution.

## Cases

| Case | Type | Description |
|------|------|-------------|
| `pos-001-exists` | positive | EXISTS → INNER JOIN |
| `pos-002-not-exists` | positive | NOT EXISTS → LEFT JOIN + IS NULL |
| `pos-003-in-subquery` | positive | IN (SELECT ...) → INNER JOIN |
| `pos-004-not-in-subquery` | positive | NOT IN (SELECT ...) → LEFT JOIN + IS NULL |
| `pos-005-exists-with-extra-conditions` | positive | EXISTS with extra WHERE conditions preserved |
| `neg-001-no-subquery` | negative | Simple WHERE — no subquery to rewrite |
| `neg-002-multi-table-subquery` | negative | Subquery with JOIN — safety guard prevents rewrite |
| `neg-003-aggregate-subquery` | negative | Subquery with GROUP BY — safety guard prevents rewrite |
| `neg-004-non-select` | negative | DELETE statement — rule is SELECT-only |
