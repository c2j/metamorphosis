-- Case: JOIN ON condition + WHERE columns combined into one probe
SELECT * FROM orders o JOIN users u ON o.user_id = u.id WHERE o.status = 'active'
