-- Case: only literal equalities — no parameterized columns
SELECT * FROM t WHERE status = 1 AND type = 'A'
