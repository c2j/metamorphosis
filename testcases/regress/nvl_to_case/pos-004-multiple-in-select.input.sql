-- Case: multiple NVL in SELECT targets — engine must iterate until all replaced
SELECT NVL(col1, 0), NVL(col2, 'x') FROM t
