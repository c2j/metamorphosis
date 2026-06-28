-- Case: INSERT...SELECT with param eq
INSERT INTO archive SELECT id, name FROM orders WHERE status = v_status
