---
name: terminal-pty-capture
description: Use when diagnosing terminal or TUI exit, cleanup, or control-sequence bugs, especially when CSI 6n, alternate-screen, mouse, focus, bracketed-paste, or Kitty keyboard sequences are involved and pipe or script capture is misleading. Also use to reproduce and verify TUI rendering-frame behaviour (full-screen repaint vs incremental updates, flicker, dropped or covered lines) with keystroke-by-keystroke PTY injection and per-frame sequence counts.
---

# Terminal PTY Capture

## Core Rule

Use a real PTY. A pipe has no terminal semantics, and macOS `script` records a PTY without emulating terminal replies. Both can create false startup or exit failures when an application asks for cursor position.

## Quick Start

From the repository root:

```bash
python3 .agents/skills/terminal-pty-capture/scripts/capture_terminal_exit.py \
  --output /tmp/vtcode-exit.bin -- vtcode
```

The default input is `/exit` plus carriage return. The file contains only bytes read from the child PTY; harness writes are not mixed into it.

For per-frame rendering behaviour (repaint counts, flicker), use `scripts/capture_frames.py` — see [Rendering-Frame Diagnosis](#rendering-frame-diagnosis) below.

## Workflow

1. Spawn the exact command in a PTY and set a realistic window size.
2. Answer terminal queries. At minimum, reply to `ESC[6n` with `ESC[1;1R`; reply to `ESC[5n` with `ESC[0n` when encountered.
3. Use the normal exit path. Direct `vtcode` exits with `/exit` and Enter. Double `Ctrl-C` tests interruption, not normal exit.
4. Keep binary output. Inspect it with `xxd -g 1`, `od -An -t x1c`, or a byte parser. Do not decode or normalize it first.
5. Separate child output, terminal replies, and harness input.

## Observed VT Code 0.142.1

Setup includes color queries, `CSI c`, bracketed paste, mouse/focus modes, `ESC 7`, `CSI >7u`, title `crossh`, cursor hide, and `CSI 6n`.

The observed normal `/exit` cleanup tail is:

```text
CSI 1G, CSI 2K
CSI ?1049l, CSI ?2004l, CSI ?1004l, CSI ?1006l, CSI ?1015l
CSI ?1003l, CSI ?1002l, CSI ?1000l, CSI <1u
OSC 22;default BEL, CSI 0 q, CSI ?25h, ESC 8, OSC 0; BEL
```

VT Code may issue another `CSI 6n` while clearing its inline terminal. If the emulator does not answer it, the error `The cursor position could not be read within a normal duration` can appear and the apparent exit tail is incomplete.

## Rendering-Frame Diagnosis

Use `scripts/capture_frames.py` when a bug is about *what the screen does over time* (flicker, full-screen repaint on every keystroke, dropped streaming characters, dock/overlay covering content) rather than what the app writes on exit.

```bash
python3 .agents/skills/terminal-pty-capture/scripts/capture_frames.py \
  --rows 30 --cols 80 --input "/model m" \
  --count '\x1b[2J' --name repaint -- target/debug/crossh-agent
```

Workflow:

1. Inject the input **one character at a time** (`--step` between characters). Pasting the whole string at once cannot pinpoint which keystroke triggers a repaint.
2. Split the byte stream on a frame separator. Default `--framesep` is `\x1b[?2026h` (synchronized-output intro) — each frame matches one keystroke's render. Use `--framesep` for apps without sync output.
3. Count a marker sequence per frame with `--count` (repeatable). `\x1b[2J` counts full-screen repaints; choose other markers per symptom (e.g. `\x1b[2K` for per-line erases, `\x1bM` for reverse index).
4. Assert before/after: run the **same command line** against the old and fixed build and compare `*_total` and per-frame counts. The fix is proven only by a drop in the counted sequence, not by eyeballing the screen.
5. Keep the raw capture (`--output`, default `/tmp/capture-frames.bin`) for hex inspection of specific lines (footer, editor, popup).

Observed with crossh-agent regular mode: every render frame is wrapped in `ESC[?2026h` … `ESC[?2026l`; a full-screen repaint is `ESC[2J ESC[H`; dock lines are erased with `ESC[2K`.

## Crossh Diagnosis

The child sends `CSI 6n` through the PTY; the emulator must send `CSI row;column R` back through that same PTY. Treat a cursor-position timeout as a terminal response-path bug before treating it as an application cleanup bug. Also verify `CSI ? ... h/l`, `CSI > ... u`, `CSI < ... u`, OSC title updates, cursor visibility, focus, mouse, bracketed paste, and Kitty keyboard restoration.

## Common Mistakes

| Mistake | Correction |
| --- | --- |
| Pipe stdout or use `script` alone | Use a PTY harness that answers DSR queries |
| Quit with `Ctrl-C` | Send `/exit` plus carriage return for normal exit |
| Decode or trim the stream | Preserve bytes and inspect hex |
| Mix harness replies into the log | Log only reads from the PTY master |
| Paste the whole input at once | Send one character at a time so each repaint trigger is isolated (capture_frames.py `--input`) |
| Watch the screen for flicker | Count `ESC[2J` occurrences per frame in the byte stream (`--count`) |
| Report a single capture as proof | Run the same capture against old and fixed builds and compare totals |

Use `scripts/capture_terminal_exit.py` for repeatable captures. Adjust `--timeout`, `--startup-delay`, `--rows`, and `--cols` before changing the harness.
