# Terminal compatibility

This project keeps protocol coverage deterministic and treats full-screen
terminal applications as a separate interactive check. The local and SSH
relays both feed the same terminal emulator, so the checks should be run on
both paths when investigating a rendering or input issue.

## Automated checks

CI runs the following checks in the `terminal-compat` job on Ubuntu and
Windows:

```bash
cargo test --release terminal_replay::tests:: -- --test-threads=1
```

The replay uses `tests/fixtures/terminal_compatibility.hex`. It is a small,
reviewable list of hand-written ASCII hexadecimal bytes, not a captured user
session. It contains no host names, paths, credentials, or environment data.
The tests cover alternate-screen isolation, resize/reflow, styles, OSC 8,
OSC 52 policy, focus/bracketed-paste, mouse modes, Kitty keyboard mode, side
channel protocols, and one-byte input replay. Keep the fixture deterministic;
update the assertions in `src/terminal_replay.rs` in the same change when
adding a protocol case.

The Windows runner also executes the real ConPTY relay:

```powershell
cargo test --release windows_conpty_smoke_round_trip -- --nocapture
```

It starts the selected shell, verifies connection and input/output round-trip,
and checks clean child shutdown. The macOS `check` job runs the full release
test suite, including the replay and host-side drain/backoff tests. These jobs
do not install or run vttest or other full-screen applications.

To run the deterministic checks locally from the repository root, use the
same filters:

```bash
cargo test --release terminal_replay::tests:: -- --test-threads=1
cargo test --release local::windows::tests::output_ -- --test-threads=1
```

The second command exercises the platform-independent ConPTY drain-window and
polling-backoff state machine. Run the ConPTY command on a Windows runner.

## Implemented protocol surface

`alacritty_terminal` owns the standards-based VT/xterm emulator path: ANSI/CSI
cursor and screen control, SGR colors and styles, alternate screen, resize and
reflow, bracketed paste, focus reporting, OSC 8 hyperlinks, OSC 52 clipboard
policy, terminal titles, color queries, device-status replies, and Kitty
keyboard modes.

Crossh's side-channel parser adds the protocol pieces that need UI or relay
policy around that grid:

- OSC 7 working-directory tracking and OSC 133 prompt/command markers, with
  generated hooks for Bash, Zsh, Fish, and PowerShell local shells.
- Completion notifications from BEL, OSC 9, OSC 777, and Kitty OSC 99. Kitty
  notifications support chunked title/body data, stable-ID replacement,
  explicit close, `alive` queries, buttons, activation reports, focus actions,
  bounded expiry, and a settings toggle. Native system notifications are
  emitted according to the terminal-focus policy.
- tmux DCS passthrough, DECRQSS, XTGETTCAP, and urxvt CSI 1015 mouse reports.
- xterm `modifyOtherKeys` levels 0-3, including the query response and
  level-3 unmodified-key encoding.
- Inline iTerm images, Sixel, and Kitty graphics. Kitty payloads support
  chunking, PNG/raw RGB/RGBA formats, bounded zlib decompression, image and
  placement IDs (including image-number lookup), Unicode placeholders (`U=1`),
  relative placements (`P/Q/H/V`), source rectangles, pixel offsets, z-order,
  image deletion ranges, `C=1` cursor preservation, bounded
  normalization/cache sizes, and protocol acknowledgements. Physical images
  remain attached to terminal grid lines while virtual images are resolved
  from the placeholder cells in the grid.
- Windows Terminal OSC 9;4 progress state, rendered as a small status bar at
  the bottom of the terminal canvas.
- Kitty OSC 99 capability queries and xterm `CSI 14 t`, `CSI 16 t`,
  `CSI 18/19 t` terminal-size queries.

The parser is incremental and is fed before output is considered complete, so
OSC/DCS/APC sequences split across PTY reads are handled consistently for local
and SSH sessions.

## Deliberate gaps

This is broad modern TUI compatibility, not an implementation of every
terminal-vendor extension. The remaining notable gaps are:

- Kitty animation frames and animation controls (`a=f`, `a=a`, `a=c`) are not
  rendered yet. The file, temporary-file, and shared-memory transmission
  media are rejected with `ENOTSUP`: their names arrive through a potentially
  untrusted PTY and Crossh does not currently expose an explicit local-file
  access policy. The graphics query action returns protocol acknowledgements,
  but does not expose terminal-specific image metadata beyond that surface.
  Natural pixel-sized images still cannot derive an exact cursor footprint, so
  cursor advancement is only performed when an explicit cell rectangle is
  available.
- Kitty notification icons, sound selection, urgency, exact `invisible`
  visibility semantics, and reliable OS-level close callbacks remain limited
  by the platform notification API. Explicit close, activation, replacement,
  expiry, buttons, and `alive` are handled.
- SSH sessions probe the user's shell through a short-lived non-interactive
  channel. Bash, Zsh, and Fish sessions start with Crossh's prompt/cwd/command
  hooks without modifying the user's shell configuration; Zsh uses an
  ephemeral `.zshrc` that is removed when the session exits. Unknown shells,
  probe timeouts, and bootstrap failures fall back to a plain remote shell, so
  terminal access remains available even when command history cannot be
  collected. OSC 7/133 emitted by the remote shell or application and all
  BEL/OSC notifications continue to work over SSH.

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
