#!/bin/sh
# Builds every artifact (i686 Windows) and packs dist/patrician-mods-<version>.zip:
#   winmm.dll, patrician-launcher.exe, mods/qol.dll, mods/settings.ini, LICENSE, README.md
# Requires: Rust (target i686-pc-windows-gnu), zig, cargo-zigbuild, zip.
set -eu
cd "$(dirname "$0")/.."

TARGET=i686-pc-windows-gnu
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
OUT=target/$TARGET/release
STAGE=dist/patrician-mods-$VERSION

cargo zigbuild --release --target "$TARGET"

rm -rf "$STAGE" "$STAGE.zip"
mkdir -p "$STAGE/mods"
cp "$OUT/winmm.dll" "$OUT/patrician-launcher.exe" "$STAGE/"
cp "$OUT/qol.dll" "$STAGE/mods/"
cp dist-template/mods/settings.ini "$STAGE/mods/"
cp LICENSE README.md "$STAGE/"
(cd dist && zip -r -9 "patrician-mods-$VERSION.zip" "patrician-mods-$VERSION")
echo "dist/patrician-mods-$VERSION.zip"
