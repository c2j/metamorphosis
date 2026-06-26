-- Case: literals preserved in WHERE while params go to GROUP BY
SELECT * FROM t WHERE type = '4' AND status = v_status AND region = v_region
