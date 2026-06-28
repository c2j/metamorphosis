-- Case: JOIN with param eq — JOIN preserved in probe
SELECT o.* FROM orders o JOIN users u ON o.user_id = u.id WHERE u.status = v_status AND o.amount > 100
