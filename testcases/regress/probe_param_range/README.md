# probe-param-range

Generates a probe SQL showing MIN/MAX/COUNT(DISTINCT)/COUNT(*) for parameterized
WHERE equality columns.

- **Category**: DataQuality
- **Safety**: Manual (generates suggestions only — never replaces original SQL)
- **Matches**: SELECT (and other DML) statements with at least one parameterized condition (`= ?`, `= :param`, etc.) in WHERE
- **Probe output**: `SELECT MIN(col) AS col_min, MAX(col) AS col_max, COUNT(DISTINCT col) AS col_distinct, COUNT(1) AS total FROM <tables> [WHERE ...]`

## Why

When a query uses `WHERE col = ?` (JDBC param) or `WHERE col = :param`, the user may
pass a value that does not exist in the data, resulting in an empty result set. This
probe shows the value range and cardinality of the parameter column, helping the
user choose valid input values.

## Cases

| Case | Type | Description |
|------|------|-------------|
| `pos-001-single-param` | positive | single `?` param → one column with MIN/MAX/COUNT(DISTINCT) |
| `pos-002-multiple-params` | positive | two `?` params → two columns with full range probe |
| `pos-003-mixed-literal-param` | positive | param + literal equality → literal preserved in WHERE |
| `pos-004-param-with-join` | positive | param on joined table → probe preserves JOIN |
| `pos-005-param-in-or-clause` | positive | param inside OR → tier1 extraction + condition filtered |
| `neg-001-no-where` | negative | no WHERE clause → no match |
| `neg-002-literal-only` | negative | only literal conditions, no params → no match |
| `neg-003-non-dml` | negative | non-query statement → no match |
