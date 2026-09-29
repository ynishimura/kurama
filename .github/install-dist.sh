#!/usr/bin/env bash
# Install the cargo-dist release.yml runs into ~/.cargo/bin, from the release
# archive checked against the SHA-256 recorded here, instead of piping dist's
# installer script into sh. The version is dist-workspace.toml's
# cargo-dist-version; docs/development/releasing.md says how to update both.
set -euo pipefail

version=0.33.0
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target=x86_64-unknown-linux-gnu
    sha256=4b3f0a5f0ebbdb798f6db649d01b32ba1518376b6f7a0502b7d92b75cc2c8293 ;;
  Linux-aarch64) target=aarch64-unknown-linux-gnu
    sha256=9c554ab21a58ad46eb9b6710f89633ccb26bc2577cf4b0c2d18a5b826a31913e ;;
  Darwin-x86_64) target=x86_64-apple-darwin
    sha256=6a49bfb61bd86770d79c27f3d2b40c6b2e71cde940d3d31a6ccaaffc124d7a29 ;;
  Darwin-arm64) target=aarch64-apple-darwin
    sha256=7b3cbe25511de01d74c0f5fcb7909edabd379bea9cfa284d93af5a3cdfa3247c ;;
  *) echo "no pinned cargo-dist archive for $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

archive="$(mktemp -d)/cargo-dist.tar.xz"
curl --proto '=https' --tlsv1.2 -fsSL -o "$archive" \
  "https://github.com/axodotdev/cargo-dist/releases/download/v${version}/cargo-dist-${target}.tar.xz"
echo "${sha256}  ${archive}" | shasum -a 256 -c -
mkdir -p "$HOME/.cargo/bin"
tar -xJf "$archive" -C "$HOME/.cargo/bin" --strip-components=1 "cargo-dist-${target}/dist"
"$HOME/.cargo/bin/dist" --version
