-- Case: IN (SELECT ...) → INNER JOIN
SELECT * FROM orders o WHERE o.user_id IN (SELECT u.id FROM users u)
