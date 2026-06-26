-- Case: param + literal conditions — literal preserved in WHERE
SELECT t.special_sql FROM t WHERE t.clear_type = '4' AND t.task_status = p_ts
