#!/usr/bin/env bash
# Remove both `kurama data` bucket stacks. CloudFormation refuses to delete a
# bucket with objects in it, so each bucket is emptied first; a bucket that is
# already gone is not an error, because a run that failed half-way is what
# this is for. Deleting takes under a minute per stack; this says so rather
# than going quiet.
set -euo pipefail
stack=${KURAMA_S3_DATA_STACK:-kurama-s3-data-test}
other_region=${KURAMA_S3_OTHER_REGION:-us-west-2}
started=$(date +%s)
say() { printf '[%3ds] %s\n' "$(($(date +%s) - started))" "$*"; }

# The bucket names come from the stacks' own outputs, so a name chosen by a
# different account or suffix is still the one emptied.
bucket_of() {
  aws cloudformation describe-stacks "$@" --query 'Stacks[0].Outputs[?OutputKey==`BucketName`].OutputValue' --output text 2>/dev/null || true
}
empty() {
  local bucket=$1; shift
  if [ -n "$bucket" ] && [ "$bucket" != "None" ]; then
    say "emptying s3://$bucket"
    aws s3 rm "s3://$bucket" --recursive --only-show-errors "$@" || say "s3://$bucket could not be emptied (already gone?)"
  fi
}

empty "$(bucket_of --stack-name "$stack")"
empty "$(bucket_of --region "$other_region" --stack-name "$stack-other")" --region "$other_region"

say "deleting $stack and $stack-other (under a minute each)"
aws cloudformation delete-stack --stack-name "$stack"
aws cloudformation delete-stack --region "$other_region" --stack-name "$stack-other"
aws cloudformation wait stack-delete-complete --stack-name "$stack"
aws cloudformation wait stack-delete-complete --region "$other_region" --stack-name "$stack-other"
say "both buckets and their stacks are gone"
