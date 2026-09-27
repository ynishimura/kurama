-- What the MySQL side of the real-database tests reads.
CREATE TABLE customers (id int PRIMARY KEY, name varchar(64) NOT NULL);
CREATE TABLE orders (
  id int PRIMARY KEY,
  customer_id int,
  status varchar(16) NOT NULL DEFAULT 'open',
  amount decimal(12,2),
  note text,
  CONSTRAINT orders_customer FOREIGN KEY (customer_id) REFERENCES customers(id)
);
CREATE VIEW open_orders AS SELECT * FROM orders WHERE status = 'open';
INSERT INTO customers VALUES (1, 'ACME'), (2, '日本語');
INSERT INTO orders VALUES
  (1, 1, 'open', 42.00, 'first'),
  (2, 2, 'paid', 1.50, NULL),
  (3, 1, 'void', 0.01, ''),
  (4, 1, 'open', 9999999999.99, 'big');

CREATE TABLE types (
  id int PRIMARY KEY,
  a_tinyint tinyint,
  a_bool tinyint(1),
  a_smallint smallint,
  a_mediumint mediumint,
  a_int int,
  a_bigint bigint,
  a_uint bigint unsigned,
  a_float float,
  a_double double,
  a_decimal decimal(40,9),
  a_varchar varchar(32),
  a_char char(4),
  a_text text,
  a_json json,
  a_enum enum('open','paid','void'),
  a_set set('a','b'),
  a_date date,
  a_time time(6),
  a_datetime datetime(6),
  a_timestamp timestamp NULL,
  a_blob blob,
  a_binary binary(3),
  a_bit bit(16),
  a_year year
);
INSERT INTO types VALUES (
  1, -1, 1, -2, -3, -4, -9223372036854775808, 18446744073709551615,
  0.1, 0.1, '12345678901234567890.123456789',
  '日本語', 'ab', '', '{"a": 1}', 'open', 'a,b',
  '2026-09-21', '12:00:00.5', '2026-09-21 12:34:56.75', '2026-09-21 12:34:56',
  UNHEX('000102'), UNHEX('000102'), b'0000000100000000', 2026
);
INSERT INTO types (id) VALUES (2);
