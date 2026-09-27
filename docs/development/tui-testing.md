# TUI development and testing

How the TUI is built so that an agent (or a person) can implement a screen,
run it, look at what a user would see, and fix it, without a real terminal
session.

## The loop

```text
model / update test
        |
render regression: every state x 80x24, 100x30, 120x40, 160x50
        |
PTY scenarios: the real binary, keys, resize, screen capture
        |
read target/agent/tui-report.md (every screen.txt embedded)
look at screen.png for visual changes
        |
fix, run again
```

One command runs it all and writes the report:

```bash
cargo xtask tui-check                     # fmt, clippy, render, pty, report
cargo xtask tui-check --update-snapshots  # accept reviewed snapshot changes
```

`cargo xtask check` (the CI gate) runs the same tests, so CI covers the
render regression, the PTY scenarios and the resize scenario headless on
Linux and macOS, and uploads `target/agent/tui/` and any
`tests/tui_snapshots/*.txt.new` as artifacts.

## Layers

| Path | Role |
| --- | --- |
| `src/shell/tui/theme.rs` | Colors, text styles, border set, markers. The only place a color is chosen. |
| `src/shell/tui/layout.rs` | Breakpoints and regions: `home_layout(area)` returns header, list, detail, footer rectangles; `modal_area` centers and clamps a box. Pure, tested as rectangles. |
| `src/shell/tui/components/` | `Header` (title, subtitle, badges), `KeyHints` (one row, drops trailing hints instead of wrapping), `panel` (bordered block), `ProfileTable` (selection, loading and empty states, name truncation with `…`), `DetailPane` (label/value rows, values wrap under the label), `Modal` (centered box sized to its wrapped body). |
| `src/shell/tui/components/` (explorer) | `KeyValues` (label/value rows for anything), `OperationTable` (method and id, dimmed when deprecated), `Form` (parameter fields with a cursor on the focused one, the body below, the last error), `TextView` (a scrollable text, the visible range in the title). |
| `src/shell/tui/components/line_input.rs` | Shared single-line input for home/explorer search, parameters and jq: UTF-8 cursor boundaries, editing keys and a display-width-aware window. MFA keeps its digits-only six-character rule. |
| `src/shell/tui/tea/update.rs` | State and transitions of the home screen, pure. |
| `src/shell/tui/tea/sources.rs` | The Auth / API / DB / Data tabs: rows from the facts `kurama status` reports, and their keys. Enter hands off to `kurama login`, `kurama api <API>` or `kurama db <DB>`, which `tui/mod.rs` runs after leaving the screen before opening it again. |
| `src/shell/tui/tea/palette.rs`, `palette_view.rs` | The command palette (`Ctrl-K` / `:`): candidates from the model plus the operations and history the runtime reads from disk, ranked by `domain/functions/fuzzy_match.rs` (at most `MAX_MATCHES`), and where Enter goes -- a tab's row, or a typed `Handoff::Explore` that `tui/mod.rs` opens as the explorer on that form. Pure. |
| `src/shell/tui/tea/view.rs` | What to show for a home model: composes layout and components. Pure. |
| `src/shell/tui/tea/runtime.rs` | Effects and the event loop: draws once after every event and once more after its effects, never while idle. |
| `src/shell/tui/explorer/update.rs`, `view.rs` | The same for `kurama api <API>`: the list with search and detail, the form, the result with headers and a jq filter, the help / error / sending / jq-input modals. Pure. |
| `src/shell/tui/explorer/jq_input.rs`, `jq_view.rs` | Pure jq completion/examples/history state and rendering. Preview effects reuse the parsed response through `Arc<Value>`; `layout::jq_modal_rows` caps choice and preview rows. |
| `src/shell/tui/explorer/history.rs`, `history_view.rs` | The request history: which parameter values are left out (a secret header or name), the order favorites and sent requests are listed in, the modal's keys and rendering. Pure; `adapters/request_history.rs` reads and appends the file. |
| `src/shell/tui/explorer/json_tree.rs` | The response as a collapsible tree: rows of the open nodes with their jq paths, bounded by `MAX_TREE_ROWS` and `MAX_CHILDREN`, and the tree's keys. Pure. |
| `src/shell/tui/explorer/runtime.rs` | The explorer's effects: the description through `spec_loader`, the call through `ApiRuntime::call`, `$EDITOR` (the terminal is suspended and the event reader paused meanwhile), the clipboard, the browser, jq. |
| `src/shell/tui/testing.rs` | Test support: render to a buffer, fixtures for every home state, snapshot files, the UI contract check (text and styles). |
| `src/shell/tui/explorer/testing.rs` | The explorer's fixtures and contract, on the same helpers. |
| `src/shell/tui/database/update.rs`, `view.rs` | The same for `kurama db <DB>`: the table list with its filter, the columns / preview / SQL / result tabs, the running request, the help / cell / error modals. Pure. |
| `src/shell/tui/database/runtime.rs` | The database explorer's effects, and the task that holds the connection: one request at a time, each under the database's deadline and stopped by `Esc`, while keys keep arriving. |
| `src/shell/tui/s3/update.rs`, `view.rs` | The same for `kurama s3 <S3>`: the bucket list, a level a page at a time, the local filter, the key and content search forms, the preview, the `kurama data` request modal. Pure. |
| `src/shell/tui/s3/runtime.rs` | The S3 explorer's effects, and the task that holds the role's clients: one request at a time, a search's rows sent as they are found, every S3 request awaited against `Esc`, which drops it and starts no other. |
| `src/shell/tui/activity/update.rs`, `view.rs` | The activity monitor (`kurama audit --watch`): the audit log's calls newest first, the selection following the newest until it is moved, a refused call in its own color and word, the selected call's details and the command that makes it again (`y` copies it). Pure. |
| `src/shell/tui/activity/runtime.rs` | Reads the log again on the tick after its length or modification time changed (so a call is on screen within 250 ms of its end), never writes it, and runs the clipboard. |
| `src/shell/tui/components/result_table.rs` | Rows and columns of a result: widths from the content, sideways scrolling to the selected cell, `∅` for NULL, `…` for a cell cut to fit. |
| `src/console.rs` | Progress lines and log records; held in memory while the TUI owns the terminal and printed after it is restored. |
| `tests/support/tui.rs` | PTY harness: launch the binary, keys, resize, wait for text, checks, screen capture. |
| `tests/support/png.rs` | `screen.png` from the emulated screen with an embedded bitmap font. |

