-- Case: three-way JOIN — columns extracted from every ON condition + WHERE
SELECT * FROM a JOIN b ON a.id = b.aid JOIN c ON b.id = c.bid WHERE a.status = 1
