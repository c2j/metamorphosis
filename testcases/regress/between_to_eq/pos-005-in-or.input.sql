-- Case: degenerate BETWEEN inside OR — not just AND compounds
SELECT * FROM t WHERE (col BETWEEN 5 AND 5) OR other = 1
