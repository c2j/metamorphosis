-- Case: SELECT * mixed with an explicit column — star expands, extra column preserved
SELECT *, status FROM users WHERE id = 1
