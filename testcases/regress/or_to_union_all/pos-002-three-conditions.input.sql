-- Case: three conditions chained with OR → engine iterates to split all
SELECT a FROM t WHERE a = 1 OR b = 2 OR c = 3
