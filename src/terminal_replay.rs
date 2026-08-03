//! Deterministic terminal protocol replay tests.
//!
//! The fixture is deliberately ASCII hex instead of a captured terminal log:
//! it keeps the test small, reviewable, and free of user/session data while
//! still exercising the exact byte boundaries that the local/SSH relays use.

use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Osc52, Term, TermMode};
use alacritty_terminal::vte::ansi::NamedColor;
use vte::ansi::Processor;

#[derive(Clone, Debug, PartialEq, Eq)]
enum ReplayEvent {
    ClipboardStore(String),
    ClipboardLoad,
    PtyWrite(Vec<u8>),
}

#[derive(Clone, Default)]
struct ReplayListener {
    events: Arc<Mutex<Vec<ReplayEvent>>>,
}

impl ReplayListener {
    fn events(&self) -> Vec<ReplayEvent> {
        self.events.lock().expect("replay event lock").clone()
    }
}

impl EventListener for ReplayListener {
    fn send_event(&self, event: Event) {
        let event = match event {
            Event::ClipboardStore(_, text) => ReplayEvent::ClipboardStore(text),
            Event::ClipboardLoad(_, _) => ReplayEvent::ClipboardLoad,
            Event::PtyWrite(text) => ReplayEvent::PtyWrite(text.into_bytes()),
            _ => return,
        };
        self.events.lock().expect("replay event lock").push(event);
    }
}

#[derive(Clone, Copy)]
struct ReplaySize {
    cols: usize,
    rows: usize,
}

impl Dimensions for ReplaySize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

struct Replay {
    term: Term<ReplayListener>,
    parser: Processor,
    size: ReplaySize,
    listener: ReplayListener,
}

impl Replay {
    fn new(cols: usize, rows: usize, osc52: Osc52) -> Self {
        let listener = ReplayListener::default();
        let size = ReplaySize { cols, rows };
        let config = Config {
            scrolling_history: 64,
            kitty_keyboard: true,
            osc52,
            ..Default::default()
        };
        Self {
            term: Term::new(config, &size, listener.clone()),
            parser: Processor::new(),
            size,
            listener,
        }
    }

    fn feed_one_byte_chunks(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(1) {
            self.parser.advance(&mut self.term, chunk);
        }
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        self.size = ReplaySize { cols, rows };
        self.term.resize(self.size);
    }

    fn line(&self, line: usize) -> String {
        self.coordinate_line(Line(line as i32))
    }

