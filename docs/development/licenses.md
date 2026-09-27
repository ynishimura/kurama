# Licenses of kurama and what it ships

kurama itself is MIT ([LICENSE](../../LICENSE)). This page is the inventory
of what else ends up in the source tree and in a built binary, and what each
needs. Taken on 2026-09-26 from `Cargo.lock` with
`cargo metadata --locked --filter-platform aarch64-apple-darwin`; rerun it
when a dependency is added.

## Rust dependencies

Every crate is under a permissive license; none is copyleft-only.

| License expression (as declared) | Crates |
| --- | --- |
| MIT and/or Apache-2.0 (any spelling) | about 460 |
| Unicode-3.0 / Unicode-DFS-2016 | the ICU4X crates (`icu_*`, `zerovec`, `yoke`, `tinystr`, ...), `unicode-ident`, `finl_unicode`, `wezterm-bidi` |
| ISC, BSD-3-Clause | `ring`, `aws-lc-rs`, `aws-lc-sys` (TLS), `subtle`, `encoding_rs` |
| Zlib | `foldhash`, `zlib-rs` |
| CDLA-Permissive-2.0 | `webpki-roots` (the Mozilla root certificates) |
| CC0-1.0 | `tiny-keccak` |
| WTFPL | `terminfo` |
| A choice that includes a copyleft option | `self_cell` (Apache-2.0 OR GPL-2.0-only), `termina` (MIT OR MPL-2.0): kurama takes the permissive side |

## Native code compiled into the binary

| Component | How it arrives | License |
| --- | --- | --- |
| DuckDB | `libduckdb-sys` with the `bundled` feature, compiled from source | MIT |
| DuckDB httpfs extension 1.5.3 | `build.rs` downloads it from `extensions.duckdb.org`, checks its SHA-256 and embeds it (`include_bytes!`) | MIT |
| SQLite | `libsqlite3-sys` with the `bundled` feature | public domain |
| AWS-LC | `aws-lc-sys`, compiled from source | ISC, Apache-2.0, MIT and BSD-3-Clause, as declared |

## What each form of distribution needs

- **The source repository** needs nothing more than LICENSE: every dependency
  is fetched with its own license by Cargo.
- **A prebuilt binary** contains the code of every crate above and the four
  native components. MIT, BSD, ISC, Zlib and Unicode require their copyright
  and license text to accompany a binary, and Apache-2.0 requires any NOTICE
  file a crate ships. So every release archive carries
  `THIRD-PARTY-NOTICES`, which the release build writes from `Cargo.lock`
  with `cargo about` (`about.toml` accepts the licenses above; `about.hbs`
  adds the httpfs extension, which Cargo does not know about). See
  [releasing.md](releasing.md).

## Continuous checking

`cargo deny` (licenses and advisories) is not run by the gate. The license
side is held at release time instead: `cargo about` refuses to write the
notices when a crate's license is not in `about.toml`'s `accepted` list, so a
dependency under a new license stops the release until someone decides about
it. Advisories are not checked.

## Names of other products

AWS, 1Password, DuckDB, GitHub, Google, Linear, ElevenLabs, OpenAI, Slack,
Contentful, Fireworks, Jira, Zendesk and Backlog are named only to say what
kurama talks to. kurama uses none of their logos and does not claim to be
endorsed by any of them.
