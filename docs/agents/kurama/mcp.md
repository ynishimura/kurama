## `kurama mcp`

An MCP server on stdio (JSON-RPC, one message per line; protocol versions
2025-06-18, 2025-03-26 and 2024-11-05). Register it with an MCP client as
the command `kurama mcp`. stdout carries the protocol only.

| Tool | Arguments | Runs |
| --- | --- | --- |
| `ready` | none | `kurama status --ready --json` |
| `list_apis` | none | `kurama status --only api --json` |
| `list_operations` | `api`, `query`? | `kurama api API --ops [QUERY] --json` |
| `describe_operation` | `api`, `operation` | `kurama api API --schema OPERATION` |
| `call_api` | `api`, `target`, `method`?, `params`?, `query`?, `body`?, `shape`?, `sample`? | `kurama api API TARGET --json` (`-P`, `-d`, `--shape`, `--sample`); `query` is appended to TARGET, each value percent-encoded |
| `query_data` | `request`, `workspace`? | `kurama data [WORKSPACE] --request - --json` |
| `query_db` | `database`, `request` | `kurama db DATABASE --request - --json` |
| `obsidian_search` | `query`, `path`?, `limit`? | `kurama obsidian search QUERY --json` |
| `obsidian_read` | `path` | `kurama obsidian read PATH --json` |
| `obsidian_files` | `folder`? | `kurama obsidian files [FOLDER] --json` |

Each call is kurama run again with `KURAMA_AGENT=1`, whatever the server's
own environment says: the `[agent]` policy applies, the audit log records
the call, and the result is that command's stdout (`structuredContent` when
it is a JSON object). A failure is a tool result with `isError: true` and
the command's JSON error document, `next_actions` included; nothing waits
for a person, because the call has no terminal. MCP offers no `--confirm`:
a call the policy refuses is made by a person on the command line.
`query_data` refuses `args.export` and `query_db` the `execute` operation
before anything runs. A call ends after `[mcp] call_timeout` seconds (600 by
default). `[mcp] tools` names the tools offered; a tool it leaves out is
absent from `tools/list`, and a call of it is a refused tool result. The
`obsidian_*` tools are offered only when `[obsidian]` is configured (see
`kurama obsidian`), and naming one in `tools` without it is `CONFIG_INVALID`. The
configuration is read at start, so an invalid one stops `kurama mcp` with
`CONFIG_INVALID` before it answers anything.

### Over HTTP: `kurama mcp --listen`

For a client that runs in the cloud and cannot start a local process
(ElevenLabs Agents, the xAI, Claude or OpenAI APIs, a claude.ai custom
connector), `--listen` serves MCP Streamable HTTP on `[mcp] listen`: JSON
responses only, no session, no SSE stream. TLS is not kurama's: put
Tailscale Funnel (`tailscale funnel --bg 8807`) or another front before it.
Funnel needs HTTPS certificates enabled for the tailnet and the `funnel`
nodeAttr in its policy file; it serves on 443 (or 8443 / 10000).

```toml
[mcp]
listen = "127.0.0.1:8807"                         # loopback only
token = "op://Agent/kurama-mcp-token/credential"  # a reference, never the value
tools = ["list_operations", "describe_operation", "call_api"]
call_timeout = 25                                 # below the client's own timeout
max_concurrent_calls = 2
```

Every request carries the token in `Authorization`, as `Bearer <token>` or
the token alone. The token is checked first, so a request without it is
`401` whatever its path or method; then a request with an `Origin` header is
`403`, a path other than `/mcp` `404`, a method other than POST `405`, a
`Content-Type` other than `application/json` `415`, an `MCP-Protocol-Version`
this server does not speak `400` (except on `initialize`, which negotiates
the version in its body), a body over 1 MiB `413`, and a body not received
within 30 seconds or one that is not one JSON-RPC message `400`; a
connection that has not sent its headers within 10 seconds is closed. A request is answered `200` with one JSON
response, a notification or a response `202`. The token is read once at
start: changing it takes a restart, and an empty value is
`SECRET_INVALID`. stderr gets `listening on ADDRESS`, then
one line per request with the method, the path and the status. A missing
`listen` or `token`, a `listen` that is not loopback, a literal `token` or an
unknown name in `tools` is `CONFIG_INVALID`; a port another process holds is
`MCP_LISTEN_FAILED` (exit 1).

kurama does not tell callers apart: whoever holds the token acts with your
AWS roles and API credentials, and a tool's input and output stay in the
logs of the service hosting the client. Expose it to your own agent only,
name only APIs whose responses may reach that service, and leave `ready`,
`list_apis`, `query_data` and `query_db` out of `tools`.
