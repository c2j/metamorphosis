-- Case: GROUP BY on JOIN query → join preserved in probe
SELECT d.dept_name, COUNT(*) FROM departments d JOIN employees e ON d.id = e.dept_id GROUP BY d.dept_name
