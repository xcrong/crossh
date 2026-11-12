use gpui::{AppContext, Context, IntoElement, Render, TestAppContext, Window, WindowOptions, div};
use terminal::terminal_settings::{AlternateScroll, CursorShape};
use terminal::{Modes, Terminal, TerminalBuilder};

struct TestRoot;

impl Render for TestRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[derive(Debug, PartialEq, Eq)]
struct TerminalSnapshot {
    content: String,
    mode: Modes,
}

fn decode_fixture(source: &str) -> Vec<u8> {
    source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .flat_map(|line| {
            assert_eq!(line.len() % 2, 0, "fixture line must contain byte pairs");
            (0..line.len()).step_by(2).map(move |index| {
                u8::from_str_radix(&line[index..index + 2], 16)
                    .expect("fixture must contain ASCII hexadecimal bytes")
            })
        })
        .collect()
}

fn display_terminal(cx: &mut TestAppContext) -> gpui::Entity<Terminal> {
    cx.new(|cx| {
        TerminalBuilder::new_display_only(
            CursorShape::default(),
            AlternateScroll::On,
            Some(64),
            0,
            cx.background_executor(),
            terminal::paths::PathStyle::local(),
        )
        .subscribe(cx)
    })
}

fn replay(
    bytes: &[u8],
    chunk_sizes: impl IntoIterator<Item = usize>,
    cx: &mut TestAppContext,
) -> TerminalSnapshot {
    let terminal = display_terminal(cx);
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| TestRoot))
            .expect("test window")
    });

    let sizes = chunk_sizes.into_iter().collect::<Vec<_>>();
    assert!(!sizes.is_empty(), "at least one chunk size is required");
    let mut offset = 0;
    let mut size_index = 0;
    while offset < bytes.len() {
        let size = sizes[size_index % sizes.len()].max(1);
        let end = (offset + size).min(bytes.len());
        terminal.update(cx, |terminal, cx| {
            terminal.write_output(&bytes[offset..end], cx)
        });
        offset = end;
        size_index += 1;
    }
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| {
            terminal.update(cx, |terminal, cx| terminal.sync(window, cx));
        })
        .expect("test window remains open");

    terminal.read_with(cx, |terminal, _| TerminalSnapshot {
        content: terminal.get_content(),
        mode: terminal.last_content().mode,
    })
}

#[gpui::test]
fn compatibility_fixture_is_independent_of_pty_read_boundaries(cx: &mut TestAppContext) {
    let fixture = decode_fixture(include_str!("fixtures/terminal_compatibility.hex"));
    let one_buffer = replay(&fixture, [fixture.len()], cx);
    let one_byte = replay(&fixture, [1], cx);
    let varied = replay(&fixture, [1, 2, 3, 5, 8, 13], cx);

    assert_eq!(one_buffer, one_byte);
    assert_eq!(one_buffer, varied);
    assert!(one_buffer.content.contains("PRIMARY STYLELINK"));
    assert!(!one_buffer.mode.contains(Modes::ALT_SCREEN));
    assert!(one_buffer.mode.contains(Modes::FOCUS_IN_OUT));
    assert!(one_buffer.mode.contains(Modes::BRACKETED_PASTE));
    assert!(one_buffer.mode.contains(Modes::MOUSE_DRAG));
    assert!(one_buffer.mode.contains(Modes::SGR_MOUSE));
}

fn remote_terminal(cx: &mut TestAppContext) -> gpui::Entity<Terminal> {
    let (tx, _rx) = async_channel::unbounded::<Vec<u8>>();
    cx.new(|cx| {
        TerminalBuilder::new_remote_with_bounds(
            CursorShape::default(),
            AlternateScroll::On,
            Some(64),
            0,
            cx.background_executor(),
            terminal::paths::PathStyle::local(),
            terminal::TerminalBounds::default(),
            tx,
        )
        .subscribe(cx)
    })
}

fn replay_remote(
    bytes: &[u8],
    chunk_sizes: impl IntoIterator<Item = usize>,
    cx: &mut TestAppContext,
) -> TerminalSnapshot {
    let terminal = remote_terminal(cx);
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| TestRoot))
            .expect("test window")
    });

    let sizes = chunk_sizes.into_iter().collect::<Vec<_>>();
    assert!(!sizes.is_empty(), "at least one chunk size is required");
    let mut offset = 0;
    let mut size_index = 0;
    while offset < bytes.len() {
        let size = sizes[size_index % sizes.len()].max(1);
        let end = (offset + size).min(bytes.len());
        terminal.update(cx, |terminal, cx| {
            terminal.write_remote_output(&bytes[offset..end], cx)
        });
        offset = end;
        size_index += 1;
    }
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| {
            terminal.update(cx, |terminal, cx| terminal.sync(window, cx));
        })
        .expect("test window remains open");

    terminal.read_with(cx, |terminal, _| TerminalSnapshot {
        content: terminal.get_content(),
        mode: terminal.last_content().mode,
    })
}

/// 对拍：PTY 保真字节（已含 `\\r\\n`）经两条入向路径进 VT，画面与 Modes 一致。
#[gpui::test]
fn remote_replay_matches_display_only_for_pty_faithful_bytes(cx: &mut TestAppContext) {
    let fixture = decode_fixture(include_str!("fixtures/terminal_compatibility.hex"));
    let display = replay(&fixture, [1024], cx);
    let remote = replay_remote(&fixture, [1024], cx);
    assert_eq!(display, remote);
    // 远程路径与读边界无关（直达 advance，无逐次重置的转换状态）。
    assert_eq!(remote, replay_remote(&fixture, [1], cx));
}

/// raw 模式裸 `\\n`：远程路径不得补 `\\r`（与 display 路径的行为差即证明）。
/// 若有人把 `write_remote_output` 简化成调 `write_output`，此用例变绿为红。
#[gpui::test]
fn remote_preserves_bare_lf_semantics(cx: &mut TestAppContext) {
    let bytes = b"ab\ncd";
    let display = replay(bytes, [bytes.len()], cx);
    let remote = replay_remote(bytes, [bytes.len()], cx);
    assert_ne!(
        display.content, remote.content,
        "remote must not inject CR for bare LF"
    );
    assert!(remote.content.contains("ab") && remote.content.contains("cd"));
}
