#!/usr/bin/env bash
# Create the two throwaway `kurama data` buckets of s3.yaml -- one in the
# profile's Region, one in another -- put the fixtures of
# tests/fixtures/data/ in both, and print the inputs that read them. The
# second bucket is the point: a bucket outside the profile's Region
# answers the first request with 301 and its own Region, and kurama has to
# follow that once. Two stacks, because a bucket lives where its stack does;
# `s3-down.sh` empties and removes both.
#
#   AWS_PROFILE=<a sandbox> tests/s3/s3-up.sh
#
# It costs about $0 an hour: a few kilobytes of storage and the requests a
# check makes (PUT $0.005 per thousand, GET/HEAD $0.0004 per thousand).
# Creating both takes about a minute. Delete them when the check is done.
#
# Every wait says what it is waiting for and how long it took; CloudFormation
# does the waiting here, so there is nothing to give up on.
set -euo pipefail

# A failure after the first deploy leaves a stack up, and a script that ends
# on its first error says nothing about that.
trap 'status=$?; [ "$status" -eq 0 ] || echo "failed: a stack may exist; remove both with tests/s3/s3-down.sh" >&2' EXIT
here="$(cd "$(dirname "$0")" && pwd)"
fixtures="$(cd "$here/../fixtures/data" && pwd)"
stack=${KURAMA_S3_DATA_STACK:-kurama-s3-data-test}
# Not the Region of any profile a check runs with; the fake of
# tests/support/data.rs puts `west-bucket` in the same one.
other_region=${KURAMA_S3_OTHER_REGION:-us-west-2}
# Bucket names are global: a run of `cargo xtask verify --layer throwaway`
# hands its id, so its buckets are never a person's or another run's.
run=${KURAMA_THROWAWAY_RUN:+-$KURAMA_THROWAWAY_RUN}

started=$(date +%s)
say() { printf '[%3ds] %s\n' "$(($(date +%s) - started))" "$*"; }

account=$(aws sts get-caller-identity --query Account --output text)
# The Region `kurama exec` exports is the one the calls below are signed
# for; the profile's own setting only when nothing is exported.
region=${AWS_REGION:-${AWS_DEFAULT_REGION:-$(aws configure get region || true)}}
[ -n "$region" ] || { echo "the profile names no region; set one in ~/.aws/config" >&2; exit 1; }
[ "$region" != "$other_region" ] || { echo "the profile's region is $region; KURAMA_S3_OTHER_REGION has to be another" >&2; exit 1; }
other_bucket="kurama-data-test-$account-other$run"

say "creating $stack-other in $other_region (about half a minute)"
aws cloudformation deploy \
  --region "$other_region" \
  --stack-name "$stack-other" \
  --template-file "$here/s3.yaml" \
  --parameter-overrides "Suffix=other$run" \
  --no-fail-on-empty-changeset

say "creating $stack in $region, naming the other bucket in its outputs"
aws cloudformation deploy \
  --stack-name "$stack" \
  --template-file "$here/s3.yaml" \
  --parameter-overrides "Suffix=same$run" "OtherBucket=$other_bucket" "OtherRegion=$other_region" \
  --no-fail-on-empty-changeset

output() {
  aws cloudformation describe-stacks --stack-name "$stack" \
    --query "Stacks[0].Outputs[?OutputKey==\`$1\`].OutputValue" --output text
}
bucket=$(output BucketName)

# The same objects in both, so the only difference a check can see is the
# Region: orders.parquet is the committed twin of orders.csv, because the
# script has no DuckDB to write one with.
for target in "s3://$bucket" "s3://$other_bucket"; do
  say "uploading the fixtures to $target"
  for file in orders.csv customers.csv events.jsonl orders.parquet; do
    aws s3 cp --only-show-errors "$fixtures/$file" "$target/$file"
  done
done
say "both buckets answer"

cat <<EOF

the buckets are up. Read them with <profile> the AWS profile this ran with:

  kurama data --from s3://$bucket/orders.parquet --aws-profile <profile> --query 'SELECT count(*) AS rows FROM data' --json
  kurama data --from s3://$other_bucket/orders.parquet --aws-profile <profile> --query 'SELECT count(*) AS rows FROM data' --json
      # meta.region is $other_region: the profile's region signed the first
      # request, S3 answered 301 with the bucket's own, and kurama followed it
  kurama data --from s3://$other_bucket/orders.csv --aws-profile <profile> --region $region --describe --json
      # a named region is never corrected: DATA_REJECTED naming $region

then delete them:  tests/s3/s3-down.sh
EOF
