-- Case: UPDATE with one param eq
UPDATE orders SET status = 'done' WHERE order_id = p_order_id AND region = 'EAST'
