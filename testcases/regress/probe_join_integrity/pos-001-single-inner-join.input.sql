-- Case: single INNER JOIN → one referential integrity probe
SELECT * FROM a JOIN b ON a.id = b.aid
