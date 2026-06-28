-- Case: EXISTS with extra WHERE conditions in subquery preserved
SELECT * FROM orders o WHERE EXISTS (SELECT 1 FROM users u WHERE u.id = o.user_id AND u.status = 'active')
