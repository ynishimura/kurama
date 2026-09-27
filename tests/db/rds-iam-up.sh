#!/usr/bin/env bash
# Create the throwaway RDS MySQL, RDS PostgreSQL and bastion of rds-iam.yaml,
# and the database users that authenticate with IAM. One stack, so
# `rds-iam-down.sh` removes all of it.
#
#   AWS_PROFILE=<a sandbox> tests/db/rds-iam-up.sh
#
# It costs about $0.06 an hour: two db.t4g.micro, their 20GiB volumes, a
# t4g.nano and its public IPv4. Creating it takes about ten minutes, nearly all
# of them RDS. Delete it when the check is done.
#
# The role that later connects needs `rds-db:connect` on the two `iam_reader`
# users; the template grants it to nobody, so a sandbox administrator has it
# and any other role has to be given it.
#
# It needs `uv`: the users are created by a few lines of Python, as the master
# user whose password RDS keeps in Secrets Manager. No password is printed.
set -euo pipefail

# Everything after the deploy can fail with the stack already up and billing,
# and a script that ends on its first error says nothing about that.
trap 'status=$?; [ "$status" -eq 0 ] || echo "failed: the stack may exist and bill; remove it with tests/db/rds-iam-down.sh" >&2' EXIT
here="$(cd "$(dirname "$0")" && pwd)"
stack=${KURAMA_RDS_IAM_STACK:-kurama-rds-iam-test}
ca=${KURAMA_RDS_CA:-$here/rds-global-bundle.pem}

started=$(date +%s)
say() { printf '[%3ds] %s\n' "$(($(date +%s) - started))" "$*"; }

