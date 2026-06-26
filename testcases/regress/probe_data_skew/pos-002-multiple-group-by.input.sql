-- Case: two GROUP BY columns → composite skew probe
SELECT dept, role, COUNT(*) FROM employees GROUP BY dept, role
