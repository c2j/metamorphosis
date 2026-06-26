# eliminate-select-star

Replaces `SELECT *` (and `SELECT t.*`) with explicit column names from the schema map.

- **Category**: Semantic
- **Safety**: Safe (semantically equivalent when schema is accurate)
- **Matches**: SELECT statements containing `SELECT *` or `SELECT t.*` in the target list
- **Requires**: A schema map (table → column → type) to resolve column names

## Why

`SELECT *` is fragile — adding or reordering columns in the table changes the
result set shape. Explicit column names make queries self-documenting and
prevent unexpected schema drift.

## Schema

Schema is provided via `_schema.json` in this directory. The harness loads it
automatically and passes it as `ctx.schema`.

## Full-Match Limitation

This rule expands `SELECT *` by iterating the schema HashMap, whose column
order is non-deterministic. Therefore `.full.sql` files are omitted — the
fragment-based `.expected.sql` checks (column presence, not order) are
sufficient.

## Cases

| Case | Type | Description |
|------|------|-------------|
| `pos-001-basic` | positive | `SELECT *` → expanded columns (id, name, email) |
| `pos-002-mixed-star-and-column` | positive | `SELECT *, status` — star + explicit column |
| `neg-001-no-wildcard` | negative | `SELECT id, name` — no `*` present |
| `neg-002-non-select` | negative | DELETE statement — rule is SELECT-only |
