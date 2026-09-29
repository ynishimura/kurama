# Releasing

A release is a `v*` tag. Pushing it runs `.github/workflows/release.yml`,
which `dist` (cargo-dist) generates from `dist-workspace.toml`; nothing else
starts that workflow, and pull requests do not (`pr-run-mode = "skip"`).

## What a release is made of

| Artifact | For |
| --- | --- |
| `kurama-<target>.tar.xz` for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu` | the binary with `LICENSE`, `README.md`, `CHANGELOG.md` and `THIRD-PARTY-NOTICES` |
| `kurama-installer.sh` | `curl ... \| sh`: installs the matching archive into `~/.cargo/bin` |
| `kurama.rb` in the tap `ynishimura/homebrew-tap` | `brew install ynishimura/tap/kurama` |
| `source.tar.gz`, `sha256.sum`, one `.sha256` per archive | verification |
| `kurama-<target>.sigstore.json` per build | the signed build provenance of that target's archives, for `gh attestation verify <archive> --repo ynishimura/kurama --bundle <file>` without asking GitHub |

Each target is built on its own native GitHub runner (`dist plan` names
them), because `build.rs` compiles DuckDB and embeds the httpfs extension of
that target. Those four are the only targets: any other stops the build with
a message naming it. crates.io is not a channel: `cargo install` would compile
DuckDB from source on every machine and download the extension during the
build.

`THIRD-PARTY-NOTICES` is written in each build job by the steps in
`.github/release-build-setup.yml` (`cargo about generate`, configured by
`about.toml` and `about.hbs`), so it always matches the `Cargo.lock` of the
tag. [licenses.md](licenses.md) is the inventory behind it. To read it before
a release: `cargo about generate about.hbs -o THIRD-PARTY-NOTICES` (the file
is ignored by Git).

## Signing and the keychain

Release binaries are ad-hoc signed, as the linker leaves them: there is no
Developer ID.

- Installed through Homebrew or the shell installer, the binary carries no
  quarantine attribute and starts without a Gatekeeper prompt. An archive
  downloaded with a browser does: `xattr -d com.apple.quarantine kurama`.
- macOS grants keychain access to an ad-hoc binary by its `cdhash`, which
  every release changes. So after an upgrade, the first read of each MFA
  session or OAuth token asks once. That is safe to refuse: the keychain is a
  cache (AGENTS.md, invariant 1), and a refused read is fetched again. A run
  without a terminal gets the 1Password service account token from
  `OP_SERVICE_ACCOUNT_TOKEN` rather than from the keychain.
- `cargo xtask install-signed` is for a build from source, not for a release:
  it signs with a local self-signed identity whose private key never leaves
  the machine, so it cannot sign what others download.
- A Developer ID (Apple Developer Program) with notarization would remove
  both prompts, because a signature with a team ID is granted by the team,
  not by the build. It is the next step if the prompts turn out to matter.

## Before the first release

These happen once, when the repository is public:

1. Create the tap repository `ynishimura/homebrew-tap` (empty is fine).
2. Create a fine-grained token that can write to the tap's contents, and
   store it as the repository secret `HOMEBREW_TAP_TOKEN`; the `publish-homebrew-formula`
   job pushes `kurama.rb` with it.
3. If the repository is not `ynishimura/kurama`, change `repository` and
   `homepage` in `Cargo.toml` and `tap` in `dist-workspace.toml`, then run
   `dist generate`.
4. For the landing page (`site/index.html`), set Settings > Pages > Source to
   "GitHub Actions" (`gh api -X POST repos/<owner>/<repo>/pages -f build_type=workflow`),
   then `gh workflow run pages.yml`. Run it again whenever the page or the
   images change; nothing else publishes it. `python3 -m http.server -d site`
   previews it locally. The command reference (`site/commands.html`) reads
   `site/commands.json`: rewrite it with `kurama agent --json > site/commands.json`
   from the build being released, so it lists that build's options.

## Cutting a release

1. On `main`: `cargo xtask check` passes, and `cargo xtask verify-real --check`
   says every feature's evidence is fresh.
2. Set `version` in `Cargo.toml`, run `cargo check` so `Cargo.lock` follows,
   and rename `## Unreleased` in `CHANGELOG.md` to `## <version> - <date>`;
   dist takes the release notes from that section.
3. Tag `main`: `git tag v<version>` and
   `git push origin v<version>`.
4. Check the release page, run the installer on a clean machine, and start a
   new `## Unreleased` section.

## Changing the release configuration

Edit `dist-workspace.toml` (or `.github/release-build-setup.yml`), then run
`dist generate` and commit the regenerated `release.yml` with it; `dist` is
pinned in `mise.toml`, and so is `cargo-about`. `dist plan` prints what a
release would contain without building anything.

`release.yml` also carries edits dist has no setting for, which
`allow-dirty = ["ci"]` in `dist-workspace.toml` lets it keep and which
`dist generate` overwrites. They are what OpenSSF Scorecard's
Token-Permissions, Pinned-Dependencies and Signed-Releases checks read, so
re-apply them after every `dist generate` (`git diff` shows each one going):

| Edit | Where |
| --- | --- |
| Top-level `permissions` is `contents: read`; only `host` gets `contents: write` | top of the file, the `host` job |
| `Install dist` runs `.github/install-dist.sh` (a hash-checked archive) instead of the installer piped into `sh` | `plan`, `build-local-artifacts` |
| The `Install Rust non-interactively` step is removed: no target builds in a container | `build-local-artifacts` |
| `Attest` has `id: attest`, and `Keep the attestation bundle` copies its bundle to `target/distrib/kurama-<targets>.sigstore.json`, which `Upload artifacts` lists | `build-local-artifacts` |

On a cargo-dist upgrade, also update `version` and the four SHA-256 values in
`.github/install-dist.sh` (each archive's `.sha256` on the cargo-dist release)
and the commits in `github-action-commits`.
