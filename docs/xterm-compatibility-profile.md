# Crossh xterm Compatibility Profile

This document defines the compatibility boundary for Crossh. It is a tested
profile, not a claim of complete ECMA-48, DEC VT, or xterm conformance.

## Sources

- [ECMA-48](https://www.ecma-international.org/publications-and-standards/standards/ecma-48/)
- [DEC VT510 ANSI control functions](https://vt100.net/docs/vt510-rm/chapter4.html)
- [xterm control sequences](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)
- [Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)

The screen emulator is the Zed `terminal` crate at the revision pinned in
`Cargo.toml` (`90d024b88abc91264d9a0ad260eb4f365fa695c3`). That crate uses
`alacritty_terminal` and `vte` internally. Crossh must not duplicate that
screen state.

## Coverage Matrix

| Area | Owner | Status | Crossh responsibility or evidence |
| --- | --- | --- | --- |
| C0 controls, UTF-8 text, line editing, cursor movement | Zed terminal | Zed | Standard screen behavior is delegated to Zed. |
| C1 7-bit/8-bit introducers and string terminators | Zed + Crossh observer | Partial | Zed consumes screen controls; Crossh observes OSC/DCS/APC and must safely handle C1 forms. |
| ESC screen controls and character-set controls | Zed terminal | Zed | Crossh observes only state-changing sequences needed for policy, such as RIS. |
| CSI public cursor, erase, insert/delete, scroll, tab, SGR | Zed terminal | Zed | No duplicate screen implementation. |
| CSI queries (`DA`, `DSR`, `CPR`, size reports) | Zed + Crossh | Partial | Zed handles built-in responses; Crossh handles queries not exposed by the public API and tests response bytes. |
| CSI private modes (alternate screen, cursor, mouse, paste, focus) | Zed terminal | Zed | Crossh observes alternate-screen transitions for image and keyboard side state. |
| DECSTR (`CSI ! p`) and RIS (`ESC c`) | Crossh observer + Zed | Partial | Crossh resets policy-owned state; Zed resets the screen emulator. |
| OSC title (`0`, `2`) | Zed + Crossh | Partial | Zed owns terminal title events; Crossh keeps the raw title for Crossh tab policy. |
| OSC 7 working directory | Crossh | Crossh | Parsed incrementally and validated as an absolute decoded path. |
| OSC 8 hyperlinks | Zed terminal | Zed | Zed owns hyperlink cells; Crossh separately detects plain-text URLs for navigation. |
| OSC 10/11/12 dynamic colors | Zed terminal | Zed | Zed handles color queries and color state; Crossh does not reimplement colors. |
| OSC 52 clipboard | Crossh policy + Zed screen | Crossh | Copy is allowed; remote clipboard reads are denied; payloads are bounded. |
| OSC 133 shell integration | Crossh | Crossh | Prompt, command start, command end, status, and cwd markers. |
| OSC 9 / 777 notifications and OSC 9;4 progress | Crossh | Crossh | Parsed into application events with bounded text and lifecycle state. |
| OSC 99 Kitty notifications | Crossh | Crossh | Chunking, lifecycle, bounded payloads, and response policy. |
| OSC 1337 iTerm images | Crossh | Crossh | Inline image payloads only; local file access is rejected. |
| DCS tmux passthrough | Crossh observer | Crossh | Unwraps bounded doubled-ESC payloads and reparses nested events. |
| DCS DECRQSS / XTGETTCAP | Crossh | Crossh | Responds to the supported capability subset; unknown queries receive negative replies. |
| DCS Sixel | Crossh | Crossh | Captures bounded Sixel payloads for the existing decoder. |
| APC Kitty graphics | Crossh | Crossh | Parses bounded base64 chunks; rendering policy lives in the terminal feature. |
| DCS/APC/OSC unknown extensions | Zed + Crossh observer | Partial | Safely ignored after bounded parsing; no vendor behavior is claimed. |
| PM and SOS strings | Crossh observer | Partial | Must be consumed and bounded without producing screen text; no semantic support is claimed. |
| Legacy, UTF-8, SGR, and urxvt mouse reports | Zed + Crossh input | Partial | Zed exposes standard modes; Crossh supplies the urxvt 1015 path and input reports. |
| SGR mouse (`1006`) and button/drag/motion modes | Zed terminal | Zed | Zed owns mode state; Crossh routes reports where the Crossh view needs them. |
| Alternate screen and bracketed paste | Zed terminal | Zed | Crossh only resets side state when applications leave the alternate screen. |
| Focus in/out reporting | Zed terminal | Zed | Zed emits the standard `CSI I`/`CSI O` bytes. |
| xterm `modifyOtherKeys` levels 0-3 | Crossh | Partial | Level parsing and input encoding exist; reset and query semantics are being hardened. |
| Kitty keyboard flags, query, push, and pop | Crossh | Partial | Input encoding exists; per-screen stacks and reset semantics are being hardened. |
| Keyboard input and application modes | Crossh input | Partial | Control, Meta, Kitty, modifyOtherKeys, keypad, mouse, and paste paths are tested independently. |
| Fuzz/property safety for arbitrary bytes | Crossh | Unimplemented | Add bounded parser fuzz/property tests before claiming parser hardening. |
| Real Vim/Neovim/tmux PTY captures | Crossh tests | Partial | Vim `CSI > 4;2m` / `CSI > 4;m` is covered; more minimized fixtures are required. |
| Kitty graphics animation, remote file/shared-memory transport | None | Out of scope | Requires media lifecycle and explicit local-file policy not present in Crossh. |
| Printer control, VT52 personality, protected fields, DEC font loading | None | Out of scope | Not needed by the supported shell/TUI profile. |

## Profile Claim

The supported claim is: Crossh delegates standard screen behavior to the
locked Zed terminal core and provides a bounded, incremental observer for the
Crossh policy surface listed above. Compatibility is supported only for rows
marked `Zed`, `Crossh`, or `Partial`, and only to the extent covered by tests
and fixtures. Rows marked `Unimplemented` or `Out of scope` are not claimed.

## Evidence Commands

```sh
cargo test --quiet
cargo test --quiet replay
scripts/check-architecture.sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```

Real PTY captures are stored as minimized, redacted fixtures under
`tests/fixtures/terminal/`. A fixture records bytes only; host names, paths,
credentials, and environment values must not be committed.
