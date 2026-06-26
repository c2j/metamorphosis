# probe-data-skew

Generates a probe SQL showing value distribution for GROUP BY columns.

- **Category**: DataQuality
- **Safety**: Manual (generates suggestions only — never replaces original SQL)
- **Matches**: SELECT statements with at least one GROUP BY column
- **Probe output**: `SELECT col1, col2, ..., COUNT(1) AS cnt FROM <tables> [WHERE ...] GROUP BY col1, col2, ... ORDER BY cnt DESC LIMIT <default_limit>`

## Why

High concentration of rows in a few group values indicates data skew, which can
cause uneven parallel execution in distributed databases. This probe shows the
top-N most frequent values and their counts, enabling the user to assess skew risk.

## Cases

| Case | Type | Description |
|------|------|-------------|
| `pos-001-single-group-by` | positive | single GROUP BY column → cnt + ORDER BY |
| `pos-002-multiple-group-by` | positive | two GROUP BY columns → composite skew probe |
| `pos-003-group-by-with-where` | positive | GROUP BY + WHERE filter → condition preserved |
| `pos-004-group-by-with-join` | positive | GROUP BY on JOIN query → join preserved |
| `neg-001-no-group-by` | negative | no GROUP BY clause → no match |
| `neg-002-non-select` | negative | non-SELECT statement → no match |
