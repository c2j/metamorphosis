-- Lowercase nvl is matched; output still contains CASE
CASE WHEN col IS NULL THEN 0 ELSE col END
!NVL
