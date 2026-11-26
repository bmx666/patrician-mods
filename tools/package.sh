#!/bin/sh
# Builds every artifact (i686 Windows) and packs dist/patrician-mods-<version>.zip:
#   winmm.dll, patrician-launcher.exe, mods/qol.dll, mods/localization.dll, mods/nosmc.dll, mods/settings.ini,
#   mods/localization/<lang>/<p2|p3>/ (without macOS .DS_Store),
#   Win32/Mp3dec.asi, Win32/Aacdec.asi, licenses/, LICENSE, README.md
# Requires: Rust (target i686-pc-windows-gnu), zig, cargo-zigbuild, zip.
set -eu
cd "$(dirname "$0")/.."

TARGET=i686-pc-windows-gnu
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
OUT=target/$TARGET/release
STAGE=dist/patrician-mods-$VERSION

cargo zigbuild --release --target "$TARGET"

rm -rf "$STAGE" "$STAGE.zip"
mkdir -p "$STAGE/mods" "$STAGE/Win32" "$STAGE/licenses"
cp "$OUT/winmm.dll" "$OUT/patrician-launcher.exe" "$STAGE/"
cp "$OUT/qol.dll" "$OUT/localization.dll" "$OUT/nosmc.dll" "$STAGE/mods/"
cp dist-template/mods/settings.ini "$STAGE/mods/"
cp -r dist-template/mods/localization "$STAGE/mods/"
find "$STAGE" -name .DS_Store -delete
# Miles codecs: both games load Win32\*.asi (AIL_set_redist_directory "./win32")
cp "$OUT/mp3dec.dll" "$STAGE/Win32/Mp3dec.asi"
cp "$OUT/aacdec.dll" "$STAGE/Win32/Aacdec.asi"
cp crates/asi-mp3/c/LICENSE.minimp3 "$STAGE/licenses/minimp3.txt"
cp crates/asi-aac/faad3/COPYING "$STAGE/licenses/faad3-LGPL-2.1.txt"
cp LICENSE README.md "$STAGE/"
(cd dist && zip -r -9 "patrician-mods-$VERSION.zip" "patrician-mods-$VERSION")
echo "dist/patrician-mods-$VERSION.zip"
