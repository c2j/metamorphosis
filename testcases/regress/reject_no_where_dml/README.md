# reject-no-where-dml

Generates a critical warning when an `UPDATE` or `DELETE` has no WHERE clause.

- **Category**: Safety
- **Safety**: Manual (generates suggestions only — never replaces the original SQL)
- **Matches**: DELETE or UPDATE statements without a WHERE clause; also matches
  TRUNCATE (which may be a converted DELETE from another rule)
- **Replacement scope**: none — produces a `Suggest` action with a critical-severity message

## Why

`UPDATE` or `DELETE` without a WHERE clause affects every row in the table.
This is often unintentional and can cause data loss. The rule emits a critical
suggestion to alert the user before execution.

## Cases

| Case | Type | Description |
|------|------|-------------|
| `pos-001-delete` | positive | DELETE without WHERE → critical suggestion |
| `pos-002-update` | positive | UPDATE without WHERE → critical suggestion |
| `neg-001-delete-with-where` | negative | DELETE with WHERE → no suggestion |
| `neg-002-update-with-where` | negative | UPDATE with WHERE → no suggestion |
| `neg-003-select` | negative | SELECT — not DML, no suggestion |