    fn coordinate_line(&self, line: Line) -> String {
        let grid = self.term.grid();
        (0..grid.columns())
            .map(|column| grid[line][Column(column)].c)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    fn lines(&self) -> Vec<String> {
        (0..self.term.grid().screen_lines())
            .map(|line| self.line(line))
            .collect()
    }

    fn cell(&self, line: usize, column: usize) -> &Cell {
        &self.term.grid()[Line(line as i32)][Column(column)]
    }
}

fn decode_fixture() -> Vec<u8> {
    include_str!("../tests/fixtures/terminal_compatibility.hex")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .flat_map(|line| {
            assert!(line.len() % 2 == 0, "fixture line must contain byte pairs");
            (0..line.len()).step_by(2).map(move |index| {
                u8::from_str_radix(&line[index..index + 2], 16)
                    .expect("fixture must contain ASCII hexadecimal bytes")
            })
        })
        .collect()
}

fn fixture_slice(bytes: &[u8], marker: &[u8]) -> (usize, usize) {
    let start = bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("fixture marker");
    (start, start + marker.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALT_ON: &[u8] = b"\x1b[?1049h";
    const ALT_OFF: &[u8] = b"\x1b[?1049l";

    #[test]
    fn fixture_replay_has_stable_protocol_and_screen_golden() {
        let bytes = decode_fixture();
        let (alt_start, _alt_end) = fixture_slice(&bytes, ALT_ON);
        let (alt_off_start, alt_off_end) = fixture_slice(&bytes, ALT_OFF);

        let mut replay = Replay::new(80, 4, Osc52::OnlyCopy);
        replay.feed_one_byte_chunks(&bytes[..alt_start]);
        assert!(!replay.term.mode().contains(TermMode::ALT_SCREEN));
        assert_eq!(replay.lines()[0], "PRIMARY");

        replay.feed_one_byte_chunks(&bytes[alt_start..alt_off_end]);
        assert!(!replay.term.mode().contains(TermMode::ALT_SCREEN));
        assert_eq!(replay.lines()[0], "PRIMARY");
        replay.feed_one_byte_chunks(&bytes[alt_off_end..]);

        // The fixture's alternate-screen payload is isolated from the primary
        // buffer; replaying exactly the middle segment proves the transition.
        let mut alternate = Replay::new(80, 4, Osc52::OnlyCopy);
        alternate.feed_one_byte_chunks(&bytes[alt_start..alt_off_start]);
        assert!(alternate.term.mode().contains(TermMode::ALT_SCREEN));
        assert_eq!(alternate.lines()[0], "ALT-SCREEN");
        assert_eq!(alternate.cell(0, 0).c, 'A');

        let styled = replay.cell(0, 8);
        assert_eq!(styled.c, 'S');
        assert_eq!(
            styled.fg,
            alacritty_terminal::vte::ansi::Color::Named(NamedColor::Red)
        );
        assert!(styled.flags.contains(Flags::BOLD | Flags::UNDERLINE));

        let link_column = 13;
        assert_eq!(replay.line(0), "PRIMARY STYLELINK");
        assert_eq!(replay.cell(0, link_column).c, 'L');
        assert_eq!(
            replay
                .cell(0, link_column)
                .hyperlink()
                .map(|link| link.uri().to_string()),
            Some("https://example.com".to_string())
        );

        let mode = *replay.term.mode();
        assert!(mode.contains(TermMode::FOCUS_IN_OUT));
        assert!(mode.contains(TermMode::BRACKETED_PASTE));
        assert!(mode.contains(TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE));
        assert!(mode.contains(TermMode::KITTY_KEYBOARD_PROTOCOL));

        let clipboard_stores = replay
            .listener
            .events()
            .into_iter()
            .filter_map(|event| match event {
                ReplayEvent::ClipboardStore(text) => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(clipboard_stores, vec!["Hello"]);
    }

    #[test]
    fn fixture_osc52_load_is_policy_gated() {
        let bytes = decode_fixture();
        let mut copy_only = Replay::new(80, 4, Osc52::OnlyCopy);
        copy_only.feed_one_byte_chunks(&bytes);
        assert!(
            !copy_only
                .listener
                .events()
                .contains(&ReplayEvent::ClipboardLoad)
        );

        let mut copy_paste = Replay::new(80, 4, Osc52::CopyPaste);
        copy_paste.feed_one_byte_chunks(b"\x1b]52;c;?\x07");
        assert!(
            copy_paste
                .listener
                .events()
                .contains(&ReplayEvent::ClipboardLoad)
        );
    }

    #[test]
    fn replay_resize_reflows_wrapped_text_deterministically() {
        let mut replay = Replay::new(4, 2, Osc52::OnlyCopy);
        replay.feed_one_byte_chunks(b"abcdefg");
        assert_eq!(replay.lines(), vec!["abcd", "efg"]);

        replay.resize(8, 2);
        assert_eq!(replay.lines(), vec!["abcdefg", ""]);

        replay.resize(4, 2);
        assert_eq!(replay.lines(), vec!["efg", ""]);
        assert_eq!(replay.coordinate_line(Line(-1)), "abcd");
    }

    #[test]
    fn fixture_chunk_boundaries_do_not_change_golden_output() {
        let bytes = decode_fixture();
        let mut one_chunk = Replay::new(80, 4, Osc52::OnlyCopy);
        one_chunk.parser.advance(&mut one_chunk.term, &bytes);

        let mut byte_chunks = Replay::new(80, 4, Osc52::OnlyCopy);
        byte_chunks.feed_one_byte_chunks(&bytes);

        assert_eq!(one_chunk.lines(), byte_chunks.lines());
        assert_eq!(*one_chunk.term.mode(), *byte_chunks.term.mode());
        assert_eq!(
            one_chunk
                .cell(0, 12)
                .hyperlink()
                .map(|link| link.uri().to_string()),
            byte_chunks
                .cell(0, 12)
                .hyperlink()
                .map(|link| link.uri().to_string())
        );
    }
}