# The databases take one address besides the bastion: this one.
address=$(curl -fsS https://checkip.amazonaws.com | tr -d '[:space:]')
image=$(aws ec2 describe-images --owners amazon \
  --filters 'Name=name,Values=al2023-ami-2023.*-kernel-6.1-arm64' \
            'Name=state,Values=available' \
  --query 'reverse(sort_by(Images,&CreationDate))[0].ImageId' --output text)
[ -n "$image" ] && [ "$image" != "None" ] || { echo "no Amazon Linux 2023 image" >&2; exit 1; }

say "creating the stack (about ten minutes, nearly all of them RDS)"
aws cloudformation deploy \
  --stack-name "$stack" \
  --template-file "$here/rds-iam.yaml" \
  --parameter-overrides "ClientCidr=$address/32" "Image=$image" \
  --capabilities CAPABILITY_IAM \
  --no-fail-on-empty-changeset

output() {
  aws cloudformation describe-stacks --stack-name "$stack" \
    --query "Stacks[0].Outputs[?OutputKey==\`$1\`].OutputValue" --output text
}
MYSQL_HOST=$(output MysqlHost)
PG_HOST=$(output PostgresHost)
MYSQL_SECRET=$(output MysqlSecret)
PG_SECRET=$(output PostgresSecret)
export MYSQL_HOST PG_HOST MYSQL_SECRET PG_SECRET

# RDS certificates are signed by Amazon's RDS authority, which no system trust
# store carries; `ca_file` is how a section names it.
[ -f "$ca" ] || curl -fsS -o "$ca" https://truststore.pki.rds.amazonaws.com/global/global-bundle.pem
export CA="$ca"

say "creating the users that authenticate with IAM"
uv run --quiet --with 'psycopg[binary]' --with pymysql python - <<'PYTHON'
import json, os, subprocess
import psycopg, pymysql

def secret(arn):
    text = subprocess.check_output(["aws", "secretsmanager", "get-secret-value",
        "--secret-id", arn, "--query", "SecretString", "--output", "text"])
    return json.loads(text)

ca = os.environ["CA"]
master = secret(os.environ["MYSQL_SECRET"])
connection = pymysql.connect(host=os.environ["MYSQL_HOST"], user=master["username"],
    password=master["password"], database="app", ssl={"ca": ca}, autocommit=True)
with connection.cursor() as cursor:
    for sql in [
        "CREATE USER IF NOT EXISTS iam_reader IDENTIFIED WITH AWSAuthenticationPlugin AS 'RDS'",
        "ALTER USER iam_reader REQUIRE SSL",
        "CREATE TABLE IF NOT EXISTS orders (id INT PRIMARY KEY, status VARCHAR(20))",
        "REPLACE INTO orders VALUES (1,'paid'),(2,'paid'),(3,'open')",
        "GRANT SELECT, INSERT, DELETE ON app.* TO iam_reader",
    ]:
        cursor.execute(sql)

master = secret(os.environ["PG_SECRET"])
with psycopg.connect(host=os.environ["PG_HOST"], user=master["username"],
        password=master["password"], dbname="app", sslmode="verify-full",
        sslrootcert=ca, autocommit=True) as connection:
    for sql in [
        "DO $$ BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='iam_reader') "
        "THEN CREATE USER iam_reader; END IF; END $$",
        "GRANT rds_iam TO iam_reader",
        "CREATE TABLE IF NOT EXISTS orders (id INT PRIMARY KEY, status TEXT)",
        "INSERT INTO orders VALUES (1,'paid'),(2,'paid'),(3,'open') ON CONFLICT DO NOTHING",
        "GRANT SELECT, INSERT, DELETE ON orders TO iam_reader",
    ]:
        connection.execute(sql)
PYTHON
say "iam_reader exists on both"

# The first IAM login to a new database has taken longer than kurama's
# 10-second connect timeout, twice, while the master password logins above had
# just worked; later ones answer at once. So the stack is up only once
# iam_reader has logged in to each with a token, within five minutes.
say "waiting for the first IAM login to each database (up to five minutes)"
STARTED=$started uv run --quiet --with 'psycopg[binary]' --with pymysql python - <<'PYTHON'
import os, subprocess, sys, time
import psycopg, pymysql

ca = os.environ["CA"]
started = int(os.environ["STARTED"])
deadline = time.time() + 300

def say(text):
    print(f"[{int(time.time()) - started:3d}s] {text}", flush=True)

def token(host, port):
    return subprocess.check_output(["aws", "rds", "generate-db-auth-token",
        "--hostname", host, "--port", str(port), "--username", "iam_reader",
        "--region", host.split(".")[2]], text=True).strip()

def postgres(host):
    psycopg.connect(host=host, user="iam_reader", password=token(host, 5432),
        dbname="app", sslmode="verify-full", sslrootcert=ca, connect_timeout=10).close()

def mysql(host):
    pymysql.connect(host=host, user="iam_reader", password=token(host, 3306),
        database="app", ssl={"ca": ca}, connect_timeout=10).close()

for name, login, host in [("PostgreSQL", postgres, os.environ["PG_HOST"]),
                          ("MySQL", mysql, os.environ["MYSQL_HOST"])]:
    while True:
        try:
            login(host)
            say(f"iam_reader logged in to {name}")
            break
        except Exception as error:
            if time.time() > deadline:
                sys.exit(f"iam_reader could not log in to {name} with a token "
                         f"within five minutes: {error}")
            say(f"no IAM login to {name} yet: {error}")
            time.sleep(10)
PYTHON

cat <<SECTIONS

both databases are up. Point [db.*] sections at them:

  [db.rds-pg]                    # and the same with engine = "mysql"
  engine = "postgresql"
  host = "$PG_HOST"              # mysql: $MYSQL_HOST
  database = "app"
  username = "iam_reader"
  ca_file = "$ca"

  [db.rds-pg.iam]
  aws_profile = "<the profile this ran with>"

and, to go through the bastion instead, add tls = "verify-ca" and:

  [db.rds-pg.tunnel]
  kind = "ssm"
  aws_profile = "<the profile this ran with>"
  instance_name = "$stack"

then delete it:  tests/db/rds-iam-down.sh
SECTIONS
