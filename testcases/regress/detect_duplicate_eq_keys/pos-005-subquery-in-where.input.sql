-- Case: subquery in WHERE with 2+ params — generates probe from inner scope
SELECT * FROM t WHERE id IN (SELECT id FROM t2 WHERE col_a = v_a AND col_b = v_b)
