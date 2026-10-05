## `kurama obsidian`

Reads the `[obsidian]` vault through the official Obsidian CLI (Obsidian
1.12.7+, Settings > General > Command line interface): the CLI answers from
Obsidian's own index, so Obsidian has to be running. Only the folders
`allow_paths` names are searched, listed and read, and nothing is written.

```toml
[obsidian]
vault = "obsidian-brain"            # the vault name (or id) the CLI targets
allow_paths = ["Wiki/", "Daily/"]   # folders that may be searched and read; required
cli_path = "/Applications/Obsidian.app/Contents/MacOS/obsidian"   # default "obsidian"
max_read_bytes = 65536              # default; a longer note is cut and says so
timeout = 20                        # seconds per CLI call
```

| Command | `--json` answers |
| --- | --- |
| `kurama obsidian search QUERY [--path FOLDER] [--limit N]` | `{"matches": [{"path", "line", "text"}]}`; every allowed folder when `--path` is absent, at most N notes (default 20) per folder |
| `kurama obsidian read PATH` | `{"path", "content", "truncated"}` |
| `kurama obsidian files [FOLDER]` | `{"files": [...]}` |

A path is relative to the vault root. An absolute one, one with `..` or a
backslash, or one outside every `allow_paths` entry is
`OBSIDIAN_PATH_REFUSED` (exit 2) before the CLI runs, and an answer is
filtered to the allowed folders again. A CLI that cannot be run, an
Obsidian that is not running or has its CLI disabled (`Unable to connect to
main process`), or one that does not answer within `timeout` is
`OBSIDIAN_UNAVAILABLE` (exit 3); a failure the CLI reports itself (a note that does not exist,
`Vault not found.`) is `OBSIDIAN_FAILED` (exit 1). The CLI prints its
failures where the answer goes, so `read` fails only on those two: any other
answer, including a one-line note that starts with `Error: `, is the note. Only the CLI's
`search:context`, `read` and `files` run, and a value given to kurama is
only ever the value of one `key=value` argument. A run is an audit entry
(`command = "obsidian"`, `target` the note, the folder, or `*` for every
allowed folder; never the query).
