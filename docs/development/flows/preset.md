# Presets

How a preset is listed, expanded against config.toml and saved.

Presets: `kurama preset` (`src/shell/cli/commands/preset.rs`) lists the
catalog, const data in `src/domain/types/preset.rs`, without reading
configuration. `preset show` expands one through
`domain/functions/preset_render.rs`: `plan_preset` decides the names
(`--as` renames the `[api.*]` only), whether an existing `[auth.*]` is
reused (its contract must be the preset's; references are compared as
written, never resolved), the missing inputs, the setup steps and the
TOML. `plan_against_file` and `check_plan` run it against config.toml
through the config writer's `append` and `validate`; `show` never calls
`save`, and `preset add` (`commands/preset_add.rs`) saves what they passed
and prints `config add`'s `SaveReport`. A preset is
never read at runtime: what it prints is ordinary configuration.
Files: `cargo xtask map preset`.
