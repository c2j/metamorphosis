-- Case: JOIN in FROM blocks the rewrite
SELECT * FROM t1 JOIN t2 ON t1.id = t2.id WHERE t1.a = 1 OR t2.b = 2
