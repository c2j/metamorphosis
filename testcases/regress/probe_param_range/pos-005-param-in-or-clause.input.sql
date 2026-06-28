-- Case: param inside OR clause → tier1 extraction + OR condition filtered from probe WHERE
SELECT * FROM t WHERE (? IS NULL OR t.b = ?) AND t.a = '1'
