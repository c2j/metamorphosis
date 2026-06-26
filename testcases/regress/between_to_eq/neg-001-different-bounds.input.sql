-- Case: bounds differ (1 AND 10) — should NOT be rewritten
SELECT * FROM t WHERE col BETWEEN 1 AND 10
