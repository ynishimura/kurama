#!/usr/bin/env bash
# The script the database client was built for, in the shape it has to have.
#
# It deletes an AWS account from an account database, and the point of it is what
# it does *not* call: no `mysql`, no `op`, no `aws`, no `nc` and
# no `jq`. The only program it runs is `kurama`, which holds the connection,
# the bastion, the credential and the projection. Everything else here is a
# shell builtin -- the JSON is printed with `printf`, not `cat`, so the script
# needs nothing on PATH but `kurama` itself.
#
#   KURAMA_DB=testdb tests/db/delete-account.sh 123456789012            # measure
#   KURAMA_DB=testdb tests/db/delete-account.sh 123456789012 --execute  # do it
#
# Without `--execute` it runs every statement and keeps nothing, which is how
# the change is measured before it is made: the row counts are real.
set -euo pipefail

DB=${KURAMA_DB:-orders-dev}
ACCOUNT_ID="${1:?usage: $0 <ACCOUNT_ID> [--execute]}"
[[ "$ACCOUNT_ID" =~ ^[0-9]{12}$ ]] || {
  echo "12 桁の数字で指定してください" >&2
  exit 1
}

echo "== the account"
kurama db "$DB" --param "$ACCOUNT_ID" --query \
  'SELECT account_id, account_name, enabled, project_id, payer_id FROM aws_accounts WHERE account_id = ?'

echo "== what its project would be left with"
SIBLINGS=$(kurama db "$DB" --param "$ACCOUNT_ID" --param "$ACCOUNT_ID" --jq '.rows[0][0]' --query \
  'SELECT COUNT(*) FROM aws_accounts WHERE project_id = (SELECT project_id FROM aws_accounts WHERE account_id = ?) AND account_id <> ?')
echo "siblings: $SIBLINGS"
[ "$SIBLINGS" = '"0"' ] && echo "WARNING: 空プロジェクトが残ります" >&2

# One transaction: the addresses first, because the account is what they name.
request() {
  printf '%s' '{"operation":"execute","args":{"statements":[
 {"sql":"DELETE FROM aws_account_addresses WHERE aws_account_id = ?","params":["'"$ACCOUNT_ID"'"]},
 {"sql":"DELETE FROM aws_accounts WHERE account_id = ?","params":["'"$ACCOUNT_ID"'"]}],
 "max_affected_rows":10}}'
}

if [ "${2:-}" != "--execute" ]; then
  echo "== what it would delete (rolled back)"
  request | kurama db "$DB" --request - --rollback
  exit 0
fi

# The confirmation lives here, not in kurama: without a terminal kurama has to
# behave the same way, so it never prompts.
read -r -p "確認のためアカウント ID を再入力: " CONFIRM
[ "$CONFIRM" = "$ACCOUNT_ID" ] || exit 1
echo "== deleting"
request | kurama db "$DB" --request - --commit
