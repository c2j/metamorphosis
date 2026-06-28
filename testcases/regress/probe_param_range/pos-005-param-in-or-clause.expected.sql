-- Probe must reference column t.b (the param-equality column)
MIN
MAX
COUNT(DISTINCT
total
-- OR condition referencing ? must NOT appear
!OR
!IS NULL
!?
-- Literal condition t.a = '1' preserved
t.a = '1'
