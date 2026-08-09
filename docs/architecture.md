# Crossh Architecture

Crossh follows the same useful split as Zed's `terminal` and
`terminal_view` crates: the workspace application owns GPUI composition, while
data, protocols, and external-system work live in packages that cannot import
the UI.

```text
crossh (application + feature views)
  -> crossh-agent
  -> crossh-ui
  -> crossh-terminal -> crossh-core
  -> crossh-ssh      -> crossh-core
  -> crossh-update
  -> crossh-core

crossh-core       -> no GPUI, no application crate
crossh-agent      -> no GPUI, protocol-neutral agent core and wire adapters
crossh-ssh        -> no GPUI, transport implementation only
crossh-terminal   -> no GPUI, terminal settings/events only
crossh-update     -> no GPUI, release/download/install implementation
crossh-ui         -> GPUI primitives and assets
```

## Crate Ownership

- `crossh-core`: OpenSSH config parsing, terminal-neutral contracts and title helpers, command history/background tasks, Git parsing, and shared connection state.
- `crossh-agent`: canonical agent messages, persisted agent configuration, HTTP transport, and adapters for OpenAI Chat, OpenAI Responses, and Anthropic Messages wire formats.
- `crossh-ssh`: `russh` authentication and channels, connection pooling, SFTP, port forwarding, ProxyJump, and the Tokio runtime. Its public API is re-exported from the crate root; implementation modules stay private.
- `crossh-terminal`: terminal-owned settings and events. It is the model boundary consumed by the GPUI terminal view.
- `crossh-update`: release manifest validation, HTTPS downloads, checksum verification, archive installation, and the standalone updater hand-off.
- `crossh-ui`: reusable GPUI widgets, context menus, theme functions, icons, and the asset source.
- `crossh`: process startup plus user-facing feature views and GPUI adapters. `features/terminal/view.rs` is the `terminal_view`-style host around Zed's terminal foundation; `features/connections/entity.rs` is the adapter around `crossh-ssh::ConnectionHandle`.

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

1. `crossh-core`, `crossh-agent`, `crossh-ssh`, `crossh-terminal`, and `crossh-update` must not import `gpui`, `gpui_platform`, or `crossh-ui`.
2. The transport crate communicates with the application through channels and public data types; it never receives a GPUI context or entity.
3. Feature views consume crate-root APIs, not private implementation modules from `crossh-ssh` or `crossh-update`.
4. Feature settings stay next to the feature that owns their behavior; the persistence layer composes snapshots without becoming the settings owner.
5. `main.rs` is assembly only: logging, runtime warm-up, platform setup, Zed global initialization, key bindings, and window boot.
6. The updater binary depends on `crossh-update` directly. It must not include application source with `#[path]`.

The crate graph is the enforcement mechanism. A logic change that attempts to
reach into GPUI fails at dependency resolution or compilation instead of
relying only on directory conventions.
