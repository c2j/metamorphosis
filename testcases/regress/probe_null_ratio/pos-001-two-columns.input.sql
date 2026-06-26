-- Case: two columns in WHERE → COUNT probe with total + two _non_null aliases
SELECT * FROM t WHERE col1 = 1 AND col2 = 2
