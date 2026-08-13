#!/usr/bin/env bash
# Assemble the external resources shared by every Crossh binary.
set -euo pipefail

cd "$(dirname "$0")/.."

DESTINATION="${1:?usage: scripts/copy-shared-assets.sh <destination>}"
mkdir -p "$DESTINATION/brand" "$DESTINATION/fonts/ibm-plex-sans" "$DESTINATION/fonts/lilex" "$DESTINATION/icons"

ZED_REV="$(awk -F '"' '/^assets = \{ git = .* rev = / { print $4; exit }' Cargo.toml)"
ZED_ROOT=""
ZED_PREFIX="${ZED_REV:0:7}"
for checkout in "$HOME"/.cargo/git/checkouts/zed-*/*; do
    checkout_revision="$(basename "$checkout")"
    if [[ "$checkout_revision" == "$ZED_PREFIX"* ]] && [ -d "$checkout/assets" ]; then
        ZED_ROOT="$checkout"
        break
    fi
done
if [ -z "$ZED_ROOT" ]; then
    echo "unable to locate cached Zed assets for revision $ZED_REV" >&2
    exit 1
fi

cp crates/crossh-assets/assets/icons/*.svg "$DESTINATION/icons/"
cp assets/appicon/icon-master.svg "$DESTINATION/brand/crossh-logo.svg"
cp "$ZED_ROOT/assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf" \
    "$DESTINATION/fonts/ibm-plex-sans/"
cp "$ZED_ROOT/assets/fonts/ibm-plex-sans/IBMPlexSans-SemiBold.ttf" \
    "$DESTINATION/fonts/ibm-plex-sans/"
cp "$ZED_ROOT/assets/fonts/lilex/Lilex-Regular.ttf" "$DESTINATION/fonts/lilex/"
cp "$ZED_ROOT/assets/fonts/lilex/Lilex-Bold.ttf" "$DESTINATION/fonts/lilex/"

printf '{"schema":1,"zed_revision":"%s"}\n' "$ZED_REV" > "$DESTINATION/manifest.json"
