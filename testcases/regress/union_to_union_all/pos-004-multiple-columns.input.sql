-- Case: multi-column select lists across UNION
SELECT a, b FROM t1 UNION SELECT a, b FROM t2
