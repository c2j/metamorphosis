-- Case: two JOINs → two separate probes generated
SELECT * FROM a JOIN b ON a.id = b.aid JOIN c ON b.id = c.bid
