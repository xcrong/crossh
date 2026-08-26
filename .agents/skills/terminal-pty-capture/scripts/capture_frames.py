#!/usr/bin/env python3
"""Capture a TUI's render frames through a PTY, one keystroke at a time.

Diagnoses per-frame rendering behaviour: full-screen repaints (ESC[2J) vs
incremental updates, flicker, dropped lines, dock/overlay coverage. The same
command line must be re-run before and after a fix; compare the repaint counts.

Frame splitting: the default frame separator is the synchronized-output intro
`ESC[?2026h` emitted by crossh's regular renderer. Applications that do not
use synchronized output can pass `--framesep` (e.g. an alternate-screen enter).

Example:
  python3 .agents/skills/terminal-pty-capture/scripts/capture_frames.py \
    --rows 30 --cols 80 --input "/model m" \
    --count 'ESC[2J' --name repaint -- target/debug/crossh

Escape sequences in --input: \\x1b, \\r, \\n, \\t are decoded.
"""

from __future__ import annotations

import argparse
import errno
import fcntl
import os
import pty
import select
import signal
import struct
import sys
import termios
import time


def decode_escapes(text: str) -> bytes:
    out = bytearray()
    i = 0
    while i < len(text):
        if text.startswith("\\x", i):
            out.append(int(text[i + 2 : i + 4], 16))
            i += 4
        elif text.startswith("\\r", i):
            out.append(13)
            i += 2
        elif text.startswith("\\n", i):
            out.append(10)
            i += 2
        elif text.startswith("\\t", i):
            out.append(9)
            i += 2
        else:
            out.extend(text[i].encode("utf-8"))
            i += 1
    return bytes(out)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Capture render frames from a TUI in a PTY, keystroke by keystroke."
    )
    parser.add_argument("--rows", type=int, default=30)
    parser.add_argument("--cols", type=int, default=80)
    parser.add_argument("--startup-delay", type=float, default=1.5)
    parser.add_argument("--step", type=float, default=0.15)
    parser.add_argument("--after-last", type=float, default=0.5)
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument(
        "--input",
        default="/",
        help="Text sent one character at a time (escape sequences decoded)",
    )
    parser.add_argument(
        "--framesep",
        default="\\x1b[?2026h",
        help="Byte sequence that starts each render frame (default: sync-output intro)",
    )
    parser.add_argument(
        "--count",
        action="append",
        default=[],
        help="Byte sequence counted per frame; repeatable (default: ESC[2J)",
    )
    parser.add_argument(
        "--name",
        action="append",
        default=[],
        help="Display name for each --count, in order",
    )
    parser.add_argument("--output", default="/tmp/capture-frames.bin")
    parser.add_argument("--hex-tail", type=int, default=1024)
    parser.add_argument(
        "command",
        nargs=argparse.REMAINDER,
        help="Command to run (e.g. target/debug/crossh)",
    )
    args = parser.parse_args()
    if args.command and args.command[0] == "--":
        args.command.pop(0)
    if not args.command:
        parser.error("a command is required")
    if not args.count:
        args.count = ["\\x1b[2J"]
        args.name = ["repaint"]
    if len(args.name) < len(args.count):
        args.name += [f"count{i}" for i in range(len(args.name), len(args.count))]
    counts = [decode_escapes(seq) for seq in args.count]
    if any(not seq for seq in counts):
        parser.error("count sequences must not be empty")
    args.framesep_bytes = decode_escapes(args.framesep)
    args.count_bytes = counts
    args.input_bytes = decode_escapes(args.input)
    return args


def main() -> int:
    args = parse_args()
    pid, master = pty.fork()
    if pid == 0:
        os.execvp(args.command[0], args.command)

    fcntl.ioctl(
        master,
        termios.TIOCSWINSZ,
        struct.pack("HHHH", args.rows, args.cols, 0, 0),
    )
    os.set_blocking(master, False)

    captured = bytearray()
    started_at = time.monotonic()
    first_input_at: float | None = None
    last_input_at: float | None = None

    def pump(seconds: float):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            ready, _, _ = select.select([master], [], [], 0.05)
            if master not in ready:
                continue
            try:
                chunk = os.read(master, 65536)
            except OSError as error:
                if error.errno in (errno.EIO, errno.EBADF):
                    return False
                raise
            if not chunk:
                return False
            captured.extend(chunk)
        return True

    try:
        if not pump(args.startup_delay):
            print("child closed during startup", file=sys.stderr)
            return 2
        for i, char in enumerate(args.input_bytes):
            try:
                os.write(master, bytes([char]))
            except OSError as error:
                if error.errno in (errno.EIO, errno.EBADF):
                    break
                raise
            if first_input_at is None:
                first_input_at = time.monotonic()
            last_input_at = time.monotonic()
            if not pump(args.step):
                break
        if last_input_at is not None:
            pump(args.after_last)
    finally:
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            os.waitpid(pid, 0)
        except ChildProcessError:
            pass
        try:
            os.close(master)
        except OSError:
            pass

    with open(args.output, "wb") as output:
        output.write(captured)

    frames = bytes(captured).split(args.framesep_bytes)
    print(f"capture={args.output}")
    print(f"command={' '.join(args.command)}")
    print(f"bytes={len(captured)}")
    print(f"frames={len(frames) - 1}")
    for i, seq in enumerate(args.count_bytes):
        print(
            f"{args.name[i]}_total={bytes(captured).count(seq)}"
            f" (sequence={seq!r})"
        )
    # Per input-character reporting: count occurrences up to each pump window.
    # Simpler and unambiguous: report per frame instead.
    for i, frame in enumerate(frames[1:], start=1):
        counts = ", ".join(
            f"{args.name[j]}={frame.count(seq)}" for j, seq in enumerate(args.count_bytes)
        )
        print(f"frame[{i}]: bytes={len(frame)} {counts}")

    tail = bytes(captured[-args.hex_tail :])
    print(f"tail_hex={tail.hex(' ')}")
    print(f"tail_repr={tail!r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())