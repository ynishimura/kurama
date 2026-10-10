# Presets

How a preset is listed, expanded against config.toml and saved.

Presets: `kurama preset` (`src/shell/cli/commands/preset.rs`) lists the
catalog, const data in `src/domain/types/preset.rs`, without reading
configuration. `preset setup` (`commands/preset_setup.rs`) is the one verb
that uses a preset. Its `configure` step expands it through
`domain/functions/preset_render.rs`: `plan_preset` decides the names
(`--as` renames the `[api.*]` only), whether an existing `[auth.*]` is
reused (its contract must be the preset's; references are compared as
written, never resolved), the missing inputs, the setup steps and the
TOML. `plan_against_file` and `check_plan` run it against config.toml
through the config writer's `append` and `validate`, and `ConfigFile::save`
writes what passed -- except on `--dry-run`, which reports the TOML and
stops. An `[api.*]` the file already has is kept, so a rerun resumes. The
later steps reuse other features rather than copy them: `check` types the
`[api.*]`, `credential` reads the API's row of `agent_ready::readiness_rows`,
`agent` writes kurama's Skill through `agent_install::place`, and
`first_read` sends the preset's example through `ApiRuntime::call` after
`check_api_request` (the `[agent]` policy), noted in the audit entry
`cli/mod.rs` opens for it. The first failed step's error is returned as it
is, so `ErrorCode::classify` gives it the code it has everywhere. A preset is
never read at runtime: what it writes is ordinary configuration.
Files: `cargo xtask map preset`.
