# TUI

The TUI's layers (theme, layout, components, pure view/update) and the runtime of each screen.

TUI (`src/shell/tui`): `theme.rs` (colors, styles, borders), `layout.rs`
(breakpoints compact `< 100` / normal / wide `>= 140` columns, regions,
modal placement), `components/` (Header, KeyHints, Tabs, panel, ProfileTable,
DetailPane, KeyValues, OperationTable, Form, LineInput, TextView, Modal,
ResultTable), `tea/view.rs` (what to show; pure) and `tea/update.rs`
(state; pure) for the home screen, `explorer/view.rs` and
`explorer/update.rs` for `kurama api <API>`, `database/view.rs` and
`database/update.rs` for `kurama db <DB>`, `s3/view.rs` and `s3/update.rs`
for `kurama s3 <S3>`. A screen never picks a color or a magic width itself;
`tests/architecture/` keeps these files free of I/O, clocks and logging.
`tea/runtime.rs` and `explorer/runtime.rs` redraw after an event only; the
explorer runs its effects (the description, the call through
`ApiRuntime::call`, `$EDITOR` with the terminal suspended, the clipboard,
the browser, jq) and shows their results. jq input state and panels live in
`explorer/jq_input.rs` and `jq_view.rs`; `domain/functions/jq_completion.rs`
shares pure JsonShape traversal with CLI completion. Previews reuse the
parsed response and the CLI jq evaluator with a bounded output count. Progress lines and logs go
through `src/console.rs`, which holds them while the TUI owns the
terminal. Details in `docs/development/tui-testing.md`.
The database explorer (`database/runtime.rs`) is the one screen whose
requests do not hold it: a spawned task owns the connection and the
tunnel, takes one request at a time over a channel, and the screen reads
the answer on the next key or tick, so `Esc` can stop a statement. Before
the screen opens, `database/mod.rs` resolves the secrets, the tunnel and
the first connection through `src/shell/db_connection.rs`, which the CLI
shares with it (with the deadline-and-stop wait), so a failure there is
the CLI's `error[CODE]`. Each request re-arms the stop flag and ends with
`DbSession::end_request`, which rolls a server's read guard back; the next
read opens it again.
`terminal.rs` checks every entry point for interactive streams and
`TERM != dumb`, is the one file outside the adapters that asks whether a
stream is a terminal (a `KURAMA_AGENT` run counts as a pipe there: no
TUI, the pipe's stdout bytes, no rewritten line), and applies `NO_COLOR` to the rendered buffer through
the pure `theme::remove_colors` before the backend diff. Text and
non-color attributes remain intact; keep environment reads out of views.
