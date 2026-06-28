-- Case: two JDBC params in WHERE → probe for both columns
SELECT * FROM orders WHERE status = ? AND type = ?
