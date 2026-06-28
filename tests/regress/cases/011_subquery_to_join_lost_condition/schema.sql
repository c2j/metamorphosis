CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    name VARCHAR(100),
    status VARCHAR(20)
);
CREATE TABLE orders (
    id INTEGER PRIMARY KEY,
    user_id INTEGER,
    amount INTEGER
);
