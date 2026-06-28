# between-to-eq

Rewrites `col BETWEEN v AND v` to `col = v` when low and high bounds are equal.

- **Category**: Semantic
- **Safety**: Safe (semantically equivalent — degenerate range is a point lookup)
- **Matches**: non-negated `BETWEEN` with `low == high` in the WHERE clause of a SELECT
- **Replacement scope**: first degenerate BETWEEN per iteration; engine re-runs

## Why

`BETWEEN x AND x` may trigger a range scan; `= x` enables a point lookup via
index.

## Cases

| Case | Type | Description |
|------|------|-------------|
| `pos-001-integer` | positive | integer bounds `BETWEEN 5 AND 5` |
| `pos-002-string` | positive | string literal bounds `BETWEEN 'a' AND 'a'` |
| `pos-003-compound` | positive | degenerate BETWEEN inside compound WHERE with AND |
| `pos-004-multiple-between` | positive | two degenerate BETWEEN — engine iterates |
| `pos-005-in-or` | positive | degenerate BETWEEN inside OR expression |
| `neg-001-different-bounds` | negative | `BETWEEN 1 AND 10` — bounds differ, preserved |
| `neg-002-not-between` | negative | `NOT BETWEEN 5 AND 5` — negated, preserved |
| `neg-003-no-where` | negative | no WHERE clause — no match |
