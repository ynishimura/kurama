#!/usr/bin/env bash
# Remove the bastion stack. Everything bastion.yaml made goes with it, which
# is the reason it is a stack and not a dozen `aws ec2 create-*` calls.
#
# Deleting takes a minute or two; this says so rather than going quiet.
set -euo pipefail
stack=${KURAMA_BASTION_STACK:-kurama-tunnel-test}
started=$(date +%s)

aws cloudformation delete-stack --stack-name "$stack"
printf '[  0s] deleting %s (a minute or two)\n' "$stack"
aws cloudformation wait stack-delete-complete --stack-name "$stack"
printf '[%3ds] the bastion and its network are gone\n' "$(($(date +%s) - started))"
