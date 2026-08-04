# Crossh Architecture

Crossh is organized by user-facing feature. Technical layers remain inside the
feature boundary where they are feature-specific, while reusable platform code
lives under `infrastructure` and `shared`.

```text
app
  -> features
       -> infrastructure
       -> shared
infrastructure -> shared
shared         -> no feature modules
```

## Module Ownership

- `app`: process startup and window bootstrap only.
- `features/workspace`: navigation, tabs, active view, local projects, and pane composition.
- `features/terminal`: terminal emulation, protocol effects, input delivery, and terminal events.
- `features/connections`: connection-facing UI, host navigation data, and connection manager.
- `features/sftp`: remote file browser and editor.
- `features/forwarding`: local, remote, and dynamic forwarding UI.
- `features/projects`: local project Git state and project-specific services.
- `features/settings`: settings state and settings UI.
- `infrastructure/ssh`: russh transport, channels, authentication, pooling, SFTP worker, and forwarding primitives.
- `infrastructure/local`: local PTY integration.
- `infrastructure/config`: OpenSSH config loading and parsing.
- `shared/terminal`: transport-neutral terminal contracts and protocol parsing.
- `shared/ui`: reusable GPUI primitives, assets, icons, theme, and context menus.

`AppShell` is the GPUI composition root for the workspace feature. Its session
collections live in `WorkspaceState` and `SessionRegistry`; SSH configuration,
host entries, and the reusable connection pool live in `ConnectionManager`.

## Boundary Rules

1. A feature may use another feature's exported API, not its internal `view` or service modules.
2. SSH and local PTY implementations communicate through `shared::terminal` contracts.
3. Infrastructure must not import UI or workspace modules.
4. Terminal notifications carry terminal events to the workspace owner; they do not reach into `AppShell` directly.
5. New cross-feature state should have an owner and a stable handle or identifier instead of another parallel collection on `AppShell`.

The directory structure is intentionally incremental. Large GPUI views may still
contain feature-local rendering code, but their transport contracts, state
registries, and reusable primitives have explicit boundaries for future splits.
