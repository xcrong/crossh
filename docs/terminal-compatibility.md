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
OSC 52 policy, focus/bracketed-paste, mouse modes, Kitty keyboard mode, and
input chunk boundaries. Keep the fixture deterministic; update the assertions
in `src/terminal_replay.rs` in the same change when adding a protocol case.

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
