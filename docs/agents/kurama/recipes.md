## Recipe: why is this Lambda function failing?

Investigate the errors of one AWS Lambda function from its CloudWatch Logs
and the recent changes of its GitHub repository, read only. The inputs are
the function name, the time window (UTC) and the repository
(`<owner>/<repo>`); ask the person for any of them that is missing. Verified
with kurama 0.1.2 and later on macOS (zsh), AWS CLI v2 and the GitHub REST
API.

What it needs:

- An AWS profile in `~/.aws/config` whose role may read the function and its
  logs: `lambda:GetFunctionConfiguration`, `logs:FilterLogEvents`,
  `logs:StartQuery`, `logs:GetQueryResults` and `logs:DescribeLogGroups` on
  that function's log group. A dedicated read-only role is the smallest;
  ReadOnlyAccess covers it.
- `[api.github]` with a fine-grained token that reads the repository
  (Contents and Metadata: read): `kurama preset setup github --set
  secret=op://<vault>/<item>/credential`.
- Run as an agent (`KURAMA_AGENT=1`): every `kurama exec` on the AWS profile
  then attaches ReadOnlyAccess, and `kurama api` sends GET and HEAD only, so
  no step below can write -- a write is `AccessDenied` from AWS or
  `AGENT_POLICY_DENIED` from kurama. Never add `--confirm` for this recipe.

Steps (one `kurama exec` per step, or one `bash -c` for several: each exec
assumes the role once):

1. Say what is about to be read, and stop if it is not what the person meant:

   ```sh
   kurama exec <profile> -- bash -c 'aws sts get-caller-identity --output json && aws lambda get-function-configuration --function-name <function> --query "{arn:FunctionArn,log_group:LoggingConfig.LogGroup,last_modified:LastModified,version:Version}" --output json && echo "region=$AWS_REGION"'
   ```

   Report the account, the role, the region and the log group
   (`/aws/lambda/<function>` when `log_group` is null). A
   `ResourceNotFoundException` is the wrong function, account or region.
2. Read the errors in the window, bounded:

   ```sh
   kurama exec <profile> -- aws logs filter-log-events \
     --log-group-name /aws/lambda/<function> \
     --start-time <epoch-ms> --end-time <epoch-ms> \
     --filter-pattern '?ERROR ?Exception ?"Task timed out" ?"Runtime exited"' \
     --max-items 200 --output json
   ```

   For counts by message over a longer window use Logs Insights
   (`aws logs start-query` with `| stats count(*) by bin(5m)` and
   `| limit 50`, then `aws logs get-query-results`). Tell the four outcomes
   apart: no events (the window or the pattern found nothing -- say so),
   `AccessDeniedException` (the role lacks the action -- stop and name it),
   `ResourceNotFoundException` (the wrong log group), and a `NextToken` in
   the answer (truncated -- narrow the window rather than reading on).
3. Read what changed before the first error:

   ```sh
   kurama api github '/repos/<owner>/<repo>/commits?since=<iso>&until=<iso>&per_page=30' --jq '.[] | {sha: .sha[0:7], date: .commit.author.date, message: (.commit.message | split("\n")[0]), url: .html_url}'
   kurama api github '/repos/<owner>/<repo>/pulls?state=closed&sort=updated&direction=desc&per_page=20' --jq '.[] | select(.merged_at != null) | {number, title, merged_at, url: .html_url}'
   ```

4. Report: when the errors started and how many (with the log group and the
   window), the changes merged or deployed before that (each with its link),
   the candidate causes each with the evidence for it, and what was not
   checked. A change that precedes the errors is a candidate, never the
   cause, until a log line or the diff shows the mechanism.

Stop and ask the person when the identity or the region is not the one
they named, when a read is refused, or when the window holds more than the
bounds above. This recipe never deploys, changes a configuration, retries
an invocation or sends the report anywhere.

Log lines can hold personal data and secrets: quote only the lines the
report needs, and replace values that look like tokens, keys or e-mail
addresses with `<redacted>`. `kurama exec` puts the role's credentials in
the command's environment and nowhere else; it is not a boundary against a
command that prints its environment.
