SELECT id, status FROM users WHERE id = 1
UNION ALL
SELECT id, status FROM users WHERE id <= 2;
