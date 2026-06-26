-- Case: subquery with GROUP BY — safety guard prevents rewrite
SELECT * FROM orders o WHERE o.user_id IN (SELECT u.id FROM users u GROUP BY u.id HAVING COUNT(*) > 0)
