-- Case: two parameterized equalities — basic match
SELECT * FROM orders WHERE account_id = v_user_id AND status = v_status
