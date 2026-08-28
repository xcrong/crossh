# Crossh xterm Compatibility Profile

This document defines the compatibility boundary for Crossh. It is a tested
profile, not a claim of complete ECMA-48, DEC VT, or xterm conformance. Rows
marked `Out of scope` are documented as not implemented in the current
codebase; several of them were implemented by the pre-Zed self-built renderer
and were removed when Crossh adopted the Zed terminal core (2026-08-08).

## Sources

- [ECMA-48](https://www.ecma-international.org/publications-and-standards/standards/ecma-48/)
- [DEC VT510 ANSI control functions](https://vt100.net/docs/vt510-rm/chapter4.html)
- [xterm control sequences](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)
- [Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)

The screen emulator is the Zed `terminal` crate at the revision pinned in
`Cargo.toml` (`f66ed399cdde86092af8af3dc7b418abf45f37f8`). That crate uses
`alacritty_terminal` and `vte` internally. Crossh must not duplicate that
screen state.

## Coverage Matrix

| Area | Owner | Status | Crossh responsibility or evidence |
| --- | --- | --- | --- |
| C0 controls, UTF-8 text, line editing, cursor movement | Zed terminal | Zed | Standard screen behavior is delegated to Zed. |
| C1 7-bit/8-bit introducers and string terminators | Zed terminal | Partial | Zed consumes screen controls; Crossh has no OSC/DCS/APC observer of its own. |
| ESC screen controls and character-set controls | Zed terminal | Zed | No duplicate screen implementation. |
| CSI public cursor, erase, insert/delete, scroll, tab, SGR | Zed terminal | Zed | No duplicate screen implementation. |
| CSI queries (`DA`, `DSR`, `CPR`, size reports) | Zed terminal | Partial | Zed handles public DA/DSR and size responses; Crossh adds no query handling. |
| CSI private modes (alternate screen, cursor, mouse, paste, focus) | Zed terminal | Zed | Zed owns mode state; Crossh reads the alternate-screen flag only to decide scroll behavior and to switch terminals. |
| DECSTR (`CSI ! p`) and RIS (`ESC c`) | Zed terminal | Zed | Zed resets the screen emulator; Crossh has no reset observer. |
| OSC title (`0`, `2`) | Zed + Crossh | Partial | Zed owns terminal title events; Crossh decodes `crossh-command=` / `crossh-command-status=` markers that ride the title channel and derives tab titles from the raw title. |
| OSC 7 working directory | Crossh hooks + title channel | Crossh | Generated hooks emit OSC 7 and title markers; Crossh decodes the title marker (`crates/crossh-core/src/terminal/title.rs`) rather than parsing OSC 7 itself. |
| OSC 8 hyperlinks | Zed terminal | Zed | Zed owns hyperlink cells; Crossh opens URL navigation targets through the platform opener. |
| OSC 10/11/12 dynamic colors | Zed terminal | Zed | Zed handles color queries and color state; Crossh does not reimplement colors. |
| OSC 52 clipboard | None | Out of scope | The pinned Zed configuration compiles OSC 52 off (`Osc52::Disabled`); there is no Crossh policy layer for copy-out or paste-in. |
| OSC 133 shell integration | Crossh hooks + title channel | Crossh | Bash/Zsh/Fish hooks report prompt, command start/end, status, and cwd; event decoding happens in the terminal view via `crossh-core::terminal` markers. PowerShell is not covered. |
| OSC 9 / 777 notifications and OSC 9;4 progress | None | Out of scope | Not implemented; only BEL triggers a native notification. |
| OSC 99 Kitty notifications | None | Out of scope | Not implemented (no chunking, lifecycle, close, `alive`, buttons, activation, expiry, or capability queries). |
| OSC 1337 iTerm images | None | Out of scope | Not implemented; the `OSC 1337` prefix is used only as an internal title-marker channel by the shell hooks. |
| DCS tmux passthrough | None | Out of scope | Not implemented. |
| DCS DECRQSS / XTGETTCAP | None | Out of scope | Not implemented. |
| DCS Sixel | None | Out of scope | Not implemented. |
| APC Kitty graphics | None | Out of scope | Not implemented (no transmission, chunking, placeholders, placement, z-order, deletion, or acknowledgements). |
| DCS/APC/OSC unknown extensions | Zed terminal | Partial | Safely consumed or ignored by the core; no vendor behavior is claimed. |
| PM and SOS strings | Zed terminal | Partial | Processed without producing screen text by the core; no Crossh handling. |
| Legacy, UTF-8, SGR, and urxvt mouse reports | Zed terminal | Partial | Zed encodes standard and SGR (1006) reports; urxvt CSI 1015 is not implemented. |
| SGR mouse (`1006`) and button/drag/motion modes | Zed terminal | Zed | Zed owns mode state and report encoding. |
| Alternate screen and bracketed paste | Zed terminal | Zed | Crossh only uses the alternate-screen flag for scroll and terminal-switch behavior. |
| Focus in/out reporting | Zed terminal | Zed | Zed emits the standard `CSI I`/`CSI O` bytes under mode 1004. |
| xterm `modifyOtherKeys` levels 0-3 | None | Out of scope | No input encoding; the Vim fixture only proves the emulator decodes `CSI > 4 ; 2 m` / `CSI > 4 ; m`. |
| Kitty keyboard protocol | None | Out of scope | No progressive encoding, stacks, or query responses. |
| Keyboard input and application modes | Zed terminal | Partial | Zed provides basic ESC-sequence encoding (cursor, function, modified keys); no Kitty/modifyOtherKeys paths. |
| Fuzz/property safety for arbitrary bytes | None | Out of scope | Replay tests cover fixtures and one-byte chunk boundaries only; no fuzz/property harness exists. |
| Real Vim/Neovim/tmux PTY captures | Crossh tests | Partial | Vim (`vim_modify_other_keys.hex`) and tmux (`tmux_pty.hex`) samples are covered; no Neovim capture is included, so no Neovim behavior is claimed. |
| Kitty graphics animation, remote file/shared-memory transport | None | Out of scope | Requires a graphics implementation and a local-file access policy that do not exist. |
| Printer control, VT52 personality, protected fields, DEC font loading | None | Out of scope | Not needed by the supported shell/TUI profile. |

## Profile Claim

The supported claim is: Crossh delegates standard screen behavior to the
locked Zed terminal core and adds shell hooks plus title-channel decoding for
prompt/command/cwd markers, BEL-based notifications, and URL navigation.
Compatibility is supported only for rows marked `Zed`, `Crossh`, or `Partial`,
and only to the extent covered by tests and fixtures. Rows marked
`Out of scope` are not claimed; `Partial` rows are limited to the exact cases
named in their evidence.

## Evidence Commands

```sh
cargo fmt --check
scripts/check-architecture.sh
cargo clippy --all-targets -- -D warnings
cargo test --workspace
cargo test --test terminal_replay -- --test-threads=1
```

> 此处为最小证据集，不替代全量 `cargo test --workspace`。

Real PTY captures are stored as minimized, redacted fixtures under
`tests/fixtures/terminal/`. A fixture records bytes only; host names, paths,
credentials, and environment values must not be committed.