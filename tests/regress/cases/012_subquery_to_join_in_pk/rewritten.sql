SELECT orders.id, orders.user_id, orders.amount FROM orders INNER JOIN users ON users.id = orders.user_id;
