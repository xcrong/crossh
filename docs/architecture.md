# Crossh Architecture

Crossh follows the same useful split as Zed's `terminal` and
`terminal_view` crates: the workspace application owns GPUI composition, while
data, protocols, and external-system work live in packages that cannot import
the UI.

```text
crossh (application + feature views)
  -> crossh-agent -> crossh-ai-sdk
  -> crossh-theme
  -> crossh-ui -> crossh-assets
  -> crossh-terminal -> crossh-core
  -> crossh-ssh      -> crossh-core
  -> crossh-update
  -> crossh-core

crossh-git (standalone Git Viewer)
  -> crossh-ui -> crossh-assets
  -> crossh-theme
  -> crossh-core

crossh-agent (standalone terminal agent)
  -> crossh-agent -> crossh-ai-sdk
  -> crossh-ssh
  -> crossh-theme
  -> crossh-core

crossh-core       -> no GPUI, no application crate
crossh-agent      -> no GPUI, agent loop, tools, sessions, and policy
crossh-ai-sdk     -> no GPUI, provider-neutral messages, HTTP/SSE, and wire adapters
crossh-ssh        -> no GPUI, transport implementation only
crossh-terminal   -> no GPUI, terminal settings/events only
crossh-update     -> no GPUI, release/download/install implementation
crossh-theme      -> no GPUI, renderer-independent color tokens
crossh-assets     -> no GPUI, embedded Crossh icon assets and icon identifiers
crossh-ui         -> GPUI primitives and the asset-source adapter
shared resources  -> external `crossh-assets` directory loaded by every binary
```

## Crate Ownership

- `crossh-core`: OpenSSH config parsing, terminal-neutral contracts and title helpers, command history/background tasks, Git parsing, and shared connection state.
- `crossh-agent`: workspace-scoped tools, project context/skill/prompt discovery, JSONL session persistence, persisted agent configuration, and the agent loop. Resource discovery stays UI-neutral; the terminal CLI owns command presentation and prompt injection.
- `crossh-ai-sdk`: canonical provider-neutral messages, HTTP and SSE transport, OpenAI Chat/Responses and Anthropic Messages adapters, reasoning-summary normalization, and the `ProviderAdapter` extension point for future compatible providers.
- `crossh-theme`: renderer-independent Crossh color tokens shared by the GPUI and ratatui surfaces.
- `crossh-ssh`: `russh` authentication and channels, connection pooling, SFTP, port forwarding, ProxyJump, and the Tokio runtime. Its public API is re-exported from the crate root; implementation modules stay private.
- `crossh-terminal`: terminal-owned settings and events. It is the model boundary consumed by the GPUI terminal view.
- `crossh-update`: release manifest validation, HTTPS downloads, checksum verification, archive installation, and the standalone updater hand-off.
- `crossh-assets`: UI-neutral Lucide SVG storage, shared external-resource discovery, debug embedded fallback, shared icon identifiers, and asset integrity tests. Its files live under `crates/crossh-assets/assets/icons/`.
- `crossh-ui`: reusable GPUI widgets, context menus, the GPUI adapter for `crossh-theme`, icon rendering, and the `AssetSource` adapter backed by the shared external resource directory.
- `crossh`: process startup plus user-facing feature views and GPUI adapters. `crossh git` delegates to the sibling `crossh-git` binary; `crossh agent` delegates to the sibling `crossh-agent` binary; `features/terminal/view.rs` is the `terminal_view`-style host around Zed's terminal foundation; `features/connections/entity.rs` is the adapter around `crossh-ssh::ConnectionHandle`.
- `crossh-git`: standalone Git Viewer entry point. It reuses the Git feature source through the same GPUI and UI dependencies but does not initialize SSH, terminal, agent, workspace, or settings features.
- `crossh-agent` binary: standalone interactive terminal agent entry point. It reuses `src/agent_cli.rs` and needs no GPUI; it reads the agent section of the shared `settings.toml` through `crossh_agent::load_agent_settings` and depends only on the pure crates.

Within the application crate:

- `features/workspace`: navigation, tabs, active view, local projects, and pane composition.
- `features/connections`: connection-facing UI, host navigation data, and the GPUI connection entity.
- `features/sftp`: remote file browser/editor UI and SFTP-specific interaction helpers.
- `features/forwarding`: local, remote, and dynamic forwarding UI.
- `features/settings`: application settings persistence and settings window.
- `features/updates`: update controller and update presentation only.

`AppShell` is the GPUI composition root for the workspace feature. Session
collections live in `WorkspaceState` and `SessionRegistry`; connection
configuration and handles live in `ConnectionManager`, while the transport
engine remains in `crossh-ssh`.

## Boundary Rules

1. `crossh-core`, `crossh-agent`, `crossh-ai-sdk`, `crossh-assets`, `crossh-theme`, `crossh-ssh`, `crossh-terminal`, and `crossh-update` must not import `gpui`, `gpui_platform`, or `crossh-ui`.
2. The transport crate communicates with the application through channels and public data types; it never receives a GPUI context or entity.
3. Feature views consume crate-root APIs, not private implementation modules from `crossh-ssh` or `crossh-update`.
4. Feature settings stay next to the feature that owns their behavior; the persistence layer composes snapshots without becoming the settings owner.
5. `main.rs` is assembly only: logging, runtime warm-up, platform setup, Zed global initialization, key bindings, and window boot.
6. The updater binary depends on `crossh-update` directly. It must not include application source with `#[path]`.
7. `crossh-git` and the `crossh-agent` binary are the only standalone entry points allowed to include application modules with `#[path]`; each must keep its boot path limited to the feature it owns. Release packaging places all binaries beside one shared `crossh-assets` directory; non-arm64-macOS packaging is built and verified only by GitHub Actions.

The crate graph is the enforcement mechanism. A logic change that attempts to
reach into GPUI fails at dependency resolution or compilation instead of
relying only on directory conventions.
Each crate README records its responsibility, boundary, public entry points,
and quick verification command for focused validation.

## 决策记录

- [0001: Zed revision and terminal fork](adr/0001-zed-revision-and-terminal-fork.md)
- [0002: Logic/UI layering](adr/0002-logic-ui-layering.md)
- [0003: Agent logic and view split](adr/0003-agent-logic-and-view-split.md)
- [0004: Feature-owned settings](adr/0004-feature-owned-settings.md)
- [0005: Standalone updater](adr/0005-standalone-updater.md)
- [0006: Executable testing contracts](adr/0006-executable-testing-contracts.md)
- [0007: Workspace panel composition](adr/0007-workspace-panel-composition.md)
- [0008: Standalone Git Viewer binary](adr/0008-standalone-git-viewer.md)
- [0009: Standalone agent binary](adr/0009-standalone-agent-binary.md)
