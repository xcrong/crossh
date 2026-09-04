# Crossh Architecture

Crossh follows the same useful split as Zed's `terminal` and
`terminal_view` crates: the workspace application owns GPUI composition, while
data, protocols, and external-system work live in packages that cannot import
the UI.

```text
crossh (application + feature views)
  -> crossh-ui -> crossh-assets
  -> crossh-ui-component -> crossh-ui-base -> (gpui only)
  -> crossh-ui-component -> crossh-ui
  -> crossh-terminal -> crossh-core
  -> crossh-update
  -> crossh-core

crossh-git (standalone Git Viewer)
  -> crossh-ui-component -> crossh-ui
  -> crossh-ui -> crossh-assets
  -> crossh-core

crossh-note (standalone Note Viewer)
  -> crossh-ui-component -> crossh-ui
  -> crossh-ui -> crossh-assets
  -> crossh-note

crossh-core       -> no GPUI, no application crate; Git command, status, branch,
                      history, stash, conflict, and commit-detail contracts
crossh-terminal   -> no GPUI, terminal settings/events only
crossh-update     -> no GPUI, release/download/install implementation
crossh-assets     -> no GPUI, embedded Crossh icon assets and icon identifiers
crossh-ui         -> GPUI primitives, renderer-independent palette (ex crossh-theme), icon rendering, and the asset-source adapter
crossh-ui-component -> GPUI widgets on top of crossh-ui
crossh-note       -> no GPUI, SQLite note store (WAL, FTS5, tags, pinned) and tag normalization
shared resources  -> external `crossh-assets` directory loaded by every binary
```

## Crate Ownership

- `crossh-core`: terminal-neutral contracts and title helpers, command history/background tasks, Git command/diff parsing, local branch inspection/switching, stash and conflict operations, the shared `git_status`, `git_branch`, `git_history`, `git_stash`, and `git_conflict` parsers.
- `crossh-terminal`: terminal-owned settings and events. It is the model boundary consumed by the GPUI terminal view.
- `crossh-update`: release manifest validation, HTTPS downloads, checksum verification, archive installation, and the standalone updater hand-off.
- `crossh-assets`: UI-neutral Lucide SVG storage, shared external-resource discovery, debug embedded fallback, shared icon identifiers, and asset integrity tests. Its files live under `crates/crossh-assets/assets/icons/`.
- `crossh-ui`: reusable GPUI widgets, context menus, renderer-independent palette (`palette.rs`, migrated from `crossh-theme`), icon rendering, and the `AssetSource` adapter backed by the shared external resource directory.
- `crossh-ui-component`: reusable stateless GPUI control kit (buttons, badges, status metrics, avatars, tooltips, toasts, layout helpers, shared tabs, and status-bar shells) layered on `crossh-ui` and `crossh-ui-base`.
- `crossh-ui-base`: unstyled GPUI behavior and geometry foundation (button behavior, popup placement, list selection); gpui-only, no theme, no application state.
- `crossh`: process startup plus user-facing feature views and GPUI adapters. `crossh git` and the workspace status-bar Git entry delegate to the sibling `crossh-git` binary; `crossh note` and the workspace status-bar Note entry delegate to the sibling `crossh-note` binary; `features/terminal/view.rs` is the `terminal_view`-style host around Zed's terminal foundation.
- `crossh-git`: standalone Git Viewer entry point. It owns the Git window source and reuses the same GPUI and UI dependencies, but does not initialize terminal, workspace, or settings features.
- `crossh-note`: standalone Note Viewer entry point. It owns the Note window source (list/search/tags, `crossh-editor` input state + Markdown preview) and reuses the same GPUI/UI dependencies, but does not initialize terminal, workspace, or settings features. Its pure logic lives in `crossh-note`.
- `features/settings`: application settings persistence and settings window.
- `features/git_launcher`: Git CLI parsing and fire-and-forget startup of the sibling `crossh-git` process.
- `features/note_launcher`: Note CLI parsing and fire-and-forget startup of the sibling `crossh-note` process.
- `src/features/git`: Git Viewer session state, GPUI adapter, window, input, and rendering for Changes, History, Branches, Stashes, and conflict resolution; `session.rs` is UI-independent and `window.rs` owns GPUI task adaptation. This source is not mounted by the `crossh` application binary and is owned by the standalone `crossh-git` entry point.
- `src/features/note`: Note Viewer window, Markdown preview, and editor adapter; `window.rs` owns state and GPUI, `markdown.rs` is UI-independent. This source is not mounted by the `crossh` application binary and is owned by the standalone `crossh-note` entry point.
- `features/updates`: update controller and update presentation only.
`AppShell` is the GPUI composition root for the workspace feature. Session
collections and terminal split state live in `WorkspaceState` and
`SessionRegistry`.

## Boundary Rules

1. `crossh-core`, `crossh-assets`, `crossh-terminal`, and `crossh-update` must not import `gpui`, `gpui_platform`, or `crossh-ui`. `crossh-ui-base` must not import `crossh-ui`, `crossh-ui-component`, or any application crate.
2. Feature views consume crate-root APIs, not private implementation modules from `crossh-update`.
3. Feature settings stay next to the feature that owns their behavior; the persistence layer composes snapshots without becoming the settings owner.
4. `main.rs` is assembly only: logging, runtime warm-up, platform setup, Zed global initialization, key bindings, and window boot.
5. The updater binary depends on `crossh-update` directly. It must not include application source with `#[path]`.
6. `crossh-git` and `crossh-note` are the only standalone entry points allowed to include application modules with `#[path]`; they must keep their boot paths limited to the Git/Note features respectively. Release packaging places all binaries beside one shared `crossh-assets` directory; non-arm64-macOS packaging is built and verified only by GitHub Actions.
7. Git protocol parsing and repository operations, including branch switching, stash lifecycle, and conflict resolution, belong in `crossh-core`; Git Viewer state transitions belong in the UI-independent session layer; GPUI views must not become the owner of Git semantics.
8. Shared GPUI chrome such as `TabStrip`, `TabItem`, `StatusBar`, `StatusMetric`, and Toast/Toaster visuals belongs in `crossh-ui-component`; the component layer owns layout and visual tokens, while feature views own state, content, and callbacks.
9. Shared behavior and geometry (button activation, popup placement, list selection) belongs in `crossh-ui-base`; visual tokens stay in `crossh-ui-component`.

The crate graph is the enforcement mechanism. A logic change that attempts to
reach into GPUI fails at dependency resolution or compilation instead of
relying only on directory conventions.
Each crate README records its responsibility, boundary, public entry points,
and quick verification command for focused validation.

## 决策记录

已移除独立  目录。架构以代码（crate 依赖图 + `scripts/check-architecture.sh`）为准。
