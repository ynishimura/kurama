#!/usr/bin/env bash
# Start the databases `KURAMA_TEST_DB=1` tests read, with TLS on, and load the
# fixture they expect. `cargo xtask db-up` runs this; `down.sh` removes them.
#
# There is no compose file on purpose: three `docker run` lines need no plugin,
# and the readiness wait has to be here either way.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
tls="$here/tls"
names=(kurama-pg17 kurama-pg18 kurama-mysql84)
PG17_PORT=${KURAMA_TEST_PG17_PORT:-55432}
PG18_PORT=${KURAMA_TEST_PG18_PORT:-55433}
MYSQL_PORT=${KURAMA_TEST_MYSQL_PORT:-55306}
PASSWORD=fake-client-secret   # what tests/fakes/op prints for `op read`

docker() { command docker "$@"; }

# A throwaway CA and one server certificate, for `tls = "verify-ca"`. The
# certificate deliberately carries a name nothing connects by, which is the
# situation verify-ca exists for.
mkdir -p "$tls"
if [ ! -f "$tls/server.crt" ]; then
  openssl req -x509 -newkey rsa:2048 -sha256 -days 365 -nodes \
    -keyout "$tls/ca.key" -out "$tls/ca.pem" -subj "/CN=kurama-test-ca" 2>/dev/null
  openssl req -newkey rsa:2048 -nodes -keyout "$tls/server.key" \
    -out "$tls/server.csr" -subj "/CN=db.test" 2>/dev/null
  openssl x509 -req -in "$tls/server.csr" -CA "$tls/ca.pem" -CAkey "$tls/ca.key" \
    -CAcreateserial -out "$tls/server.crt" -days 365 -sha256 2>/dev/null
  rm -f "$tls/server.csr"
fi

for name in "${names[@]}"; do
  docker rm -f "$name" >/dev/null 2>&1 || true
done

start_postgres() {
  local name=$1 image=$2 port=$3
  docker run -d --name "$name" \
    -e POSTGRES_PASSWORD="$PASSWORD" -e POSTGRES_USER=kurama -e POSTGRES_DB=app \
    -p "127.0.0.1:$port:5432" "$image" >/dev/null
  # The key has to belong to the server and be unreadable by anyone else, so it
  # is placed after the entrypoint created the user. Readiness is asked over
  # TCP: the entrypoint first runs a socket-only server to create the user,
  # and a check on the socket answers for that one, which then goes away
  # under the next statement.
  until docker exec "$name" pg_isready -h 127.0.0.1 -U kurama -d app >/dev/null 2>&1; do sleep 1; done
  docker cp "$tls/server.crt" "$name:/var/lib/postgresql/server.crt"
  docker cp "$tls/server.key" "$name:/var/lib/postgresql/server.key"
  docker exec -u root "$name" sh -c \
    'chown postgres:postgres /var/lib/postgresql/server.* && chmod 600 /var/lib/postgresql/server.key'
  # `ALTER SYSTEM` cannot share a transaction, so each one is its own call.
  for setting in \
    "ssl = on" \
    "ssl_cert_file = '/var/lib/postgresql/server.crt'" \
    "ssl_key_file = '/var/lib/postgresql/server.key'"; do
    docker exec "$name" psql -U kurama -d app -v ON_ERROR_STOP=1 \
      -c "ALTER SYSTEM SET $setting" >/dev/null
  done
  docker restart "$name" >/dev/null
  until docker exec "$name" pg_isready -h 127.0.0.1 -U kurama -d app >/dev/null 2>&1; do sleep 1; done
  docker exec -i "$name" psql -U kurama -d app -v ON_ERROR_STOP=1 -q < "$here/postgres.sql"
}

start_postgres kurama-pg17 postgres:17 "$PG17_PORT"
start_postgres kurama-pg18 postgres:18 "$PG18_PORT"

# MySQL generates its own CA and server certificate at first start, which is
# exactly the chain `verify-ca` should accept.
docker run -d --name kurama-mysql84 \
  -e MYSQL_ROOT_PASSWORD="$PASSWORD" -e MYSQL_USER=kurama \
  -e MYSQL_PASSWORD="$PASSWORD" -e MYSQL_DATABASE=app \
  -p "127.0.0.1:$MYSQL_PORT:3306" mysql:8.4 \
  --require_secure_transport=ON >/dev/null
# The fixture is loaded as root over the local socket: `kurama` exists only as
# `kurama@%`, and TCP into this server has to be TLS. Readiness is a statement
# over TCP: the entrypoint first runs a server with networking off to create
# the user and the database, and a statement on the socket answers for that
# one -- a fixture loaded into it is gone when the real server starts.
until docker exec -e MYSQL_PWD="$PASSWORD" kurama-mysql84 \
  mysql -h 127.0.0.1 --protocol=tcp -uroot -e "SELECT 1" app >/dev/null 2>&1; do
  sleep 1
done
docker cp kurama-mysql84:/var/lib/mysql/ca.pem "$tls/mysql-ca.pem"
# utf8mb4 on the way in: the client default would store UTF-8 as latin1.
docker exec -i -e MYSQL_PWD="$PASSWORD" kurama-mysql84 \
  mysql -uroot --default-character-set=utf8mb4 app < "$here/mysql.sql"

cat <<EOF
databases are up:
  postgres 17  127.0.0.1:$PG17_PORT   ca $tls/ca.pem
  postgres 18  127.0.0.1:$PG18_PORT   ca $tls/ca.pem
  mysql 8.4    127.0.0.1:$MYSQL_PORT  ca $tls/mysql-ca.pem
run the tests with: KURAMA_TEST_DB=1 cargo test --locked --features test-fakes --test real_db
EOF
