-- Case: SELECT DISTINCT blocks the rewrite
SELECT DISTINCT * FROM t WHERE a = 1 OR b = 2
