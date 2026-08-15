# Terminal compatibility

The standards boundary and current claim are tracked in
[`xterm-compatibility-profile.md`](xterm-compatibility-profile.md). This
document describes the existing replay and manual smoke workflows.

This project keeps protocol coverage deterministic and treats full-screen
terminal applications as a separate interactive check. The local and SSH
relays both feed the same terminal emulator, so the checks should be run on
both paths when investigating a rendering or input issue.

## Automated checks

CI runs the following checks in the `terminal replay / platform smoke` job on
Ubuntu and Windows:

```bash
cargo test --release --workspace --lib
cargo test --release --test terminal_replay -- --test-threads=1
```

The replay uses `tests/fixtures/terminal_compatibility.hex`. It is a small,
reviewable list of hand-written ASCII hexadecimal bytes, not a captured user
session. It contains no host names, paths, credentials, or environment data.
The tests cover alternate-screen isolation, resize/reflow, styles, OSC 8,
bracketed paste, focus reporting, mouse modes, and one-byte input replay.
Real-program minimized samples live in `tests/fixtures/terminal/`; the Vim
and tmux samples there contain control bytes only, with screen text and
environment data removed. Keep all fixtures deterministic and update the
assertions in the same change when adding a protocol case.

`tests/terminal_replay.rs` is also the home of the Windows-only ConPTY smoke:
under `cfg(windows)` it starts `cmd.exe` through the real ConPTY relay,
verifies input/output round-trip, and runs as part of the same
`--test terminal_replay` invocation on the Windows job. The macOS `check`
job runs the full release test suite, which includes the replay. These jobs
do not install or run vttest or other full-screen applications.

To run the deterministic checks locally from the repository root:

```bash
cargo test --release --test terminal_replay -- --test-threads=1
```

## Implemented protocol surface

The pinned Zed `terminal` crate owns the standards-based VT/xterm emulator
path. It delegates to its pinned `alacritty_terminal` and `vte` dependencies
for ANSI/CSI cursor and screen control, SGR colors and styles, alternate
screen, resize and reflow, bracketed paste, focus reporting, OSC 8
hyperlinks, terminal titles, standard mouse modes (including SGR 1006), and
device-status replies. Crossh does not duplicate that screen state. OSC 52
clipboard store/load is compiled off in the pinned Zed configuration
(`Osc52::Disabled`), so remote programs can neither write to nor read from
the user's clipboard through OSC 52.

Crossh adds the layer around that grid:

- Shell bootstrap hooks for Bash, Zsh, and Fish, used by both local
  terminals and SSH sessions. Temporary rc/env files (a `--rcfile` script for
  Bash, a `ZDOTDIR` directory with `.zshenv` for Zsh, an
  `XDG_DATA_DIRS`/`vendor_conf.d` file for Fish) inject prompt, command
  start/end, and cwd reporting without modifying the user's shell
  configuration. The marker payload rides the terminal title channel (`OSC 0`
  and `OSC 1337` prefixed `crossh-command=`/`crossh-command-status=` markers,
  base64-encoded) so Zed's normal title events carry it; the
  `crossh-core::terminal` title helpers decode it and the terminal view maps
  it to command-history, cwd, and tab-title events. PowerShell hooks are not
  implemented.
- SSH sessions probe the remote user's shell through a short-lived
  non-interactive channel (2 s timeout). Bash, Zsh, and Fish sessions start
  through a bootstrap command that builds the same hooks in a temporary
  directory and removes it when the session exits; unknown shells, probe
  timeouts, and bootstrap failures fall back to a plain remote shell, so
  terminal access remains available even when command history cannot be
  collected.
- Bell completion notifications, gated by a setting and a focus policy
  (notifications are shown only while the terminal is unfocused).
- OSC 8 hyperlink and URL navigation: hyperlink cells from the terminal core
  and hovered-word detection open through the platform opener.
- OSC 133 A/B/C/D-style markers are emitted by the generated hooks for tools
  that read them; Crossh's own parsing is the title-channel marker path
  described above.

There is no second incremental parser: output bytes are consumed by the
single Zed/alacritty emulator for both local and SSH sessions, and the
title-channel markers hold up across PTY-read boundaries because they travel
through the emulator's normal OSC-title handling.

