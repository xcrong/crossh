# Terminal PTY Fixtures

These fixtures are minimized control-byte samples captured from real
applications. The checked-in files contain ASCII hexadecimal bytes only;
screen text, paths, host names, and environment values were removed before
commit.

## Vim

- Source: `/usr/bin/vim` on macOS, started through `/usr/bin/script` with
  `TERM=xterm-256color`.
- Retained bytes: startup `CSI > 4 ; 2 m` and exit `CSI > 4 ; m`.
- File: `vim_modify_other_keys.hex`.

## tmux

- Source: `/opt/homebrew/bin/tmux` attached to a local PTY with
  `TERM=xterm-256color`.
- Retained bytes: tmux's application alternate-screen and SGR mouse mode
  transitions.
- File: `tmux_pty.hex`.

## Neovim

No Neovim fixture is included in this change, so no Neovim behavior is
claimed. Add a minimized PTY capture and a regression test when a reproducible
Neovim capture environment is available; do not synthesize one from Vim bytes.

All fixture tests must replay both as one buffer and across one-byte or
arbitrary PTY-read boundaries where the protocol under test is incremental.
