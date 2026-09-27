## `kurama mcp`

An MCP server on stdio (JSON-RPC, one message per line; protocol versions
2025-06-18, 2025-03-26 and 2024-11-05). Register it with an MCP client as
the command `kurama mcp`. stdout carries the protocol only.

| Tool | Arguments | Runs |
| --- | --- | --- |
| `ready` | none | `kurama agent ready --json` |
| `list_apis` | none | `kurama status --only api --json` |
| `list_operations` | `api`, `query`? | `kurama api API --ops [QUERY] --json` |
| `describe_operation` | `api`, `operation` | `kurama api API --schema OPERATION` |
| `call_api` | `api`, `target`, `method`?, `params`?, `body`?, `shape`?, `sample`? | `kurama api API TARGET --json` (`-P`, `-d`, `--shape`, `--sample`) |
| `query_data` | `request`, `workspace`? | `kurama data [WORKSPACE] --request - --json` |
| `query_db` | `database`, `request` | `kurama db DATABASE --request - --json` |

Each call is kurama run again with `KURAMA_AGENT=1`, whatever the server's
own environment says: the `[agent]` policy applies, the audit log records
the call, and the result is that command's stdout (`structuredContent` when
it is a JSON object). A failure is a tool result with `isError: true` and
the command's JSON error document, `next_actions` included; nothing waits
for a person, because the call has no terminal. MCP offers no `--confirm`:
a call the policy refuses is made by a person on the command line.
`query_data` refuses `args.export` and `query_db` the `execute` operation
before anything runs. A call ends after at most 600 seconds.
