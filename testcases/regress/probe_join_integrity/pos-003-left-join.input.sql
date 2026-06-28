-- Case: LEFT JOIN → probe still generates integrity check
SELECT * FROM a LEFT JOIN b ON a.id = b.aid
