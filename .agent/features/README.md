# Feature map

One file per feature, `<feature>.toml`, holding a single `[<feature>]`
table. A feature is added by adding a file, so two branches adding
different features never touch the same one. `tests/architecture/` and
`cargo xtask doctor` fail when a file name and its table disagree, or when
two files declare the same feature.

# This is the only hand-written metadata in the repository. It is validated by
# tests/architecture/ (every path exists, every scenario exists, every source
# file belongs to a feature, every depends_on names a feature, every import
# that crosses into another feature's files is declared in depends_on) and by
# `cargo xtask doctor` (every test filter matches at least one test), and read
# by `cargo xtask map|impact|verify`.
#
#   summary    one line, what the feature does for the user
#   entry      the file to read first
#   files      files or directory prefixes (ending in "/") that implement it
#   tests      substring filters: cargo test --locked --features test-fakes -- <filters>
#   scenarios  test names under tests/scenarios/ (runtime verification)
#   depends_on features whose code runs when this feature is exercised, held
#              to the imports by `cargo xtask deps`. For a src/ change `impact`
#              walks the importers and takes one depends_on step; for any other
#              file it follows depends_on transitively
#   notes      what runtime verification does not cover yet

