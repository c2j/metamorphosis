-- Case: specific column projection preserved after OR-to-UNION ALL rewrite
SELECT id, name FROM t WHERE status = 'active' OR status = 'pending'