## Deliberate gaps

This is a narrow profile built on the locked Zed terminal core, not an
implementation of every terminal-vendor extension. The following are not
implemented in the current codebase. Several of them *were* implemented by
the pre-Zed self-built renderer (protocol parser, input encoding, image
codecs, and view layers) and were removed when Crossh adopted the Zed
terminal core and its forked `TerminalElement` on 2026-08-08; the
`terminal_compatibility.hex` replay fixture survives from that era. Restoring
them is a deliberate non-goal: terminal work is now bug-fix only.

- Kitty graphics and animation (`APC`): no image transmission, chunking,
  Unicode placeholders, z-ordering, deletion, or acknowledgements. This also
  means no animation-frame controls (`a=f`, `a=a`, `a=c`), no file or
  temporary-file transmission (`t=f`, `t=t`), and no shared-memory transport.
- iTerm2 inline images (`OSC 1337`) and Sixel (`DCS`): not implemented. The
  `OSC 1337` prefix is only used internally by the shell hooks as a title
  marker channel.
- Kitty OSC 99 notifications and OSC 9 / OSC 777 completion notifications:
  not implemented. Only BEL triggers a native notification, without
  chunking, stable-ID replacement, explicit close, `alive` queries, buttons,
  activation reports, focus actions, expiry, or capability queries.
- Windows Terminal OSC 9;4 progress: not implemented.
- xterm `modifyOtherKeys` and the Kitty keyboard protocol: no input-encoding
  support beyond Zed's basic ESC-sequence encoding. The checked-in Vim
  fixture proves the emulator decodes `CSI > 4 ; 2 m` / `CSI > 4 ; m`, not
  that Crossh encodes those levels.
- tmux DCS passthrough, DECRQSS, XTGETTCAP, and urxvt CSI 1015 mouse
  reports: not implemented.
- OSC 52 clipboard: disabled by the pinned Zed configuration (no crossh
  policy layer).
- Terminal-size queries beyond the emulator's built-in replies: the earlier
  claim of `CSI 14/16/18/19 t` handling is not backed by code.
- Crossh-side parsing of external data files, remote paths, or shell config:
  remote paths and shell names are untrusted input and are only used to
  select between the Bash/Zsh/Fish bootstrap scripts or the plain-shell
  fallback.

## Manual vttest pass

`vttest` is intentionally an interactive tool, so it is not presented as a
fully automated CI gate. Install it through the platform package manager when
needed; no vttest dependency is added to this repository.

1. Open a local Crossh terminal and an SSH terminal with the same dimensions.
2. Run `vttest` separately in each terminal.
3. Follow the program's menu, use its `*` selection/continue entry, and follow
   the on-screen keyboard and mouse hints. Exercise cursor movement, colors,
   scrolling, alternate screen, resize, focus, and mouse reporting where the
   menu offers them.
4. Compare local and SSH behavior. If `vttest` is not installed, skip this
   pass and record that fact in the issue report.

## Suggested real TUI smoke

Run whichever applications are already available, separately in local and SSH
terminals. Missing applications are skipped rather than installed by CI.

- `tmux`: create a session, split panes and windows, scroll, and resize.
- `nvim`: open a file, move/search, edit, use colors, and resize the window.
- `htop` or `btop`: verify continuously updating rows, colors, and scrolling.
- `fzf`: type a query, move the selection, accept, and cancel.
- `less`: page, search, follow/quit, and resize.
- `lazygit`: navigate panes and dialogs, inspect colors, and resize.
- `yazi`: navigate directories, open/close previews, and resize.

These checks are deliberately manual: terminal applications vary by version,
configuration, and available data, making a scripted CI transcript brittle.

## Issue report details

Include the Crossh commit or version, OS/version, build mode, terminal size,
font or scale settings when relevant, and whether the failure is local, SSH,
or both. Record the remote OS and shell for SSH cases, the exact application
and command, minimal reproduction steps, expected versus actual behavior, and
whether a resize or focus change is involved. Attach a screenshot and safe
terminal output or escape-sequence sample when useful; redact paths, host
names, tokens, and other secrets. Note which tools were unavailable or which
manual checks were skipped.