Breakpoints (`layout.rs`):

```text
compact : width < 100    list only
normal  : 100 <= w < 140 list + detail pane (46 %)
wide    : width >= 140   list + detail pane of 64 columns
```

Modals are at most 60 columns wide, never wider than the terminal minus a
margin, and exactly as tall as their wrapped text plus padding.

## Render regression

`src/shell/tui/tea/view_snapshot_tests.rs` renders every fixture in
`testing::fixtures::all()` at every size in `testing::SIZES` with ratatui's
`TestBackend` and compares the text with `tests/tui_snapshots/<state>_<size>.txt`.

Fixture states: `loading`, `loaded`, `selected` (later row, readonly and
console on), `scrolled` (selection below the first page), `searching`,
`empty` (search without a match), `no_profiles`, `error`, `browser_failed`
(the console URL opener failed), `mfa`, `help`,
`processing`, `success`, `session_soon` and `session_expired` (the
selected profile's MFA session in the header, under 15 minutes and gone),
`auth_tab`, `api_tab` (a notice in the header), `db_tab`, `data_tab` (no
section), `palette` (operations and rows, `get` typed), `palette_no_match`, `long_text`, `unicode`, `unicode_help` (a modal over
full-width names).

The explorer (`src/shell/tui/explorer/view_snapshot_tests.rs`,
`tests/tui_snapshots/explorer_<state>_<size>.txt`) renders `loading`,
`loaded`, `token_soon` (the source's token time in the header), `selected`, `searching`, `empty`, `error` (the description failed),
`form`, `form_body` (a body skeleton), `form_error` (a refused send),
`form_long_body_error` (the error stays above a long body),
`form_no_inputs`, `processing`, `result`, `result_headers`,
`result_tree` (a node open, a leaf selected), `result_tree_large` (600
items cut at 500, selected past the page),
`history` (a favorite, a body, values), `history_naming`, `history_empty`,
`result_scrolled` (a 404 with a long body), `jq_input`, `result_jq`,
`result_jq_error`, `help`, `long_text`, `long_text_form`, `unicode` and
`notice`.

The database explorer (`src/shell/tui/database/view_snapshot_tests.rs`,
`tests/tui_snapshots/db_<state>_<size>.txt`) renders `loading`, `loaded`,
`columns`, `preview` (same-named columns, NULL and an empty string, a long
cell, full-width text, a control character, a big integer and a DECIMAL, a
base64 column, JSON, cut at `max_rows`), `sql`, `running`, `result`,
`stopped` (an `Esc` kept the SQL and the last result), `no_rows`, `cell`,
`binary_cell`, `error`, `help`, `searching` and `no_match`. Its contract adds
the `READ ONLY` badge in its on style.

The S3 explorer (`src/shell/tui/s3/view_snapshot_tests.rs`,
`tests/tui_snapshots/s3_<state>_<size>.txt`) renders `loading`, `loaded`,
`selected`, `buckets`, `filtering`, `empty`, `form` (a content search form
with a refused bound), `key_search_running` (rows arrived, more on the way),
`stopped` (`Esc` kept the partial result), `content_search` (stopped at a
bound, an escape sequence in an excerpt), `preview`, `preview_binary`,
`handoff` (the `kurama data` request), `error`, `help` and `long_text`. Its
contract adds the `READ ONLY` badge, and the selection is checked only while
the list is on screen.

The activity monitor (`src/shell/tui/activity/view_snapshot_tests.rs`,
`tests/tui_snapshots/activity_<state>_<size>.txt`) renders `loading`,
`empty`, `loaded` (a read, a refused write, a 404, a query, an exec and a
person's call), `refused` (the refused write selected), `detail`, `copied`,
`read_error` and `long_text` (an escape sequence in a path). Its contract has
no badge; the selection is checked while the list is on screen.

The `searching_middle` fixtures on both screens and explorer `form_middle`,
`jq_input_middle` and `jq_input_long` pin cursor placement and clipping. The
form PTY scenario corrects a character in the middle and checks the resulting
request URL and copied command.

A difference writes `<name>.txt.new` next to the snapshot and fails the test
with the first changed lines. Read the new file, decide whether the change is
intended, then run `cargo xtask tui-check --update-snapshots`
(`KURAMA_UPDATE_SNAPSHOTS=1` for a plain `cargo test`). `tui-check` fails
while a `.txt.new` exists, so an unreviewed change cannot pass.

The snapshots are plain text so an agent reads them directly and a diff in a
pull request shows the screen. Colors and emphasis are not in the text:
`check_contract` reads them from the buffer for the selection marker, the
modal border and the badges, the theme unit tests pin the palette, and
`screen.ansi` / `screen.png` from the PTY scenarios show them.

`NO_COLOR` is also exercised through the real backend: both screens have
PTY scenarios checking default colors and bold selection at 80x24, 120x40
and 160x50. The home scenario checks enabled badges; an empty `NO_COLOR`
keeps the palette. A theme test checks that removing colors preserves text
and modifiers for every fixture at all four sizes. `TERM=dumb` scenarios
check both entry points for exit 2, a hint, zero service calls and no raw
terminal escape sequences with debug logging enabled, including startup
logs before the TUI's terminal check. The PTY combines stdout and stderr;
its error checks inspect the combined output, while piped CLI scenarios
verify the stream separation.

## UI contract

`testing::check_contract` checks every rendered buffer in the regression:

- the header row shows the application title, and both mode badges on
  terminals at least 60 columns wide, each drawn in its on or off style
- the last row holds the key hints and shows the primary hint of the current
  screen (`↑↓ move`, `Esc clear`, `Enter back`, ...)
- the row above the footer is a panel bottom border: the footer is one row
  and the panels end above it
- the selected profile is marked with `▸` on a visible row (the table scrolls
  it into view), and the marker is drawn in the selection style
- a modal shows its title on its top border, both corners of its top and
  bottom border are on screen (it is inside the viewport), and its border is
  drawn in the modal's tone

`layout.rs` tests add: regions never overlap, the footer is the last row,
the list and the detail pane fill the width, tiny terminals do not panic.
`testing.rs` corrupts one cell (the marker's style, a modal corner's color, a
badge) and expects the contract to report it, so the checks are known to
bite. `tests/architecture/` keeps `view.rs`, `update.rs`, `layout.rs`,
`theme.rs` and `components/` free of I/O, clocks and logging.

What the machine does not judge: whether the screen is pleasant and whether
the emphasis lands on the right elements. That is what `screen.png` and the
embedded screen text in the report are for.

## PTY scenarios

`tests/support/tui.rs` runs the real `target/debug/kurama` on a pseudo
terminal (`portable-pty`) inside the same sandbox as the CLI scenarios: fake
STS and federation endpoint, fake 1Password, isolated HOME and config, the
file-backed session cache. Output is fed into a terminal emulator (`vt100`),
so the harness sees exactly the cells a terminal would show.

`launch_zsh` uses the same terminal and verification path for shell completion
on macOS. It starts `zsh -f -i` with isolated HOME/ZDOTDIR and explicitly
sources a harness `.zshrc`: compinit, the real binary's init script, an Emacs
keymap and a Delete binding. `expect_command_line` reads the current prompt
row and preserves trailing space up to the cursor; it is intended for test
commands that fit on one row. Use `exit_zsh` to cancel the edited line and
collect the outer shell's exit status and fake-service calls through the common
checks. `launch_zsh` automatically marks a completion session: all such sessions
get zero-call, no-file-write and visible-diagnostic checks. `.zshrc` and any
`with_home_file` fixtures are seeded before the file baseline, so subsequent
writes to them also fail. `launch_zsh` also points `KURAMA_COMPLETION_STDERR`
at a sandbox file that `init zsh` appends the completing children's stderr to,
and the session fails if anything lands in it. Only direct `COMPLETE=zsh` runs
check each child's exit code.

The sandbox passes `LLVM_PROFILE_FILE` to child processes when coverage is
enabled. This keeps their `.profraw` files in cargo-llvm-cov's output
directory, away from the isolated HOME. The assertions on files written
under HOME and TMPDIR stay active during coverage runs.

```rust
let mut t = tui::launch(Scenario::tui("tui_search_filters_profiles_and_esc_clears"), tui::STANDARD);
t.wait_for("Profiles (3)")
    .key(Key::Char('/'))
    .type_text("ops")
    .wait_for("Profiles 1/3  /ops_")
    .expect_row_with(&["▸", "aws", "ops-mfa"])
    .snapshot("filtered")
    .resize(tui::SMALL)
    .expect_footer_contains("q quit");
let mut v = t.quit();
v.expect_exit_code(0).expect_sts_actions(&[]);
v.finish();
```

- `Scenario::tui(id)` has no CLI run; `then_run(&["login", "ops-mfa"])` adds
  preparation runs, `with_aws_config` replaces the profiles,
  `with_env_script` sets `KURAMA_ENV_SCRIPT` like the zsh wrapper.
- `key` waits for the screen to change (text or styles), then for a quiet
  period; a key without a visible effect costs 2 seconds. `wait_for` /
  `wait_until` poll up to 10 seconds, read the screen once more at the
  deadline, and fail with the screen and the machine's load average.
  `KURAMA_TEST_PTY_TIMEOUT_SECS` replaces that deadline and the one for the
  process to exit, for a machine too loaded to draw in time.
- `resize` changes the PTY size; the process receives SIGWINCH and redraws.
- `snapshot(step)` writes `screen.txt`, `screen.ansi` and `screen.png` under
  `target/agent/tui/<scenario>/<step>/` and checks the screen for the fake
  secrets: the files leave the sandbox and CI uploads them.
- `quit` presses `q`; `exit` waits for the process. Both return the same
  `Verification` as a CLI scenario: exit code, STS / 1Password / browser
  calls, files written, the env script (mode 600), plus every recorded check.
  The terminal after the TUI is checked for `error[` lines.
- `launch_command(scenario, size, &["api", "pets"])` starts the explorer;
  `with_fake_tools()` puts the fake clipboard (`tests/fakes/pbcopy`, also
  `xclip`) on PATH and `with_env("EDITOR", FAKE_EDITOR)` makes `Ctrl-E` run
  `tests/fakes/editor`, which logs the file it was handed
  (`Run::editor_calls`), replaces the body and exits. `exit_then_run` runs
  the command the explorer copied through the CLI in the same sandbox, so
  the two recorded requests can be compared. The `tui_explorer_*` scenarios
  belong to `[api-explorer]`.
- `launch_command_for_a_person` is `launch_command` for a screen a person
  keeps open beside an agent (`audit --watch`): the scenario's
  `KURAMA_AGENT` reaches its CLI runs but not the process on the terminal,
  since a `KURAMA_AGENT` run opens no screen. `raw_output` is every byte
  the terminal received, for a comparison with what a pipe gets.

Scenario names start with `tui_` and are listed under `[tui]` in
`.agent/features/`, so `cargo xtask verify tui` runs them and
`cargo xtask impact` selects them for a change to the TUI or the executor.

## Artifacts

```text
target/agent/tui/<scenario>/<step>/
  screen.txt    plain text, one line per row, what an agent reads first
  screen.ansi   the escape sequences that redraw the screen: `cat` it in a
                terminal to see colors and emphasis
  screen.png    the rendered image, for visual review
target/agent/tui-report.md   gates, snapshot status, screen index, every screen.txt
```

`screen.png` is drawn from the emulated screen with the embedded `font8x8`
bitmap font: 16x24 pixel cells, the 16 ANSI colors, bold as the bright color,
dim, inverse and underline. Box-drawing glyphs span the full cell height so
borders join; glyphs the font lacks (most kanji) are drawn as a hollow box,
which is fine for a layout check and irrelevant for the text artifacts. No
system font, no tmux, no external tool, so the image is identical everywhere.

## Decisions

- **PTY harness: `portable-pty` + `vt100` in `tests/support/tui.rs`**
  instead of a terminal testing library. The harness must share the scenario
  sandbox, the fakes, the invariant checks and the JSON report of the CLI
  scenarios; a library (`terminal-testlib`, `ratatui-testlib`) brings its
  own scenario model, assertions and report, so neither was adopted or
  evaluated further. The harness is about 450 lines. Both crates are MIT
  and maintained.
- **Scenarios are Rust, not a YAML DSL.** The builder in
  `tests/support/tui.rs` is the DSL: a step is a method call, the compiler
  checks it, `cargo test --test scenarios -- --list` is the catalog, and a
  new assertion (`expect_row_with`, the secret check) is one method. A YAML
  layer would need a parser, its own docs and the same methods underneath,
  for no step an agent cannot already write. Revisit only if people who do
  not write Rust need to author scenarios.
- **Modal height from `Paragraph::line_count`**, behind ratatui's
  `unstable-rendered-line-info` feature (`components/modal.rs`). If a
  ratatui upgrade drops the API, wrap each body line with
  `components::take_width` and count the pieces instead.
- **The runtime draws after events only** (`tea/runtime.rs`): once after
  every dispatched event and once more after its effects. An idle TUI writes
  nothing, so the harness treats an unchanged screen as a finished frame,
  and the `Processing` modal is on screen while STS is called.
- **Snapshots: plain `.txt` files with a small comparison helper** instead
  of `insta`. The files are the screens; an agent reads them without a tool,
  review needs no `cargo-insta`, and the accept step is explicit
  (`--update-snapshots`) and refused while a `.txt.new` is unread.
- **PNG in pure Rust** (`font8x8` + `png`) instead of `tmux capture-pane`
  and `freeze`: no daemon, no font differences, works in CI. Generated on
  every run because it costs a few milliseconds; stored as a CI artifact.
- **Unicode width**: `unicode-width` (already used by ratatui) for the
  truncation in components; the `unicode` fixture and the
  `tui_help_modal_and_unicode_names_on_a_small_terminal` scenario exercise
  full-width names in the table and in the detail pane.
- **Resize**: `MasterPty::resize` sends SIGWINCH; crossterm reports it and
  ratatui redraws with the new size. The resize scenario asserts the
  selection, the footer and the pane change at 80x24 and 160x50.
- **Terminal capabilities**: kurama uses the 16 ANSI colors. A nonempty
  `NO_COLOR` removes frame colors before the backend computes its diff,
  preserving bold selection and badges. Relying on crossterm 0.29's color
  suppression alone emits empty SGR commands that reset those attributes.
  `TERM=dumb` rejects the home screen and explorer with their existing
  usage errors before raw mode or the alternate screen. Logging also
  disables ANSI output on a dumb terminal, including debug messages
  printed during bootstrap; explicit CLI commands remain available.
- **A run declared non-interactive**: `KURAMA_AGENT` (the variable that
  already marks an agent's run) makes every stream count as a pipe in
  `terminal.rs`, the only file outside the adapters that asks whether a
  stream is a terminal (ARCH-044). There is no second switch and no reverse
  one. What a login may ask a person (`can_prompt_a_person`) stays blind to
  it: the declaration changes what a run shows, not how it authenticates.

## Limits

- The PTY scenarios run the debug binary; timing constants are generous
  (10 s waits) so a slow CI machine does not flake.
- 1Password TOTP through the TUI is not driven on the PTY; the shared
  executor is covered by the CLI scenarios.
- `Terminal::resume` (after `$EDITOR`) clears the screen and resets both
  buffers instead of `ratatui::Terminal::clear`, which asks the terminal for
  the cursor position and would wait on the pseudo terminal; the explorer's
  `o` key (browser) is not driven on the PTY.
- The `Processing` modal is on screen only while STS answers, which the
  fake does within milliseconds, so it is verified by the render regression
  only.

## Change the TUI: the checklist

Compiling and passing unit tests does not show what a user sees. The loop
is implement, run, look at the screen, fix:

1. Add or update the model / `update` test for the screen.
2. Add or update the fixture in `src/shell/tui/testing.rs` when a new state
   appears (loading, loaded, empty, error, modal, long text, unicode, ...);
   every fixture is rendered at 80x24, 100x30, 120x40 and 160x50 and compared
   with `tests/tui_snapshots/`.
3. Add a `tui_*` scenario in `tests/scenarios/` when a key flow or a
   resize behavior changes (`tests/support/tui.rs`: `launch`, `key`,
   `type_text`, `resize`, `wait_for`, `expect_*`, `snapshot`, `quit`/`exit`).
   `key` writes one key and waits for the screen to react; `type_text` writes
   the whole string at once and waits once, the way a paste arrives. Per
   character it waited per character -- a 60ms quiet period each -- which is
   what made one zsh completion scenario take 40 seconds. A step that has to
   be seen keystroke by keystroke is `key` calls, not `type_text`.
4. Run `cargo xtask tui-check`.
5. Read `target/agent/tui-report.md`: it embeds every captured
   `screen.txt`. For a visual change open `screen.png` (and `screen.ansi`
   for the exact escape sequences) under `target/agent/tui/<scenario>/<step>/`.
6. Confirm nothing breaks at 80x24, 120x40 and 160x50: the render gate
   covers every state at every size, the resize scenario covers the real
   binary.
7. A snapshot difference is written as `<name>.txt.new`. Read it next to
   its `.txt`, decide whether the change is intended, then accept with
   `cargo xtask tui-check --update-snapshots`. Never accept unread.
8. Fix anything that differs from the intent and run `tui-check` again.
9. Register new files, tests and scenarios in `.agent/features/`.
10. The TUI change is done only when `tui-check` passes and the screens have
    been read.
