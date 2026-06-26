-- Case: subquery with JOIN — safety guard prevents rewrite
SELECT * FROM orders o WHERE EXISTS (SELECT 1 FROM users u JOIN addresses a ON u.id = a.user_id WHERE u.id = o.user_id)
