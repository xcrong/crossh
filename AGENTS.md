# Crossh Agent Instructions

## Icon Assets

- Every SVG under `assets/icons/` must be an unmodified official SVG downloaded from the pinned Lucide release `1.27.0`.
- The source release is https://github.com/lucide-icons/lucide/tree/1.27.0/icons and the raw source pattern is `https://raw.githubusercontent.com/lucide-icons/lucide/1.27.0/icons/<name>.svg`.
- Do not hand-write, redraw, simplify, reformat, or manually edit icon path data. Additions and replacements must use the canonical Lucide filename and the exact downloaded file.
- When a Lucide icon is renamed, update the local `IconName` mapping and asset loader together. The current `CircleX` mapping intentionally follows Lucide's canonical `circle-x.svg` filename.
- Keep the pinned release and third-party attribution in sync when updating the icon set. See `THIRD_PARTY_NOTICES.md`.

## Zed / GPUI Dependency Source

- This project depends on the `gpui` and `gpui_platform` crates directly from the Zed git repository (see `Cargo.toml` for the pinned revision).
- The checked-out source lives under `~/.cargo/git/checkouts/zed-<hash>/<rev>/` (e.g. `crates/gpui/src/`, `crates/gpui_platform/src/`). The `<hash>` is Cargo's stable path hash and `<rev>` is the git revision from `Cargo.toml`.
- Do NOT hardcode the revision: it changes whenever the Zed dependency is updated. Always resolve the current revision by reading the `rev` key in `Cargo.toml`, then locate the matching subdirectory under `~/.cargo/git/checkouts/zed-*/`.
- The full git database is also cached at `~/.cargo/git/db/zed-<hash>/`.

## GPUI Skill

- This project has a local GPUI skill at `.agents/skills/gpui/SKILL.md` covering GPUI concepts, patterns, and APIs (actions/keybindings, async tasks, context management, entities, focus, layout, testing).
- Load the `gpui` skill whenever working with GPUI framework code or answering GPUI API questions. It also points to the Zed source under `~/.cargo/git/checkouts/` for authoritative API lookups.
- For Terminal/PTY debugging, see also the `terminal-pty-capture` skill at `.agents/skills/terminal-pty-capture/SKILL.md`.

## Engineering Notes

- Reusable debugging findings live under `docs/engineering-notes/`. Start with `docs/engineering-notes/README.md`, then load only the topic note whose keywords match the current symptom; do not load the whole collection by default.
- After resolving a non-obvious, reproducible failure, add or update a concise note with the symptom, root cause, durable rule, verification method, and search keywords.

## Engineering Rules (derived from Zed's architecture)

