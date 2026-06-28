-- Case: UPDATE with two parameterized equalities
UPDATE orders SET x = 1 WHERE col_a = v_a AND col_b = v_b AND region = 'EAST'
