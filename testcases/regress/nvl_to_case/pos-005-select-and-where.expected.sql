-- NVL in SELECT replaced first, then NVL in WHERE on next iteration
CASE WHEN a IS NULL THEN 0 ELSE a END
CASE WHEN b IS NULL THEN 1 ELSE b END
!NVL
