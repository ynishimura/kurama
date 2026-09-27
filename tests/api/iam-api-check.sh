#!/usr/bin/env bash
# Check `kurama api` with `[api.*] aws_profile` against the real AWS endpoints
# that iam-api-up.sh created. Each check asserts what the echo function saw, so
# a signature AWS accepted for the wrong request still fails here.
#
#   tests/api/iam-api-check.sh <REST_BASE_URL> <FUNCTION_URL> <AWS_PROFILE>
#
# with the two URLs as iam-api-up.sh printed them. It writes a scratch
# config.toml next to itself in a temporary directory, so the real one is not
# touched, and uses the `kurama` on PATH unless KURAMA names another build.
#
# The first run that needs a credential spends a TOTP and caches the MFA
# session; the rest reuse it, so the whole check takes about a minute. A TOTP
# AWS has already seen is retried once after the 30-second window, which is
# what happens when the session cannot be cached at all.
set -uo pipefail

[ $# -eq 3 ] || { echo "usage: $0 <REST_BASE_URL> <FUNCTION_URL> <AWS_PROFILE>" >&2; exit 2; }
rest=${1%/}
lambda=${2%/}
profile=$3
kurama=${KURAMA:-kurama}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export KURAMA_CONFIG_PATH="$work/config.toml"
# The [onepassword] section of the real configuration is what lets this run
# unattended; without it kurama asks a person for the MFA code.
real=${KURAMA_REAL_CONFIG:-$HOME/.config/kurama/config.toml}
if [ -f "$real" ]; then
  awk '/^\[onepassword\]/{keep=1} /^\[/{ if ($0 != "[onepassword]") keep=0 } keep' \
    "$real" >"$KURAMA_CONFIG_PATH"
fi
cat >>"$KURAMA_CONFIG_PATH" <<CONFIG

[api.iam-rest]
base_url = "$rest"
aws_profile = "$profile"
openapi = "$rest/openapi.json"
openapi_auth = true

[api.iam-lambda]
base_url = "$lambda"
aws_profile = "$profile"

# The same REST API signed for the wrong service on purpose: the endpoint must
# refuse it, or every success above would only mean the endpoint is open.
[api.iam-wrong-service]
base_url = "$rest"
aws_profile = "$profile"
service = "s3"
CONFIG

passed=0
failed=0
report() {
  if [ "$1" = pass ]; then
    passed=$((passed + 1))
    printf 'ok   %s\n' "$2"
  else
    failed=$((failed + 1))
    printf 'FAIL %s\n  %s\n' "$2" "$3"
  fi
}

# check <name> <out|err> <expected substring> <exit code> <kurama args...>
#
# `out` matches stdout, which carries only data. `err` matches stderr and also
# requires stdout to be empty, which is where a dry run and a refusal belong.
check() {
  local name=$1 stream=$2 expect=$3 code=$4
  shift 4
  local out status stderr
  stderr="$work/stderr.$name"
  out=$("$kurama" "$@" <"/dev/null" 2>"$stderr")
  status=$?
  # AWS refuses a TOTP it has already seen, which happens when the session
  # cache cannot hold the one this run would have reused.
  if grep -qiE 'MultiFactorAuthentication|invalid MFA|not currently valid' "$stderr"; then
    sleep 33
    out=$("$kurama" "$@" <"/dev/null" 2>"$stderr")
    status=$?
  fi
  local found=1 detail
  case $stream in
    out) [[ $out == *"$expect"* ]] || found=0 ;;
    err)
      grep -qF -- "$expect" "$stderr" || found=0
      [ -z "$out" ] || { found=0; expect="$expect and an empty stdout"; }
      ;;
  esac
  detail="exit $status (wanted $code), wanted $stream to hold: $expect"
  if [ "$status" = "$code" ] && [ "$found" = 1 ]; then
    report pass "$name"
  else
    report fail "$name" "$detail; stdout: $out; stderr: $(tail -2 "$stderr" | tr '\n' ' ')"
  fi
}

# The signing scope comes off each hostname with no service or region key, and
# neither the signature nor the session token is printed. Signs without
# sending, so it spends no credential.
check dry_rest err 'ap-northeast-1/execute-api/aws4_request' 0 api iam-rest /echo --dry-run
check dry_masked err 'Signature=****' 0 api iam-rest /echo --dry-run
check dry_lambda err 'ap-northeast-1/lambda/aws4_request' 0 api iam-lambda /echo --dry-run

# The caller the endpoint authenticated is the role kurama signed as, which is
# what says the signature was validated rather than absent.
role="assumed-role"
check rest_get out "$role" 0 api iam-rest /echo --jq .caller
check lambda_get out "$role" 0 api iam-lambda /echo --jq .caller

# A body, a query with an empty and a percent-encoded value, a path with
# characters percent-encoding could mangle, and a header the caller set: each
# is in the signature, so each proves the canonical request matches what was
# sent.
# The body comes back as a JSON string, so `--jq .body` prints it with its
# quotes escaped; the multibyte character is what survives that unchanged. A
# body the signature did not cover would have been a 403, not a 200.
check rest_post out 'あ' 0 api iam-rest /echo -d '{"a":1,"b":"あ"}' --jq .body
check rest_query out '"z":["あ"]' 0 api iam-rest '/echo?b=2&a=1&z=%E3%81%82&empty=' --jq .query
check rest_path out '/echo/a%20b:c~d' 0 api iam-rest '/echo/a b:c~d' --jq .path
check rest_header out 'signed' 0 api iam-rest /echo -H 'X-Kurama-Check: signed' --jq .headers
check lambda_put out 'PUT' 0 api iam-lambda '/echo?a=1' -X PUT -d '{"n":2}' --jq .method

# The description is behind the same authorization, so --ops proves
# openapi_auth fetched it with the signature.
check ops out 'getEcho' 0 api iam-rest --ops
check describe out 'GET /echo' 0 api iam-rest --describe getEcho
check operation out '"q":["hello"]' 0 api iam-rest getEcho -P q=hello --jq .query

# Signed for the wrong service or region: refused, as one classified line.
check wrong_service err "error[API_HTTP_ERROR]: HTTP 403" 4 api iam-wrong-service /echo
check wrong_region err "error[API_HTTP_ERROR]: HTTP 403" 4 api iam-rest /echo --region us-east-1

# No signature at all: the endpoints must answer nobody.
for url in "$rest/echo" "$lambda/echo" "$rest/openapi.json"; do
  code=$(curl -s -o /dev/null -w '%{http_code}' "$url")
  if [ "$code" = 403 ]; then
    report pass "unsigned_403 $url"
  else
    report fail "unsigned_403 $url" "answered $code"
  fi
done

printf '\n%d passed, %d failed\n' "$passed" "$failed"
[ "$failed" -eq 0 ]
