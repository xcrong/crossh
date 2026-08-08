#!/usr/bin/env bash
# Generate the machine-readable release manifest consumed by Crossh.
#
# Usage: scripts/generate-update-manifest.sh <version> <owner/repository> [dist]
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION="${1:?usage: scripts/generate-update-manifest.sh <version> <owner/repository> [dist]}"
REPOSITORY="${2:?usage: scripts/generate-update-manifest.sh <version> <owner/repository> [dist]}"
DIST="${3:-dist}"
OUTPUT="$DIST/stable.json"

mkdir -p "$DIST"

artifact_json() {
    local filename=$1
    local format=$2
    local path="$DIST/$filename"
    if [ ! -f "$path" ]; then
        echo "missing release artifact: $path" >&2
        exit 1
    fi
    local checksum
    if command -v sha256sum >/dev/null 2>&1; then
        checksum="$(sha256sum "$path" | awk '{print $1}')"
    else
        checksum="$(shasum -a 256 "$path" | awk '{print $1}')"
    fi
    local size
    size="$(wc -c < "$path" | tr -d '[:space:]')"
    local url="https://github.com/$REPOSITORY/releases/download/v$VERSION/$filename"
    printf '    "url": "%s",\n    "filename": "%s",\n    "format": "%s",\n    "sha256": "%s",\n    "size": %s' \
        "$url" "$filename" "$format" "$checksum" "$size"
}

emit_target() {
    local key=$1
    local filename=$2
    local format=$3
    if [ "$first" -eq 0 ]; then
        printf ',\n'
    fi
    first=0
    printf '  "%s": {\n' "$key"
    artifact_json "$filename" "$format"
    printf '\n  }'
}

{
    printf '{\n'
    printf '  "schema": 1,\n'
    printf '  "version": "%s",\n' "$VERSION"
    printf '  "notes": "",\n'
    printf '  "release_url": "https://github.com/%s/releases/tag/v%s",\n' "$REPOSITORY" "$VERSION"
    printf '  "targets": {\n'

    first=1
    emit_target "macos-aarch64" "crossh-$VERSION-aarch64-macos.zip" "zip"
    emit_target "macos-x86_64" "crossh-$VERSION-x86_64-macos.zip" "zip"
    emit_target "linux-aarch64" "crossh-$VERSION-linux-aarch64.AppImage" "appimage"
    emit_target "linux-x86_64" "crossh-$VERSION-linux-x86_64.AppImage" "appimage"
    emit_target "windows-x86_64" "crossh-$VERSION-windows-x86_64.zip" "zip"

    optional_windows_arm="crossh-$VERSION-windows-aarch64.zip"
    if [ -f "$DIST/$optional_windows_arm" ]; then
        emit_target "windows-aarch64" "$optional_windows_arm" "zip"
    fi

    printf '\n  }\n}\n'
} > "$OUTPUT"

echo "generated $OUTPUT"
