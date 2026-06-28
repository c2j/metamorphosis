# extract-candidate-values

Generates probe SQL showing existing values of parameterized WHERE equality columns.

- **Category**: DataQuality
- **Safety**: Manual (generates probe suggestions only, never replaces the original SQL)
- **Matches**: Any statement (SELECT, UPDATE, DELETE, INSERT...SELECT, MERGE) with a scope
  containing **≥1** parameterized equality condition (tier-1 column)
- **Probe format**: `SELECT col1, col2, count(1) AS cnt FROM tables WHERE non_param_conds GROUP BY col1, col2 ORDER BY cnt DESC LIMIT N`

## Why

When a SQL query uses `WHERE col = :param` and the input parameter value does not
exist in the data, the query returns nothing. This probe shows what values *do*
exist (filtered by non-parameterized conditions), enabling the user to find a
valid input value.

Unlike `detect-duplicate-eq-keys`, this rule matches with just **one** parameterized
equality and produces a probe **without** `HAVING` — its purpose is discovery, not
uniqueness verification.

## Cases

| Case | Type | Description |
|------|------|-------------|
| `pos-001-single-param` | positive | One param eq — basic match |
| `pos-002-mixed-param-and-literal` | positive | Param + literal preserved in WHERE |
| `pos-003-update-statement` | positive | UPDATE with one param eq |
| `pos-004-insert-select` | positive | INSERT...SELECT with param eq |
| `pos-005-join-with-param` | positive | JOIN + param eq — JOIN preserved in probe |
| `neg-001-literal-only` | negative | Only literal conditions — no param |
| `neg-002-col-col-equality` | negative | Column = Column — no param |
| `neg-003-no-where` | negative | No WHERE clause — no conditions |
