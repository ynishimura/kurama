#!/usr/bin/env bash
# Remove the IAM API stack: the function, its URL, the REST API, the role and
# the log group go with it. Deleting takes under a minute.
set -euo pipefail
stack=${KURAMA_IAM_API_STACK:-kurama-iam-api-test}
started=$(date +%s)

aws cloudformation delete-stack --stack-name "$stack"
printf '[  0s] deleting %s\n' "$stack"
aws cloudformation wait stack-delete-complete --stack-name "$stack"
printf '[%3ds] the function, both endpoints and their log group are gone\n' "$(($(date +%s) - started))"
