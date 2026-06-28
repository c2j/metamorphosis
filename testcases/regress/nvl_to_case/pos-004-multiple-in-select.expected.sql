-- Both NVL replaced; no NVL remains after engine iterations
CASE WHEN col1 IS NULL THEN 0 ELSE col1 END
CASE WHEN col2 IS NULL THEN 'x' ELSE col2 END
!NVL
