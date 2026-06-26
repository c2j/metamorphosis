-- Case: top-level OR with two conditions → split into UNION ALL
SELECT * FROM t WHERE a = 1 OR b = 2
