# union-to-union-all

Converts `UNION` to `UNION ALL` to skip implicit deduplication when order
or duplicates are not a concern.

- **Category**: Performance
- **Safety**: Safe (purely syntactic transformation — no schema dependency)
- **Matches**: SELECT statements whose outermost set operation is `UNION`
  (without `ALL`); INTERSECT and EXCEPT are ignored
- **Replacement scope**: one `UNION` per engine iteration; the engine re-runs
  for chained operations

## Why

`UNION` performs implicit deduplication (sort or hash). `UNION ALL` skips
deduplication and is faster. When duplicates are guaranteed absent or
irrelevant, `UNION ALL` is preferred.

## Cases

| Case | Type | Description |
|------|------|-------------|
| `pos-001-basic` | positive | simple two-way `UNION` |
| `pos-003-with-where` | positive | `UNION` of SELECTs with WHERE clauses preserved |
| `pos-004-multiple-columns` | positive | multi-column select lists across `UNION` |
| `neg-001-already-union-all` | negative | already `UNION ALL` — unchanged |
| `neg-002-intersect` | negative | `INTERSECT` — not a UNION operation |
| `neg-003-except` | negative | `EXCEPT` — not a UNION operation |
| `neg-004-non-select` | negative | non-SELECT statement — no match |
