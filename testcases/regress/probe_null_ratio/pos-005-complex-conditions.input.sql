-- Case: mixed condition types — IS NULL, IN list, LIKE
SELECT * FROM t WHERE col1 IS NULL AND col2 IN (1, 2, 3) AND col3 LIKE '%test%'
