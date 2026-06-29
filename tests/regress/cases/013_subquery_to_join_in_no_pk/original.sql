SELECT id, user_id, amount FROM orders WHERE user_id IN (SELECT id FROM users);
