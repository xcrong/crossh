#!/bin/sh
# Enforce the layering rules that are easy to break during feature work.

set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
failure=0

# Keep exceptions explicit and local to this check. This file is a 2186-line
# local fork of Zed's terminal_view code, so it remains maintained as upstream
# third-party code rather than being split locally.
# crossh-editor vendors longbridge/gpui-component crates/base (commit b2b7e41),
# keeping its large pure-input engine files unsplit to preserve upstream diffability.
size_whitelist='src/features/terminal/zed_view/terminal_element.rs
crates/terminal/src/terminal.rs
crates/crossh-editor/src/input/base/state.rs
crates/crossh-editor/src/input/base/element.rs'

is_size_whitelisted() {
    path=$1
    case "$path" in
        crates/crossh-editor/*) return 0 ;;
    esac
    for allowed_path in $size_whitelist; do
        if [ "$path" = "$allowed_path" ]; then
            return 0
        fi
    done
    return 1
}

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
    "$repo_root/crates/crossh-terminal" \
    "$repo_root/crates/crossh-update" \
    "$repo_root/crates/crossh-theme"

check_absent \
    "logic crates import the application or shared UI crate" \
    'crossh_ui|crossh::|crate::features|crate::shared' \
    "$repo_root/crates/crossh-core" \
    "$repo_root/crates/crossh-assets" \
    "$repo_root/crates/crossh-terminal" \
    "$repo_root/crates/crossh-update" \
    "$repo_root/crates/crossh-theme"

check_absent \
    "workspace matches over concrete pane variants" \
    'enum[[:space:]]+Pane|Pane::Terminal' \
    "$repo_root/src/features/workspace"

check_absent \
    "shared i18n owns persisted application settings" \
    'AppSettings|std::fs|std::path|settings\.toml' \
    "$repo_root/src/shared/i18n.rs"

check_absent \
    "shared application logic imports GPUI or the UI crate" \
    '(use|extern crate).*gpui|gpui::|crossh_ui' \
    "$repo_root/src/shared"

check_absent \
    "standalone updater includes application source with #[path]" \
    '#\[path' \
    "$repo_root/src/bin/crossh-updater.rs"

size_files=$(find "$repo_root/src" "$repo_root"/crates/*/src -type f -name '*.rs' -print 2>/dev/null || true)
for file in $size_files; do
    line_count=$(wc -l < "$file")
    relative_path=${file#"$repo_root"/}
    if [ "$line_count" -gt 2000 ] && ! is_size_whitelisted "$relative_path"; then
        printf '%s\n' "architecture violation: file exceeds 2000 lines: $relative_path"
        failure=1
    fi
done

if [ "$failure" -ne 0 ]; then
    exit 1
fi

echo "architecture checks passed"
