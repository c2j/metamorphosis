-- Case: JOIN with WHERE condition → integrity probe still generated
SELECT * FROM a JOIN b ON a.id = b.aid WHERE a.status = 1
