-- Case: single column in WHERE → COUNT probe with one _non_null alias
SELECT * FROM t WHERE status = 1
