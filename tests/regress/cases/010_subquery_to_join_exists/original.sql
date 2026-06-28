SELECT id, user_id, amount FROM orders WHERE EXISTS (SELECT 1 FROM users WHERE users.id = orders.user_id);
