-- Case: param + literal equality → literal preserved in probe WHERE
SELECT * FROM t WHERE status = ? AND category = 'A'
