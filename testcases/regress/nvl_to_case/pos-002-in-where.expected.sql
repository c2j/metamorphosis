-- NVL in WHERE should be replaced; status IS NULL branch must appear
CASE WHEN status IS NULL THEN 'X' ELSE status END
!NVL
