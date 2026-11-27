#!/bin/sh
# Unpacks the language archives resources/localization/<lang>.cpr (git LFS) into
# dist-template/mods/localization/<lang>/, the working copy edited by hand and packed again by tools/package.sh.
# The target folder is deleted first: files missing from the archives (tutorial voice-over in <p2|p3>/musik/) go too.
# Requires: Rust (host target), git lfs (`git lfs pull` when the archives are still pointer files).
set -eu
cd "$(dirname "$0")/.."

SRC=resources/localization
DST=dist-template/mods/localization

# cprtool runs here: built for the host, not the default target of .cargo/config.toml
HOST=$(rustc -vV | sed -n 's/^host: //p')
cargo build --release -p cprtool --target "$HOST"
CPRTOOL=target/$HOST/release/cprtool

rm -rf "$DST"
mkdir -p "$DST"
for cpr in "$SRC"/*.cpr; do
  lang=$(basename "$cpr" .cpr)
  "$CPRTOOL" unpack "$cpr" "$DST/$lang"
done
