-- Probe must GROUP BY the param column; no HAVING
GROUP BY
COUNT(1) AS cnt
ORDER BY
status
!HAVING
