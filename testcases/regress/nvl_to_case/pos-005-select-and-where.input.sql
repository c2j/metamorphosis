-- Case: NVL in both SELECT target and WHERE — engine replaces across locations
SELECT NVL(a, 0) FROM t WHERE NVL(b, 1) = 1
