CREATE TABLE users (
    id INTEGER,
    name VARCHAR(100)
);
CREATE TABLE orders (
    id INTEGER,
    user_id INTEGER,
    amount INTEGER
);
