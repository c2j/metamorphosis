-- Case: single GROUP BY column → skew probe with cnt ORDER BY DESC
SELECT dept, COUNT(*) FROM employees GROUP BY dept
