-- Case: NVL inside WHERE clause
SELECT * FROM t WHERE NVL(status, 'X') = 'Y'
