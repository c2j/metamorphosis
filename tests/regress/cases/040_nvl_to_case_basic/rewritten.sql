SELECT id, CASE WHEN name IS NULL THEN 'unknown' ELSE name END FROM users;
