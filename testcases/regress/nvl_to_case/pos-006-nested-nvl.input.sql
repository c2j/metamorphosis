-- Case: nested NVL(NVL(col, 0), -1) — outer replaced first, inner(s) on subsequent iterations
SELECT NVL(NVL(col, 0), -1) FROM t
