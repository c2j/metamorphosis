-- Case: degenerate BETWEEN inside compound WHERE with AND
SELECT * FROM t WHERE (col BETWEEN 5 AND 5) AND other = 1
