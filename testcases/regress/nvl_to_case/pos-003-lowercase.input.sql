-- Case: lowercase nvl should be matched (case-insensitive function name)
SELECT nvl(col, 0) FROM t
