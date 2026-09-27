#!/usr/bin/env bash
# Create the throwaway IAM-authenticated API of iam-api.yaml and print the
# `[api.*]` sections that reach it, so `kurama api` with `aws_profile` can be
# run against real AWS: an API Gateway REST API (service `execute-api`) and a
# Lambda function URL (service `lambda`), both refusing an unsigned request.
#
#   AWS_PROFILE=<a sandbox> tests/api/iam-api-up.sh
#
# Headless, with kurama itself getting the credentials (no MFA prompt):
#
#   eval "$(kurama env <profile> --json 2>/dev/null | jq -r '
#     "export AWS_ACCESS_KEY_ID=\(.AccessKeyId)",
#     "export AWS_SECRET_ACCESS_KEY=\(.SecretAccessKey)",
#     "export AWS_SESSION_TOKEN=\(.SessionToken)")"
#   tests/api/iam-api-up.sh
#
# Creating it takes about a minute and nothing in it is billed at rest, but a
# public endpoint is a public endpoint: remove it with
# tests/api/iam-api-down.sh when the check is done.
set -euo pipefail

# A failure after the deploy leaves the stack up, and a script that ends on
# its first error says nothing about that.
trap 'status=$?; [ "$status" -eq 0 ] || echo "failed: the stack may exist; remove it with tests/api/iam-api-down.sh" >&2' EXIT
here="$(cd "$(dirname "$0")" && pwd)"
stack=${KURAMA_IAM_API_STACK:-kurama-iam-api-test}

started=$(date +%s)
say() { printf '[%3ds] %s\n' "$(($(date +%s) - started))" "$*"; }

say "creating the stack (about a minute)"
aws cloudformation deploy \
  --stack-name "$stack" \
  --template-file "$here/iam-api.yaml" \
  --capabilities CAPABILITY_IAM \
  --no-fail-on-empty-changeset

output() {
  aws cloudformation describe-stacks --stack-name "$stack" \
    --query "Stacks[0].Outputs[?OutputKey==\`$1\`].OutputValue" --output text
}
url=$(output FunctionUrl)
rest=$(output RestApiUrl)
caller=$(aws sts get-caller-identity --query Arn --output text)
say "both endpoints answer $caller and nobody else"

cat <<SECTIONS

the API is up. Point [api.*] sections at it, with <profile> the AWS profile
this ran with -- neither section needs service or region, because each host
names both:

  [api.iam-rest]
  description = "throwaway REST API with AWS_IAM authorization"
  base_url = "$rest"
  aws_profile = "<profile>"
  openapi = "$rest/openapi.json"
  openapi_auth = true

  [api.iam-lambda]
  description = "throwaway Lambda function URL with AuthType AWS_IAM"
  base_url = "$url"
  aws_profile = "<profile>"

then check that a signature is what they answer:

  kurama api iam-rest /echo --jq .caller          # the role kurama signed as
  kurama api iam-lambda /echo --jq .caller
  kurama api iam-rest /echo -d '{"a":1}'          # a body in the signature
  kurama api iam-rest '/echo?b=2&a=1'             # a query in the signature
  kurama api iam-rest --ops                       # the description, fetched signed
  curl -s -o /dev/null -w '%{http_code}\n' $rest/echo   # 403: unsigned

and delete it:  tests/api/iam-api-down.sh
SECTIONS
