#!/usr/bin/env python3
"""Capture a terminal application's raw PTY output through its normal exit."""

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


CURSOR_POSITION_REQUEST = b"\x1b[6n"
CURSOR_POSITION_RESPONSE = b"\x1b[1;1R"
DEVICE_STATUS_REQUEST = b"\x1b[5n"
DEVICE_STATUS_RESPONSE = b"\x1b[0n"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Capture raw PTY output until a terminal application exits."
    )
    parser.add_argument(
        "--output",
        default="/tmp/terminal-pty-capture.bin",
        help="Raw PTY output path (default: /tmp/terminal-pty-capture.bin)",
    )
    parser.add_argument("--rows", type=int, default=40)
    parser.add_argument("--cols", type=int, default=120)
    parser.add_argument("--startup-delay", type=float, default=1.5)
    parser.add_argument("--timeout", type=float, default=20.0)
    parser.add_argument(
        "--exit-text",
        default="/exit",
        help="Text sent before carriage return (default: /exit)",
    )
    parser.add_argument(
        "command",
        nargs=argparse.REMAINDER,
        help="Command after -- (default: vtcode)",
    )
    args = parser.parse_args()
    if args.command and args.command[0] == "--":
        args.command.pop(0)
    if not args.command:
        args.command = ["vtcode"]
    if args.rows <= 0 or args.cols <= 0:
        parser.error("rows and cols must be positive")
    if args.startup_delay < 0 or args.timeout <= 0:
        parser.error("startup-delay must be non-negative and timeout must be positive")
    return args


def status_text(status: int) -> str:
    if os.WIFEXITED(status):
        return f"exit:{os.WEXITSTATUS(status)}"
    if os.WIFSIGNALED(status):
        return f"signal:{os.WTERMSIG(status)}"
    return f"status:{status}"


def main() -> int:
    args = parse_args()
    output_path = os.path.abspath(args.output)
    command = args.command
    exit_input = args.exit_text.encode() + b"\r"

    pid, master = pty.fork()
    if pid == 0:
        os.execvp(command[0], command)

    fcntl.ioctl(
        master,
        termios.TIOCSWINSZ,
        struct.pack("HHHH", args.rows, args.cols, 0, 0),
    )
    os.set_blocking(master, False)

    captured = bytearray()
    scan_overlap = b""
    cursor_queries = 0
    status_queries = 0
    exit_sent = False
    started_at = time.monotonic()
    exit_sent_at: float | None = None
    status: int | None = None
    child_reaped = False
    timed_out = False

    try:
        while time.monotonic() - started_at < args.timeout:
            ready, _, _ = select.select([master], [], [], 0.1)
            if master in ready:
                try:
                    chunk = os.read(master, 65536)
                except OSError as error:
                    if error.errno in (errno.EIO, errno.EBADF):
                        break
                    raise
                if not chunk:
                    break

                captured.extend(chunk)
                scan = scan_overlap + chunk
                cursor_count = scan.count(CURSOR_POSITION_REQUEST)
                status_count = scan.count(DEVICE_STATUS_REQUEST)
                for _ in range(cursor_count):
                    os.write(master, CURSOR_POSITION_RESPONSE)
                    cursor_queries += 1
                for _ in range(status_count):
                    os.write(master, DEVICE_STATUS_RESPONSE)
                    status_queries += 1
                scan_overlap = scan[-(len(CURSOR_POSITION_REQUEST) - 1) :]

            elapsed = time.monotonic() - started_at
            if not exit_sent and cursor_queries and elapsed >= args.startup_delay:
                os.write(master, exit_input)
                exit_sent = True
                exit_sent_at = time.monotonic()
            if exit_sent and exit_sent_at is not None and time.monotonic() - exit_sent_at > 5:
                timed_out = True
                break
    finally:
        try:
            waited_pid, status = os.waitpid(pid, os.WNOHANG)
            child_reaped = waited_pid == pid
        except ChildProcessError:
            status = 0
            child_reaped = True
        if not child_reaped:
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        try:
            os.close(master)
        except OSError:
            pass

    if not child_reaped:
        try:
            _, status = os.waitpid(pid, 0)
            child_reaped = True
        except ChildProcessError:
            status = 0
            child_reaped = True

    with open(output_path, "wb") as output:
        output.write(captured)

    tail = bytes(captured[-1024:])
    print(f"capture={output_path}")
    print(f"command={' '.join(command)}")
    print(f"bytes={len(captured)}")
    print(f"cursor_position_queries={cursor_queries}")
    print(f"device_status_queries={status_queries}")
    print(f"exit_sent={exit_sent}")
    print(f"wait_status={status_text(status or 0)}")
    print(f"tail_hex={tail.hex(' ')}")
    print(f"tail_repr={tail!r}")
    normal_exit = (
        child_reaped
        and not timed_out
        and status is not None
        and os.WIFEXITED(status)
        and os.WEXITSTATUS(status) == 0
    )
    return 0 if exit_sent and normal_exit else 1


if __name__ == "__main__":
    sys.exit(main())
