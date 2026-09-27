#!/usr/bin/env bash
# Remove the Aurora DSQL stack: the cluster goes with it, which is why
# dsql.yaml creates it without deletion protection. Deleting takes a minute
# or two; this says so rather than going quiet.
set -euo pipefail
stack=${KURAMA_DSQL_STACK:-kurama-dsql-test}
started=$(date +%s)

aws cloudformation delete-stack --stack-name "$stack"
printf '[  0s] deleting %s (a minute or two)\n' "$stack"
aws cloudformation wait stack-delete-complete --stack-name "$stack"
printf '[%3ds] the cluster is gone\n' "$(($(date +%s) - started))"
