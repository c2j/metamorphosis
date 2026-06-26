-- Case: NOT IN (SELECT ...) → LEFT JOIN + IS NULL
SELECT * FROM orders o WHERE o.user_id NOT IN (SELECT u.id FROM users u)
