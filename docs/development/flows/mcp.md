# MCP

How `kurama mcp` maps tool calls to kurama's own JSON command lines, on stdio or Streamable HTTP.

MCP: `kurama mcp` (`commands/mcp.rs`) reads JSON-RPC lines on stdin and
answers on stdout, one request at a time; `domain/functions/mcp.rs` maps
each tool call to one of kurama's own JSON command lines (every agent value
after `--` or glued with `=`), and `adapters/own_command.rs` runs this
binary again with `KURAMA_AGENT=1`, stdin null or the request, and a
deadline. The policy, the audit entry and the error document are the
command line's, not a copy. `kurama mcp --listen` serves the same tools
over Streamable HTTP on the loopback `[mcp] listen` for a cloud client:
`domain/functions/mcp_http.rs` decides each response (token first, then
`Origin`, path, method, headers, body) and `adapters/mcp_http.rs` only
listens with hyper and reads the bounded body. Files: `cargo xtask map mcp`.
