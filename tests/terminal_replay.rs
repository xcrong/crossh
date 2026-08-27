use gpui::{AppContext, Context, IntoElement, Render, TestAppContext, Window, WindowOptions, div};
#[cfg(windows)]
use task::Shell;
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
            util::paths::PathStyle::local(),
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

#[gpui::test]
fn real_vim_and_tmux_mode_fixtures_replay_consistently(cx: &mut TestAppContext) {
    let vim = decode_fixture(include_str!("fixtures/terminal/vim_modify_other_keys.hex"));
    assert_eq!(replay(&vim, [vim.len()], cx), replay(&vim, [1], cx));

    let tmux = decode_fixture(include_str!("fixtures/terminal/tmux_pty.hex"));
    let snapshot = replay(&tmux, [1], cx);
    assert_eq!(snapshot, replay(&tmux, [2, 3, 1], cx));
    assert!(!snapshot.mode.contains(Modes::ALT_SCREEN));
    assert!(snapshot.mode.contains(Modes::SGR_MOUSE));
}

#[cfg(windows)]
#[gpui::test]
async fn windows_conpty_smoke_round_trip(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let builder = cx.update(|app| {
        TerminalBuilder::new(
            None,
            terminal::TerminalMode::interactive(),
            Shell::WithArguments {
                program: "cmd.exe".into(),
                args: vec!["/d".into(), "/c".into(), "echo crossh-conpty-smoke".into()],
                title_override: None,
            },
            Default::default(),
            CursorShape::default(),
            AlternateScroll::On,
            Some(64),
            Vec::new(),
            std::time::Duration::from_millis(0),
            false,
            0,
            app,
            Vec::new(),
            util::paths::PathStyle::local(),
        )
    });
    let builder = builder.await.expect("ConPTY terminal should start");
    let terminal = cx.new(|cx| builder.subscribe(cx));

    for _ in 0..100 {
        if terminal
            .read_with(cx, |terminal, _| terminal.get_content())
            .contains("crossh-conpty-smoke")
        {
            return;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(10))
            .await;
    }
    panic!("ConPTY output did not contain the smoke marker");
}
