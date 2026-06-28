# probe-join-integrity

Generates a probe SQL checking referential integrity for JOIN queries.

- **Category**: DataQuality
- **Safety**: Manual (generates suggestions only — never replaces original SQL)
- **Matches**: SELECT statements with at least one JOIN in FROM clause
- **Probe output**: `SELECT COUNT(1) AS total, COUNT(right_col) AS matched FROM left LEFT JOIN right ON condition` (one probe per JOIN)

## Why

In multi-table JOIN queries, low match rates between joined tables may indicate
orphan records or data integrity issues. This probe performs a LEFT JOIN and
counts matched vs total rows, revealing the percentage of left-table rows that
have corresponding right-table entries.

## Cases

| Case | Type | Description |
|------|------|-------------|
| `pos-001-single-inner-join` | positive | single INNER JOIN → one probe with total + matched |
| `pos-002-multiple-joins` | positive | two JOINs → two separate probes |
| `pos-003-left-join` | positive | LEFT JOIN → probe still uses LEFT JOIN |
| `pos-004-join-with-where` | positive | JOIN with WHERE → probe purpose preserves reference |
| `neg-001-no-join` | negative | single table FROM, no JOIN → no match |
| `neg-002-non-select` | negative | non-SELECT statement → no match |
