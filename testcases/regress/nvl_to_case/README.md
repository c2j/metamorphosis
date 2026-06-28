# nvl-to-case

Rewrites `NVL(a, b)` to `CASE WHEN a IS NULL THEN b ELSE a END`.

- **Category**: Semantic
- **Safety**: Safe (semantically equivalent)
- **Matches**: SELECT statements containing `NVL()` in SELECT targets or WHERE clause
- **Replacement scope**: one NVL per engine iteration; engine re-runs until clean

## Why

`NVL` is Oracle / openGauss-specific. `CASE WHEN` is standard SQL and allows
the optimiser to pick index access paths on `a`.

## Cases

| Case | Type | Description |
|------|------|-------------|
| `pos-001-basic` | positive | NVL in SELECT target (**exact mode**) |
| `pos-002-in-where` | positive | NVL inside WHERE clause |
| `pos-003-lowercase` | positive | lowercase `nvl` matched (case-insensitive) |
| `pos-004-multiple-in-select` | positive | two NVL in SELECT targets — engine iterates |
| `pos-005-select-and-where` | positive | NVL in both SELECT and WHERE |
| `pos-006-nested-nvl` | positive | `NVL(NVL(col, 0), -1)` — nested replacement |
| `neg-001-no-nvl` | negative | no NVL present — no rewrite |
| `neg-002-non-select` | negative | DELETE statement — rule is SELECT-only |
