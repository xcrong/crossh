# Terminal PTY Fixtures

These fixtures are minimized control-byte samples captured from real
applications. The checked-in files contain ASCII hexadecimal bytes only;
screen text, paths, host names, and environment values were removed before
commit.

## Compatibility

- File: `../terminal_compatibility.hex` (parent directory) — comprehensive
  replay covering alternate-screen, SGR mouse, modifyOtherKeys, etc.
  Used by `tests/terminal_replay.rs` to verify PTY read-boundary independence.

All fixture tests must replay both as one buffer and across one-byte or
arbitrary PTY-read boundaries where the protocol under test is incremental.
