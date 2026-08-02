# Third-Party Notices

## Lucide

Crossh embeds SVG icons downloaded from the official Lucide release `1.27.0`:

- Source: https://github.com/lucide-icons/lucide/tree/1.27.0/icons
- Release commit: `4aec3f892fd6c23063bc2fead83c899b5d412b1c`
- License: ISC

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

- **gpui** — UI toolkit, vendored from the Zed project at a pinned commit
  (see `Cargo.toml`). APACHE-2.0 / GPL-3.0-or-later (source: zed-industries/zed).
- **russh / russh-sftp** — SSH client and SFTP protocol. MIT.
- **alacritty_terminal / vte** — terminal emulator core and escape sequence
  parsing. Apache-2.0 / MIT.

## Application icon

`assets/appicon/AppIcon.icns` is hand-drawn for Crossh (a mint crosshair on the
app's graphite background) and is not a Lucide asset. Its source is
`assets/appicon/icon-master.svg`; regenerate the iconset with
`iconutil -c icns` after editing the master.
