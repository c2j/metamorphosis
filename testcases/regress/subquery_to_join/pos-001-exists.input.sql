-- Case: EXISTS subquery → INNER JOIN
SELECT * FROM orders o WHERE EXISTS (SELECT 1 FROM users u WHERE u.id = o.user_id)
