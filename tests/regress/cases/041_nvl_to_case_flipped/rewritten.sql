SELECT id, CASE WHEN name IS NULL THEN name ELSE 'unknown' END FROM users;
