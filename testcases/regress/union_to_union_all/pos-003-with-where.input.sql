-- Case: UNION of two SELECTs with WHERE clauses; conditions should be preserved
SELECT * FROM t WHERE a > 1 UNION SELECT * FROM t WHERE a < 10
