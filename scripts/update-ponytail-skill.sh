#!/bin/sh
# Re-vendor the ponytail skill from upstream at the pinned commit.
#
# Usage:
#   scripts/update-ponytail-skill.sh           # refresh from PINNED_COMMIT
#   scripts/update-ponytail-skill.sh <commit>  # bump to a new upstream commit
#
# After bumping, copy the new hash into PINNED_COMMIT below and update the
# ponytail entry in THIRD_PARTY_NOTICES.md.

set -eu

PINNED_COMMIT="356918eba965ee1eac64bd3a7f0dd02108350de5"
UPSTREAM="https://raw.githubusercontent.com/DietrichGebert/ponytail"
SKILL="ponytail"

rev="${1:-$PINNED_COMMIT}"
repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
dest="$repo_root/.agents/skills/$SKILL/SKILL.md"

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT INT TERM
curl -sfL "$UPSTREAM/$rev/skills/$SKILL/SKILL.md" -o "$tmp"
mkdir -p "$(dirname -- "$dest")"
mv "$tmp" "$dest"
trap - EXIT INT TERM

echo "vendored $SKILL@$rev -> $dest"
if [ "$rev" != "$PINNED_COMMIT" ]; then
    echo "bumped: set PINNED_COMMIT in $0 and THIRD_PARTY_NOTICES.md to $rev"
fi
