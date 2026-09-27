@AGENTS.md

# Claude Code notes

- `.claude/settings.json` runs `cargo fmt` on every `.rs` file you edit.
- Think in English, generate responses in English.
- TDD: write the failing test before the implementation.
- Install or update the daily-use binary with `cargo xtask install-signed`,
  not `cargo install` (which discards the code signature the keychain grants
  are bound to), and load the shell wrapper with `eval "$(kurama init zsh)"`.
  Details in `docs/development/setup.md`; the shell wrapper design in
  `src/shell/cli/commands/init.rs`.
