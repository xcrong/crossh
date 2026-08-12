#!/usr/bin/env bash
# Update the workspace version, commit, tag, and optionally push.
#
# Usage:
#   scripts/release.sh 0.11.0
#   scripts/release.sh 0.11.0 --push
#
# `--allow-dirty` is intended only for a prepared version bump. Without it,
# the release must start from a clean worktree.

set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

usage() {
    printf 'usage: %s <version> [--push] [--allow-dirty]\n' "$0" >&2
}

die() {
    printf 'release error: %s\n' "$1" >&2
    exit 1
}

if [[ $# -lt 1 ]]; then
    usage
    exit 2
fi

version_arg=$1
shift
version=${version_arg#v}
push_release=0
allow_dirty=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --push)
            push_release=1
            ;;
        --allow-dirty)
            allow_dirty=1
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage
            die "unknown option: $1"
            ;;
    esac
    shift
done

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    die "version must be a stable semantic version such as 0.11.0"
fi

tag="v$version"
branch="$(git branch --show-current)"
[[ -n "$branch" ]] || die "release requires a named branch"
git remote get-url origin >/dev/null 2>&1 || die "remote 'origin' is not configured"

manifest_paths=(Cargo.toml crates/*/Cargo.toml)

read_package_version() {
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
    ' "$1"
}

ensure_versions_match() {
    local manifest current reference=''
    for manifest in "${manifest_paths[@]}"; do
        current="$(read_package_version "$manifest")"
        [[ -n "$current" ]] || die "no package version found in $manifest"
        if [[ -z "$reference" ]]; then
            reference=$current
        elif [[ "$current" != "$reference" ]]; then
            die "workspace package versions do not match: $manifest has $current, expected $reference"
        fi
    done
}

is_allowed_path() {
    case "$1" in
        Cargo.lock|Cargo.toml|README.md|scripts/release.sh|crates/*/Cargo.toml)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

check_worktree_paths() {
    local status_line path
    while IFS= read -r status_line; do
        [[ -n "$status_line" ]] || continue
        path=${status_line:3}
        is_allowed_path "$path" || die "unexpected worktree change: $path"
    done < <(git status --porcelain=v1 --untracked-files=all)
}

if [[ "$allow_dirty" -eq 0 ]] && [[ -n "$(git status --porcelain)" ]]; then
    die "worktree is not clean; commit or stash changes, or use --allow-dirty for a prepared version bump"
fi
check_worktree_paths

ensure_versions_match

for manifest in "${manifest_paths[@]}"; do
    temp_path="$manifest.tmp.$$"
    awk -v target="$version" '
        /^\[package\][[:space:]]*$/ { in_package = 1; print; next }
        /^\[/ { in_package = 0 }
        in_package && /^version[[:space:]]*=/ {
            sub(/\"[^\"]*\"/, "\"" target "\"")
        }
        { print }
    ' "$manifest" > "$temp_path"
    mv "$temp_path" "$manifest"
done

ensure_versions_match

echo "==> sync Cargo.lock"
cargo check --workspace

check_worktree_paths
git diff --check

if git rev-parse --verify --quiet "refs/tags/$tag" >/dev/null; then
    die "local tag already exists: $tag"
fi
if ! remote_tag="$(git ls-remote --tags origin "refs/tags/$tag")"; then
    die "could not query origin for tag $tag"
fi
[[ -z "$remote_tag" ]] || die "remote tag already exists: $tag"

echo "==> stage release files"
git add -- Cargo.lock README.md scripts/release.sh "${manifest_paths[@]}"
git diff --cached --check

echo "==> commit release"
git commit --no-verify -m "chore: release $tag"

echo "==> tag $tag"
git tag -a "$tag" -m "Release $tag"

if [[ "$push_release" -eq 1 ]]; then
    echo "==> push $branch"
    git push origin "$branch"
    echo "==> push $tag"
    git push origin "$tag"
    echo "release $tag pushed; GitHub Actions will build and publish the release"
else
    echo "release $tag committed and tagged locally"
    echo "push with: git push origin $branch && git push origin $tag"
fi
