-- Case: param on joined table → probe preserves JOIN + WHERE
SELECT o.* FROM orders o JOIN users u ON o.user_id = u.id WHERE u.status = ? AND o.amount > 100
