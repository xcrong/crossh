#!/bin/sh
# Enforce the layering rules that are easy to break during feature work.
# POSIX sh + grep/find/wc/awk only: CI runners must not install ripgrep.

set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
failure=0

# Keep exceptions explicit and local to this check. These files are maintained
# as upstream-derived code and remain unsplit to preserve diffability.
# terminal_element.rs is deliberately absent: it is no longer oversized because
# it was split positionally only (see the split map in its header) into
# terminal_element_{layout,paint,input}.rs.
size_whitelist='crates/terminal/src/terminal.rs
crates/terminal/src/alacritty/hyperlinks.rs
crates/crossh-editor/src/input/base/state.rs
crates/crossh-editor/src/input/base/element.rs
crates/crossh-editor/src/scrollbar.rs
crates/crossh-editor/src/text/node.rs
crates/crossh-editor/src/text_selection.rs'

is_size_whitelisted() {
    path=$1
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

    matches=$(grep -R -n -E --include='*.rs' -- "$pattern" "$@" 2>/dev/null || true)
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
    "$repo_root/crates/crossh-session" \
    "$repo_root/crates/crossh-update"

check_absent \
    "logic crates import the application or shared UI crate" \
    'crossh_ui|crossh::|crate::features|crate::app' \
    "$repo_root/crates/crossh-core" \
    "$repo_root/crates/crossh-assets" \
    "$repo_root/crates/crossh-terminal" \
    "$repo_root/crates/crossh-session" \
    "$repo_root/crates/crossh-update"

# pane 身份是 daemon 快照的 `PaneId` + `PaneStatus`，工作区只按 id 持有视图，
# 不再定义具体 pane 枚举。
check_absent \
    "workspace defines a concrete Pane enum" \
    'enum[[:space:]]+Pane([^a-zA-Z]|$)' \
    "$repo_root/src/features/workspace"

check_absent \
    "core i18n owns persisted application settings" \
    'AppSettings|std::fs|std::path|settings\.toml' \
    "$repo_root/crates/crossh-core/src/i18n.rs"

check_absent \
    "application bootstrap imports GPUI or the UI crate" \
    '(use|extern crate).*gpui|gpui::|crossh_ui' \
    "$repo_root/src/app"

check_absent \
    "standalone binaries include application source with #[path]" \
    '#\[path' \
    "$repo_root/src/bin"

# `#[path]` 在 src 下仅允许两处：shell 的平台二选一（link_preview）
# 与 settings 窗口的同名 render 模块；其他新增一律视为回抄。
path_uses=$(grep -R -n --include='*.rs' -- '#\[path' "$repo_root/src" 2>/dev/null || true)
if [ -n "$path_uses" ]; then
    unexpected=$(printf '%s\n' "$path_uses" | grep -v -E 'src/features/workspace/shell/mod\.rs|src/features/settings/window\.rs' || true)
    if [ -n "$unexpected" ]; then
        printf '%s\n' "architecture violation: unexpected #[path] under src (allowed: shell/mod.rs link_preview gate, settings/window.rs render)"
        printf '%s\n' "$unexpected"
        failure=1
    fi
fi

check_absent \
    "crossh-ui-base imports upper layers" \
    'crossh_ui_component|crossh-ui-component|crossh_ui::|crate::features|crate::shared|crossh_core|crossh-note|crossh-editor' \
    "$repo_root/crates/crossh-ui-base"

check_absent \
    "crossh-ui-base depends on themed or app crates (Cargo level)" \
    'crossh-ui[ "=]|crossh_ui |crossh-core|crossh-terminal|crossh-update|crossh-assets|crossh-note|crossh-editor' \
    "$repo_root/crates/crossh-ui-base/Cargo.toml"

check_absent \
    "crossh-ui-base exposes pub fields across the seam" \
    '^[[:space:]]*pub [a-z_][a-zA-Z0-9_]*:' \
    "$repo_root/crates/crossh-ui-base/src"

check_absent \
    "crossh-ui-base abbreviates context as ctx" \
    '(^|[^a-zA-Z0-9_])ctx([^a-zA-Z0-9_]|$)' \
    "$repo_root/crates/crossh-ui-base/src"

check_absent \
    "upper UI layers bypass the base seam with private module paths" \
    'crossh_ui_base::button::|crossh_ui_base::positioner::|crossh_ui_base::list_state::' \
    "$repo_root/crates/crossh-ui-component" \
    "$repo_root/crates/crossh-ui" \
    "$repo_root/src"

# Cargo only applies workspace lints to packages that opt in. Keep new members
# from silently dropping the shared baseline in the root manifest.
# Requires the exact two-line block `[lints]` + `workspace = true`.
has_workspace_lints() {
    awk 'prev == "[lints]" && $0 == "workspace = true" { found = 1 } { prev = $0 } END { exit !found }' "$1"
}
for manifest in "$repo_root/Cargo.toml" "$repo_root"/crates/*/Cargo.toml; do
    if ! has_workspace_lints "$manifest"; then
        printf '%s\n' "architecture violation: manifest does not inherit workspace lints: ${manifest#"$repo_root"/}"
        failure=1
    fi
done

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
