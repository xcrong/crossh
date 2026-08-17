#!/usr/bin/env bash
# Print the [package] version of a Cargo manifest.
#
# Usage:
#   scripts/package-version.sh [manifest]
#
# The manifest defaults to the workspace root Cargo.toml. release.sh and the
# release workflow both call this script so version extraction lives in one
# place.

set -euo pipefail

manifest=${1:-Cargo.toml}

awk '
    /^\[package\][[:space:]]*$/ { in_package = 1; next }
    /^\[/ { in_package = 0 }
    in_package && /^version[[:space:]]*=/ {
        value = $0
        sub(/^[^\"]*\"/, "", value)
        sub(/\".*$/, "", value)
        print value
        exit
    }
' "$manifest"