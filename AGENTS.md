# Crossh Agent Instructions

## Icon Assets

- Every SVG under `assets/icons/` must be an unmodified official SVG downloaded from the pinned Lucide release `1.27.0`.
- The source release is https://github.com/lucide-icons/lucide/tree/1.27.0/icons and the raw source pattern is `https://raw.githubusercontent.com/lucide-icons/lucide/1.27.0/icons/<name>.svg`.
- Do not hand-write, redraw, simplify, reformat, or manually edit icon path data. Additions and replacements must use the canonical Lucide filename and the exact downloaded file.
- When a Lucide icon is renamed, update the local `IconName` mapping and asset loader together. The current `CircleX` mapping intentionally follows Lucide's canonical `circle-x.svg` filename.
- Keep the pinned release and third-party attribution in sync when updating the icon set. See `THIRD_PARTY_NOTICES.md`.

## Sandbox-Aware Command Execution

- The workspace sandbox may permit source changes while denying writes to `.git` and external network access.
- When a command is predictably blocked by those boundaries, request `sandbox_permissions: require_escalated` on the first attempt instead of retrying the same command in the sandbox.
- Commands that normally require first-attempt escalation include `git commit`, `git tag`, `git push`, release operations, `git fetch`, `git ls-remote`, `gh`, and dependency downloads that need network access.
- Keep read-only inspection and ordinary source work in the sandbox when possible: `rg`, `git status`, `git diff`, `cargo fmt`, cached builds, and cached tests.
- Use a narrow `prefix_rule` for elevated commands. Never use a shell-wide prefix such as `zsh`, `sh`, `bash`, `python`, or an unrestricted `git` prefix.
- Use explicit working directories and targets for elevated Git or release operations. Do not broaden the target scope through unresolved globs or shell substitutions.
- Do not elevate destructive commands without explicit user authorization. Keep the user-visible justification and automated command review in place for every elevated operation.
