-- Case: single JDBC param in WHERE → MIN/MAX/COUNT(DISTINCT) probe
SELECT * FROM t WHERE status = ?
