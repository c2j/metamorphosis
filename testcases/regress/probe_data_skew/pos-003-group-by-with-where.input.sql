-- Case: GROUP BY with WHERE filter → filter preserved in probe
SELECT dept, COUNT(*) FROM employees WHERE status = 'active' GROUP BY dept
