#!/bin/sh
# Enforce the layering rules that are easy to break during feature work.

set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
failure=0

check_absent() {
    label=$1
    pattern=$2
    shift 2

    matches=$(rg -n --glob '*.rs' "$pattern" "$@" 2>/dev/null || true)
    if [ -n "$matches" ]; then
        printf '%s\n' "architecture violation: $label"
        printf '%s\n' "$matches"
        failure=1
    fi
}

check_absent \
    "logic crates import GPUI" \
    '(use|extern crate).*gpui|gpui::' \
    "$repo_root/crates/crossh-core" \
    "$repo_root/crates/crossh-assets" \
    "$repo_root/crates/crossh-ssh" \
    "$repo_root/crates/crossh-terminal" \
    "$repo_root/crates/crossh-update"

check_absent \
    "logic crates import the application or shared UI crate" \
    'crossh_ui|crossh::|crate::features|crate::shared' \
    "$repo_root/crates/crossh-core" \
    "$repo_root/crates/crossh-assets" \
    "$repo_root/crates/crossh-ssh" \
    "$repo_root/crates/crossh-terminal" \
    "$repo_root/crates/crossh-update"

check_absent \
    "main.rs contains infrastructure implementation details" \
    '(^|[[:space:]])(log::|std::fs|std::io|std::panic)|trim_log|TeeWriter' \
    "$repo_root/src/main.rs"

check_absent \
    "workspace matches over concrete pane variants" \
    'enum[[:space:]]+Pane|Pane::(Terminal|Sftp|Forward)' \
    "$repo_root/src/features/workspace"

check_absent \
    "shared i18n owns persisted application settings" \
    'AppSettings|std::fs|std::path|settings\.toml' \
    "$repo_root/src/shared/i18n.rs"

check_absent \
    "standalone updater includes application source with #[path]" \
    '#\[path' \
    "$repo_root/src/bin/crossh-updater.rs"

if [ "$failure" -ne 0 ]; then
    exit 1
fi

echo "architecture checks passed"
