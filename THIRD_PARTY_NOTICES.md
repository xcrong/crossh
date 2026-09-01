# Third-Party Notices

## Crossh license

Crossh is distributed under the GNU General Public License, version 3 or
later (`GPL-3.0-or-later`). Interactive terminals use Zed's `assets`,
`terminal`, `task`, settings, and theme infrastructure directly. Crossh also
maintains a local, application-free fork of the required Zed `terminal_view`
`TerminalElement` and APCA helper sources. The fork is derived from the Zed
revision pinned in `Cargo.toml`; editor, LSP, workspace, search, and other Zed
Crossh adds its workspace, command-management, and Git/Note layers around
that foundation.

## Lucide

Crossh embeds SVG icons downloaded from the official Lucide release `1.27.0`:

- Source: https://github.com/lucide-icons/lucide/tree/1.27.0/icons
- Release commit: `4aec3f892fd6c23063bc2fead83c899b5d412b1c`
- License: ISC
- Embedded by the UI-neutral `crossh-assets` crate from
  `crates/crossh-assets/assets/icons/`; `crossh-ui` only adapts those bytes to
  GPUI.

```text
ISC License

Copyright (c) 2026 Lucide Icons and Contributors

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

Lucide identifies some icons as derived from Feather. Those icons retain the
Feather MIT notice documented in Lucide's official `LICENSE` file.

## Core dependencies

- **gpui / gpui_platform** — UI toolkit and platform integration, direct git
  dependencies from the Zed project at the pinned revision in `Cargo.toml`.
  APACHE-2.0 / GPL-3.0-or-later (source: zed-industries/zed).
- **terminal** — Zed's terminal emulator and PTY integration at the same pinned
  revision. It owns process I/O, emulation, resize, and terminal scrollback.
  GPL-3.0-or-later.
- **terminal_view source fork** — Crossh's
  `src/features/terminal/zed_view/terminal_element.rs` is derived from Zed's
  `crates/terminal_view/src/terminal_element.rs`; its local APCA helper is
  derived from Zed's `crates/ui/src/utils/apca_contrast.rs`. Both are
  GPL-3.0-or-later and retain the pinned revision in their source headers. The fork keeps
  GPUI painting, keyboard/mouse input, selection, IME, and terminal scrolling,
  while removing Zed editor/workspace application integrations.
- **crossh-editor input engine** — Vendored from `longbridge/gpui-component`
  `crates/base` (commit `b2b7e41d5624114f7a5d0ba89ede2cc952aff315`, 2026-02-05) at
  `crates/crossh-editor/`. Retains `Apache-2.0` Copyright 2024-2026 Longbridge.
  Crossh vendors the shared editing engine (`input/base/*`, `textarea`, `input`, `editor/display_map`)
  to obtain `Rope`+`UndoManager`+`DisplayMap`+`TextElement` with `soft_wrap`, `history`, `IME/UTF16`,
  `grapheme` movement and `selection` without pulling `gpui-component`'s styled façade or theme.
  Rendering is re-skinned via `crossh-theme` (see `crates/crossh-editor/README.md` for the copied file list and rev pin).
  Complete `gpui-component` theme/style layers are intentionally not used; the vendored code is tracked as third-party
  `Apache-2.0` source and remains diffable against upstream.
- **assets** — Zed's embedded resource source, including the bundled Lilex
  and IBM Plex fonts loaded by Crossh's GPUI text system. GPL-3.0-or-later.
- **settings / task / theme / theme_settings / release_channel / util** — Zed
  infrastructure used to initialize the terminal core, apply terminal settings,
  create shell processes, and integrate its theme.
  Licensing follows the Zed source at the pinned revision in `Cargo.toml`.
- **alacritty_terminal / vte** — transitive dependencies of Zed's `terminal`
  emulator. Crossh no longer owns a second production renderer or direct
  protocol implementation using these crates. Apache-2.0 / MIT.
## Application icon

`assets/appicon/AppIcon.icns` is hand-drawn for Crossh (a mint crosshair on the
app's graphite background) and is not a Lucide asset. Its source is
`assets/appicon/icon-master.svg`; regenerate the iconset with
`iconutil -c icns` after editing the master.
