-- Case: Column = Column — no parameter
SELECT * FROM orders o JOIN users u ON o.id = u.id WHERE o.status = u.status
