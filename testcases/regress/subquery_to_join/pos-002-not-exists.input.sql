-- Case: NOT EXISTS → LEFT JOIN + IS NULL
SELECT * FROM orders o WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.id = o.user_id)
