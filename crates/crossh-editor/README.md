# crossh-editor

Vendored input engine for `crossh-note`.

## Source

- Upstream: `longbridge/gpui-component` `crates/base` (Apache-2.0)
- Commit: `b2b7e41d5624114f7a5d0ba89ede2cc952aff315` (2026-02-05 main)
- License: Apache-2.0 Copyright 2024-2026 Longbridge (see `LICENSE-APACHE` in this crate and `THIRD_PARTY_NOTICES.md`)

## What is vendored

Full `crates/base` as of the pinned commit, kept intact for import stability:

- `src/input/base/*` — `InputBaseState`, `RopeExt`, `Selection`, `UndoManager`, `DisplayMap`, `Movement`, `TextElement`
- `src/input/textarea` / `src/input/input` / `src/input/editor/display_map` — `TextareaState` / `InputState`
- Supporting modules (`dock`, `motion`, `theme`, etc.) required by the crate's `lib.rs` (unused by `crossh-note` but kept to preserve upstream diffability)

`crossh-note` only uses:

```rust
use crossh_editor::input::{TextareaState, Textarea};
use crossh_editor::input::InputEvent;
```

Input is `Rope`+`UndoManager`+`DisplayMap`, giving `soft_wrap`, `history`, `IME/UTF16`, grapheme-correct movement.

## Why vendor vs depend

Direct `gpui-component` would force `gpui` rev alignment (`f66ed399` vs `1d217ee`, see `issue #2532`). Vendoring lets `crossh-editor` build against `f66ed399` and lets us re-skin `TextElement` via `crossh-theme` instead of `gpui-component`'s theme.

## Maintenance

- Do not edit `src/input/base/state.rs` (5338 lines) / `element.rs` (3245 lines) locally except for `crossh-theme` color shims; keep them diffable.
- The size whitelist in `scripts/check-architecture.sh` covers only the large
  upstream-derived files that are still kept intact.
- To update: `git clone --depth 1 https://github.com/longbridge/gpui-component && cp -R crates/base/* crates/crossh-editor/src/` then re-apply `Cargo.toml` rev patch (`scripts/fetch-crossh-editor.sh` TBD).
