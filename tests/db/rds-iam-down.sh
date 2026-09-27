#!/usr/bin/env bash
# Remove the RDS IAM stack. Everything rds-iam.yaml made goes with it, the
# master passwords in Secrets Manager included, and no final snapshot is kept.
#
# Deleting takes about ten minutes; this says so rather than going quiet.
set -euo pipefail
stack=${KURAMA_RDS_IAM_STACK:-kurama-rds-iam-test}
started=$(date +%s)

aws cloudformation delete-stack --stack-name "$stack"
printf '[  0s] deleting %s (about ten minutes)\n' "$stack"
aws cloudformation wait stack-delete-complete --stack-name "$stack"
printf '[%3ds] the databases, the bastion and their network are gone\n' "$(($(date +%s) - started))"
