-- Case: two degenerate BETWEEN in same WHERE — engine replaces one per iteration
SELECT * FROM t WHERE col1 BETWEEN 5 AND 5 AND col2 BETWEEN 10 AND 10
