#!/usr/bin/env bash
# Assemble the external resources shared by every Crossh binary.
set -euo pipefail

cd "$(dirname "$0")/.."

DESTINATION="${1:?usage: scripts/copy-shared-assets.sh <destination>}"
mkdir -p "$DESTINATION/brand" "$DESTINATION/fonts/ibm-plex-sans" "$DESTINATION/fonts/lilex" "$DESTINATION/icons"

# 字体已 vendor 进 crossh-assets（不再依赖 Zed assets crate），直接从仓库拷贝。
cp crates/crossh-assets/assets/icons/*.svg "$DESTINATION/icons/"
cp assets/appicon/icon-master.svg "$DESTINATION/brand/crossh-logo.svg"
cp crates/crossh-assets/assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf \
    "$DESTINATION/fonts/ibm-plex-sans/"
cp crates/crossh-assets/assets/fonts/ibm-plex-sans/IBMPlexSans-SemiBold.ttf \
    "$DESTINATION/fonts/ibm-plex-sans/"
cp crates/crossh-assets/assets/fonts/lilex/Lilex-Regular.ttf "$DESTINATION/fonts/lilex/"
cp crates/crossh-assets/assets/fonts/lilex/Lilex-Bold.ttf "$DESTINATION/fonts/lilex/"
