#!/usr/bin/env bash
# Create the throwaway SSM bastion of bastion.yaml and return when it can
# actually answer a statement. One stack, so `bastion-down.sh` removes all of
# it.
#
#   AWS_PROFILE=<a sandbox> tests/db/bastion-up.sh
#
# It costs about $0.012 an hour: a t4g.nano, its 8GiB volume and one public
# IPv4. Session Manager itself is free. Delete it when the check is done.
#
# Every wait says what it is waiting for, how long it has waited and when it
# will give up: a script that prints nothing for four minutes is a script
# nobody lets finish.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
stack=${KURAMA_BASTION_STACK:-kurama-tunnel-test}
# `mysql` is what tests/db/delete-account.sh talks to; `postgresql` is the default because
# the tunnel test reads it.
engine=${KURAMA_BASTION_ENGINE:-postgresql}
# The database is installed at first boot, which is a minute or two of package
# downloads on the smallest Arm instance.
agent_deadline=${KURAMA_BASTION_AGENT_SECONDS:-300}
database_deadline=${KURAMA_BASTION_DATABASE_SECONDS:-600}

started=$(date +%s)
say() { printf '[%3ds] %s\n' "$(($(date +%s) - started))" "$*"; }

say "resolving the current Amazon Linux 2023 image"
# The public SSM parameter that names it is blocked in some accounts, so the
# image is looked up the way every account can.
image=$(aws ec2 describe-images --owners amazon \
  --filters 'Name=name,Values=al2023-ami-2023.*-kernel-6.1-arm64' \
            'Name=state,Values=available' \
  --query 'reverse(sort_by(Images,&CreationDate))[0].ImageId' --output text)
[ -n "$image" ] && [ "$image" != "None" ] || { echo "no Amazon Linux 2023 image" >&2; exit 1; }

say "creating the stack with $engine (about a minute)"
aws cloudformation deploy \
  --stack-name "$stack" \
  --template-file "$here/bastion.yaml" \
  --parameter-overrides "Image=$image" "Engine=$engine" \
  --capabilities CAPABILITY_IAM \
  --no-fail-on-empty-changeset

instance=$(aws cloudformation describe-stacks --stack-name "$stack" \
  --query 'Stacks[0].Outputs[?OutputKey==`InstanceId`].OutputValue' --output text)
say "instance $instance"

# There is no waiter for "the agent registered", so this is a poll; it prints
# its own clock so the wait is visibly bounded.
say "waiting for the SSM agent (up to ${agent_deadline}s)"
until [ "$(aws ssm describe-instance-information \
  --filters "Key=InstanceIds,Values=$instance" \
  --query 'InstanceInformationList[0].PingStatus' --output text 2>/dev/null)" = "Online" ]; do
  if [ $(($(date +%s) - started)) -gt "$agent_deadline" ]; then
    say "the agent never registered; check that the subnet still reaches the internet"
    exit 1
  fi
  sleep 5
done
say "the agent is online"

# The database is waited for *on the instance*: one round trip instead of one
# per attempt, and the log comes back with the failure when it fails.
say "waiting for the database (up to ${database_deadline}s)"
wait_script=$(mktemp)
trap 'rm -f "$wait_script"' EXIT
if [ "$engine" = mysql ]; then
  cat > "$wait_script" <<'REMOTE'
for _ in $(seq 1 150); do
  systemctl is-active --quiet mysqld &&
    mysql -ukurama -pfake-client-secret -N -e 'SELECT count(*) FROM app.orders' 2>/dev/null &&
    exit 0
  sleep 5
done
echo "the database never started; the end of cloud-init:"
tail -20 /var/log/cloud-init-output.log
exit 1
REMOTE
else
  cat > "$wait_script" <<'REMOTE'
for _ in $(seq 1 120); do
  systemctl is-active --quiet postgresql && { psql -U postgres -h 127.0.0.1 -tAc 'SELECT count(*) FROM orders' && exit 0; }
  sleep 5
done
echo "the database never started; the end of cloud-init:"
tail -20 /var/log/cloud-init-output.log
exit 1
REMOTE
fi
parameters=$(mktemp)
python3 - "$wait_script" "$parameters" <<'BUILD'
import base64, json, sys
script = base64.b64encode(open(sys.argv[1], "rb").read()).decode()
json.dump(
    {"commands": [f"echo {script} | base64 -d > /tmp/wait.sh", "bash /tmp/wait.sh"]},
    open(sys.argv[2], "w"),
)
BUILD
command=$(aws ssm send-command --instance-ids "$instance" \
  --document-name AWS-RunShellScript \
  --timeout-seconds "$database_deadline" \
  --parameters "file://$parameters" \
  --query Command.CommandId --output text)
rm -f "$parameters"
aws ssm wait command-executed --command-id "$command" --instance-id "$instance" || true
output=$(aws ssm get-command-invocation --command-id "$command" --instance-id "$instance" \
  --query StandardOutputContent --output text)
case "$output" in
  *4*) say "the database answers: $(printf '%s' "$output" | tr -d '[:space:]') rows in orders" ;;
  *) say "the database did not come up:"; printf '%s\n' "$output"; exit 1 ;;
esac

cat <<EOF

the bastion is up. Run the check against it with:

  AWS_PROFILE=<the profile this ran with> \\
  KURAMA_TEST_BASTION=$stack \\
    cargo test --locked --features test-fakes --test real_db db_real_a_tunnel_through

or point a [db.*] section at it:

  [db.sandbox]
  engine = "postgresql"
  host = "localhost"          # as the bastion reaches it
  database = "postgres"
  username = "postgres"
  password = "op://<vault>/<item>/password"   # unread: localhost is trusted
  tls = "disable"

  [db.sandbox.tunnel]
  kind = "ssm"
  aws_profile = "<the profile this ran with>"
  instance_name = "$stack"

then delete it:  tests/db/bastion-down.sh
EOF
