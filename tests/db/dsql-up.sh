#!/usr/bin/env bash
# Create the throwaway Aurora DSQL cluster of dsql.yaml and return when it is
# ACTIVE, which is when a token can open it. One stack, so `dsql-down.sh`
# removes all of it.
#
#   AWS_PROFILE=<a sandbox> tests/db/dsql-up.sh
#
# What it costs: Aurora DSQL bills DPUs (compute, per request) and storage,
# with a free tier of 100,000 DPU-hours and 1GB a month; an idle cluster
# with the few rows a check writes is about $0 an hour. It is deleted when the
# check is done all the same. Creating it takes two to five minutes.
#
# Every wait says what it is waiting for, how long it has waited and when it
# will give up: a script that prints nothing for four minutes is a script
# nobody lets finish.
set -euo pipefail

# A failure after the deploy leaves the stack up, and a script that ends on
# its first error says nothing about that.
trap 'status=$?; [ "$status" -eq 0 ] || echo "failed: the stack may exist; remove it with tests/db/dsql-down.sh" >&2' EXIT
here="$(cd "$(dirname "$0")" && pwd)"
stack=${KURAMA_DSQL_STACK:-kurama-dsql-test}
active_deadline=${KURAMA_DSQL_ACTIVE_SECONDS:-600}

started=$(date +%s)
say() { printf '[%3ds] %s\n' "$(($(date +%s) - started))" "$*"; }

say "creating the stack (two to five minutes)"
aws cloudformation deploy \
  --stack-name "$stack" \
  --template-file "$here/dsql.yaml" \
  --no-fail-on-empty-changeset

output() {
  aws cloudformation describe-stacks --stack-name "$stack" \
    --query "Stacks[0].Outputs[?OutputKey==\`$1\`].OutputValue" --output text
}
identifier=$(output Identifier)
endpoint=$(output Endpoint)
say "cluster $identifier at $endpoint"

# CloudFormation reports the stack complete once the cluster exists; whether
# it answers is its own status, so it is polled with its own clock.
say "waiting for the cluster to be ACTIVE (up to ${active_deadline}s)"
until [ "$(aws dsql get-cluster --identifier "$identifier" --query status --output text 2>/dev/null)" = "ACTIVE" ]; do
  if [ $(($(date +%s) - started)) -gt "$active_deadline" ]; then
    say "the cluster never became ACTIVE: $(aws dsql get-cluster --identifier "$identifier" --query status --output text 2>&1 || true)"
    exit 1
  fi
  sleep 10
done
say "the cluster is ACTIVE"

cat <<EOF

the cluster is up. Point a [db.*] section at it, with <profile> the AWS
profile this ran with -- the host names the cluster, so the section takes
IAM only, and DSQL's own token instead of RDS's:

  [db.dsql]
  engine = "postgresql"
  host = "$endpoint"
  database = "postgres"
  username = "admin"           # DbConnectAdmin; a role mapped with AWS IAM GRANT is DbConnect
  tls = "verify-full"

  [db.dsql.iam]
  aws_profile = "<profile>"

then check that the token is what it answers:

  kurama db dsql --query 'SELECT 1 AS one' --json
  kurama db dsql --tables --json

and delete it:  tests/db/dsql-down.sh
EOF
