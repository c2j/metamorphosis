# detect-duplicate-eq-keys

Detects candidate keys from equality conditions in WHERE clauses and generates
a GROUP BY probe SQL to verify uniqueness.

- **Category**: DataQuality
- **Safety**: Manual (generates probe suggestions only, never replaces the original SQL)
- **Matches**: Any statement (SELECT, UPDATE, DELETE, INSERT...SELECT, MERGE) with a scope
  containing **≥2** parameterized equality conditions (tier-1 columns)
- **Probe format**: `SELECT col1, col2, count(1) AS cnt FROM tables WHERE keep_conds GROUP BY col1, col2 HAVING count(1) > 1 ORDER BY cnt DESC LIMIT N`

## Why

When a WHERE clause uses two or more equality conditions (e.g. `WHERE tenant_id = :t AND user_id = :u`),
those columns are candidate keys / unique identifiers. This probe checks whether
the combination is actually unique in the data — if any `cnt > 1`, the candidate
key has duplicates, which may indicate a data quality issue.

## Cases

| Case | Type | Description |
|------|------|-------------|
| `pos-001-two-params` | positive | Two parameterized equalities — basic match |
| `pos-002-three-params` | positive | Three parameterized equalities — composite key probe |
| `pos-003-mixed-literal-and-params` | positive | Params in tier1, literal conditions preserved in WHERE |
| `pos-004-update-statement` | positive | UPDATE statement with 2+ param eqs |
| `pos-005-subquery-in-where` | positive | Subquery with 2+ param eqs generates probe |
| `neg-001-single-param` | negative | Only one param eq — needs ≥2 |
| `neg-002-literal-only` | negative | All literal equalities — no param |
| `neg-003-no-where` | negative | No WHERE clause — no conditions at all |
