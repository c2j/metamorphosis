-- Case: DELETE statement — rule is SELECT-only
DELETE FROM orders WHERE EXISTS (SELECT 1 FROM users WHERE id = 1)