- **Logic must not depend on UI.** Pure-logic modules (`shared/terminal/`, SSH sessions, protocols, engines) must contain zero `gpui` imports. GPUI views depend on logic; logic never depends on views. To verify, treat any `gpui` import in a logic module as a layering violation.
- **Keep the app entry point thin.** `main.rs` is assembly only: window setup, keybindings, boot. Utilities (logging, trimming, panic hooks) live in `infrastructure/`, not in `main.rs`.
- **Split vertical features, then split logic and view inside each.** A feature owns its state, its settings, and its UI together. Within a feature, separate the pure logic from the `gpui` view layers (e.g. `engine.rs` + `render.rs` + `view.rs`, mirroring Zed's `terminal` vs `terminal_view` crates).
- **Each feature ships its own settings.** Do not centralize all settings in one place; settings for a feature live next to that feature (mirrors Zed's per-crate `*_settings.rs`).
- **Keep files small and focused.** When a view file outgrows ~1–2k lines, split rendering, input handling, and state into separate modules rather than growing one file.
- **Depend on abstractions, not concrete panes.** A workspace/container should consume traits or common interfaces (mirrors Zed's `item.rs` / `pane.rs`), not `match` over an enum of concrete view types.
- **No backward-compatibility bloat.** This project evolves fast and has no historical baggage — do not write redundant shims, deprecated paths, or compatibility code "just in case" older APIs are still used. Keep every layer behind the current, intended contract only.

## Size and Language Discipline

- **Keep source files under 2000 lines.** `scripts/check-architecture.sh` rejects Rust files above this limit under `src/` and `crates/*/src/`. Keep any exception explicit in that script's whitelist and document why it is maintained.
- **Match the surrounding language.** Preserve the existing language of comments and documentation when editing a file. New documentation defaults to Chinese; `AGENTS.md` and `docs/architecture.md` remain in English.
- **Code is the spec.** Structural decisions are enforced by crate graph + `scripts/check-architecture.sh`, not separate docs.

## Responsive UI

- **Responsive layout is a default requirement for every UI change.** New and modified views must remain usable from the application's declared minimum window size through the standard window size, without clipped controls, overlapping text, or unreachable content.
- Base layout decisions on the space actually available to the component. A workspace view must account for visible sidebars, rails, and panels instead of using the full window width as a proxy for its own width.
- Define explicit compact-layout behavior for controls that cannot safely shrink. Prefer reflowing rows into columns, reducing nonessential spacing, and enabling scrolling over shrinking text or interactive targets below usable sizes.
- Keep breakpoint decisions deterministic and cover them with focused pure tests where practical. For material UI changes, verify both compact and standard layouts visually when the local platform supports it.

## Build Cache Discipline

- Use the project's default `target/` directory for Cargo builds, tests, checks, and releases. Do not set `CARGO_TARGET_DIR` or create an independent compiler cache unless the user explicitly requests it.
- If the default target is locked by another Cargo process, wait for it or report the condition; do not switch to a separate target as a workaround.

## Test Execution Discipline (Local Resource Saving)

- Avoid `cargo test --workspace` full-suite runs on the local machine by default. Full workspace tests are expensive and should be reserved for pre-release or when explicitly requested.
- For routine changes, run targeted tests only: the crate or module directly affected (`cargo test -p <crate>`, `cargo test --lib <filter>`, or a single test binary). Choose the smallest scope that covers the changed behavior.
- Documentation-only, formatting, or single-file mechanical refactors do not require a test run; rely on `cargo fmt --check` / `cargo clippy` / `scripts/check-architecture.sh` instead.
- When a full green suite is required as a hard gate (e.g. simplification-audit direct fixes: `Hard gate for direct fixes: a batch may be committed only after a fully green cargo test --workspace`), treat that as an explicit exception and state the reason when running it.

## Local Workspace and Platform Responsibility

- Work directly in the current checkout. Do not create Git worktrees, duplicate the repository, or copy the build cache unless the user explicitly requests that isolation strategy.
- The local development machine is responsible only for macOS arm64 behavior and platform-independent logic. Never run local Cargo builds, checks, tests, or clippy commands for `x86_64-apple-darwin`, Linux, or Windows targets. Do not install emulators, cross-compile toolchains, local compatibility layers, or target-specific dependencies merely to investigate or claim non-arm64-macOS verification; these consume local disk and compute without providing authoritative platform validation.
- macOS x86_64-, Linux-, and Windows-specific behavior is verified by GitHub Actions. Changes that affect those platforms must add or update the corresponding CI coverage, and must not be reported as platform-verified until the relevant Actions jobs pass.
- Local verification should still run all cached, platform-independent tests that are available on macOS. A platform-specific test skipped locally must be named explicitly in the handoff together with the CI job that owns it.

## Simplification Audits (Manual)

- Run `find-simplifications` audits ad hoc when there is no active development task or whenever the user asks to find things to simplify. One round per trigger; do not run it in the background or as a standing habit inside unrelated sessions.
- Findings are delivered in-session. Persist a report under `docs/audit/yyyy-mm-dd-simplification-audit.md` only when it carries unresolved items needing product input or cross-session tracking; once those items are resolved, the report is pruned like any other expired document.

## Sandbox-Aware Command Execution

- The workspace sandbox may permit source changes while denying writes to `.git` and external network access.
- When a command is predictably blocked by those boundaries, request `sandbox_permissions: require_escalated` on the first attempt instead of retrying the same command in the sandbox.
- Commands that normally require first-attempt escalation include `git commit`, `git tag`, `git push`, release operations, `git fetch`, `git ls-remote`, `gh`, and dependency downloads that need network access.
- Keep read-only inspection and ordinary source work in the sandbox when possible: `rg`, `git status`, `git diff`, `cargo fmt`, cached builds, and cached tests.
- Use a narrow `prefix_rule` for elevated commands. Never use a shell-wide prefix such as `zsh`, `sh`, `bash`, `python`, or an unrestricted `git` prefix.
- Use explicit working directories and targets for elevated Git or release operations. Do not broaden the target scope through unresolved globs or shell substitutions.
- Do not elevate destructive commands without explicit user authorization. Keep the user-visible justification and automated command review in place for every elevated operation.
