# probe-null-ratio

Generates a probe SQL measuring NULL ratio for columns used in WHERE / JOIN ON conditions.

- **Category**: DataQuality
- **Safety**: Manual (generates suggestions only — never replaces original SQL)
- **Matches**: SELECT statements with at least one column reference in WHERE or JOIN ON
- **Probe output**: `SELECT COUNT(1) AS total, COUNT(col) AS col_non_null, ... FROM <tables>`

## Why

Columns with high NULL ratios can cause unexpected results due to SQL three-valued
logic (e.g. `NOT IN` against a nullable column returns no rows). The probe reveals
total row count and per-column non-null counts so the user can assess NULL impact.

## Cases

| Case | Type | Description |
|------|------|-------------|
| `pos-001-two-columns` | positive | two WHERE columns → COUNT + two `_non_null` aliases |
| `pos-002-single-column` | positive | single WHERE column → COUNT + one alias |
| `pos-003-join-condition` | positive | JOIN ON + WHERE columns combined |
| `pos-004-multiple-joins` | positive | three-way JOIN — columns from every ON condition |
| `pos-005-complex-conditions` | positive | mixed IS NULL / IN / LIKE conditions |
| `neg-001-no-where` | negative | no WHERE, no JOIN ON — no columns extracted |
| `neg-002-non-select` | negative | DELETE — rule is SELECT-only |
