-- What the PostgreSQL side of the real-database tests reads.
-- `types` carries one row per shape the binary readers have to get right.
CREATE TABLE customers (id integer PRIMARY KEY, name text NOT NULL);
CREATE TABLE orders (
  id integer PRIMARY KEY,
  customer_id integer REFERENCES customers(id),
  status text NOT NULL DEFAULT 'open',
  amount numeric(12,2),
  note text
);
CREATE VIEW open_orders AS SELECT * FROM orders WHERE status = 'open';
INSERT INTO customers VALUES (1, 'ACME'), (2, '日本語');
INSERT INTO orders VALUES
  (1, 1, 'open', 42.00, 'first'),
  (2, 2, 'paid', 1.50, NULL),
  (3, 1, 'void', 0.01, ''),
  (4, 1, 'open', 9999999999.99, 'big');

CREATE TABLE types (
  id integer PRIMARY KEY,
  a_bool boolean,
  a_int2 smallint,
  a_int4 integer,
  a_int8 bigint,
  a_float4 real,
  a_float8 double precision,
  a_numeric numeric,
  a_text text,
  a_varchar varchar(20),
  a_bpchar char(4),
  a_uuid uuid,
  a_json json,
  a_jsonb jsonb,
  a_date date,
  a_time time,
  a_timestamp timestamp,
  a_timestamptz timestamptz,
  a_bytea bytea,
  a_int4_array integer[],
  a_text_array text[],
  a_matrix integer[][]
);
INSERT INTO types VALUES (
  1, true, -32768, -1, 9223372036854775807, 0.1, 0.1,
  '12345678901234567890.123456789',
  '日本語', '', 'ab',
  '12345678-9abc-def0-1234-56789abcdef0',
  '{"a": 1}', '{"a": 1}',
  DATE '1999-12-31', TIME '12:00:00.5',
  TIMESTAMP '2026-09-21 12:34:56.75',
  TIMESTAMPTZ '2026-09-21 12:34:56+00',
  '\x000102'::bytea,
  ARRAY[1, NULL, 3],
  ARRAY['a,b', '', 'NULL', 'plain'],
  ARRAY[[1,2],[3,4]]
);
-- Everything null, so a null of every type is read as a null.
INSERT INTO types (id) VALUES (2);
-- The values no decimal type holds.
INSERT INTO types (id, a_numeric, a_float8) VALUES (3, 'NaN', 'NaN');

-- A database whose `pg_cancel_backend` answers false, the answer PostgreSQL
-- gives when the backend it names is not there to signal. No client can make
-- a real backend vanish between the deadline and the cancel, so the role that
-- reads it finds this function before pg_catalog's; everything else it calls
-- is pg_catalog's own. A database of its own keeps it out of every listing of
-- `app`.
CREATE DATABASE cancel_refused;
CREATE ROLE kurama_cancel_refused LOGIN PASSWORD 'fake-client-secret';
ALTER ROLE kurama_cancel_refused IN DATABASE cancel_refused
  SET search_path = cancel_refused, pg_catalog;
\connect cancel_refused
CREATE SCHEMA cancel_refused;
CREATE FUNCTION cancel_refused.pg_cancel_backend(integer) RETURNS boolean
  LANGUAGE sql AS 'SELECT false';
GRANT USAGE ON SCHEMA cancel_refused TO kurama_cancel_refused;
