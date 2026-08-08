//! Terminal view behavior tests.

use super::*;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use async_channel::TrySendError;

fn keystroke(source: &str) -> gpui::Keystroke {
    gpui::Keystroke::parse(source).expect("valid test keystroke")
}

#[test]
fn standard_system_notification_can_be_resolved_by_tag() {
    let states = HashMap::from([(
        "system-0".to_string(),
        NotificationState {
            tag: "crossh-terminal-7-0".to_string(),
            kitty_id: None,
            report_activation: false,
            report_close: false,
            focus_on_activation: true,
        },
    )]);

    let (key, state) =
        notification_state_for_tag(&states, "crossh-terminal-7-0").expect("notification");
    assert_eq!(key, "system-0");
    assert!(state.kitty_id.is_none());
    assert!(state.focus_on_activation);
    assert!(notification_state_for_tag(&states, "missing").is_none());
}

#[test]
fn protocol_parser_tracks_chunked_command_markers() {
    let mut parser = TerminalProtocolParser::default();
    assert!(parser.feed(b"output\x1b]13").is_empty());
    assert_eq!(
        parser.feed(b"3;C\x07command output"),
        vec![ProtocolEvent::Shell(ShellEvent::CommandStart)]
    );
    assert_eq!(
        parser.feed(b"\x1b]133;D;0\x1b\\"),
        vec![ProtocolEvent::Shell(ShellEvent::CommandFinished {
            status: Some(0)
        })]
    );
    assert_eq!(
        parser.feed(b"\x1b]133;A\x07prompt"),
        vec![ProtocolEvent::Shell(ShellEvent::PromptStart)]
    );
}

#[test]
fn selection_columns_are_ordered_for_same_line_drags() {
    assert_eq!(selection_column_bounds(2, 4, 8, 4), (2, 8));
    assert_eq!(selection_column_bounds(8, 4, 2, 4), (2, 8));
}

#[test]
fn timestamps_use_fixed_millisecond_precision() {
    let timestamp = format_timestamp(Local::now());
    assert_eq!(timestamp.len(), 12);
    assert_eq!(timestamp.as_bytes()[8], b'.');
    assert!(
        timestamp
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 2 | 5 | 8) || byte.is_ascii_digit())
    );
}

#[test]
fn timestamp_visibility_controls_terminal_content_origin() {
    let bounds = Bounds {
        origin: Point::new(px(10.), px(20.)),
        size: gpui::size(px(640.), px(300.)),
    };
    let with_gutter = terminal_bounds_for(bounds, true);
    assert_eq!(with_gutter.origin.x.as_f32(), 122.0);
    assert_eq!(with_gutter.size.width.as_f32(), 528.0);

    let without_gutter = terminal_bounds_for(bounds, false);
    assert_eq!(without_gutter.origin.x.as_f32(), 10.0);
    assert_eq!(without_gutter.size.width.as_f32(), 640.0);
}

#[test]
fn timestamp_tracker_preserves_rows_when_scrollback_grows() {
    let config = Config {
        scrolling_history: SCROLLBACK,
        ..Default::default()
    };
    let mut term: Term<NoopListener> = Term::new(
        config,
        &TermSize { cols: 20, rows: 2 },
        NoopListener::default(),
    );
    let mut parser: Processor = Processor::new();
    let mut tracker = TerminalTimestampState::default();

    parser.advance(&mut term, b"one\r\ntwo");
    tracker.observe(&term, "10:00:00.001".to_string());
    parser.advance(&mut term, b"\r\nthree");
    tracker.observe(&term, "10:00:00.002".to_string());

    assert_eq!(
        tracker.visible(&term),
        vec![
            Some("10:00:00.001".to_string()),
            Some("10:00:00.002".to_string())
        ]
    );
}

#[test]
fn timestamp_tracker_hides_wrapped_rows_and_alternate_screen() {
    let config = Config {
        scrolling_history: SCROLLBACK,
        ..Default::default()
    };
    let mut term: Term<NoopListener> = Term::new(
        config,
        &TermSize { cols: 5, rows: 3 },
        NoopListener::default(),
    );
    let mut parser: Processor = Processor::new();
    let mut tracker = TerminalTimestampState::default();

    parser.advance(&mut term, b"abcdef");
    tracker.observe(&term, "10:00:00.003".to_string());
    let visible = tracker.visible(&term);
    assert_eq!(visible[0], Some("10:00:00.003".to_string()));
    assert_eq!(visible[1], None);

    parser.advance(&mut term, b"\x1b[?1049h\x1b[2Jtui");
    assert!(term.mode().contains(TermMode::ALT_SCREEN));
    assert!(
        tracker
            .visible(&term)
            .into_iter()
            .all(|stamp| stamp.is_none())
    );

    parser.advance(&mut term, b"\x1b[?1049l");
    assert!(!term.mode().contains(TermMode::ALT_SCREEN));
    assert_eq!(tracker.visible(&term)[0], Some("10:00:00.003".to_string()));
}

#[test]
fn timestamp_tracker_detects_capped_scrollback_shift() {
    let signature = |value: &str| {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        RowSignature {
            hash: hasher.finish(),
            has_content: true,
            text: value.to_string(),
            wraps_to_next: false,
        }
    };
    let old = [
        signature("one"),
        signature("two"),
        signature("three"),
        signature("four"),
    ];
    let new = [
        signature("two"),
        signature("three"),
        signature("four"),
        signature("five"),
    ];
    assert_eq!(detect_scroll_shift(&old, &new), Some(1));
}

#[test]
fn timestamp_tracker_preserves_rows_after_resize_reflow() {
    let config = Config {
        scrolling_history: SCROLLBACK,
        ..Default::default()
    };
    let mut term: Term<NoopListener> = Term::new(
        config,
        &TermSize { cols: 5, rows: 4 },
        NoopListener::default(),
    );
    let mut parser: Processor = Processor::new();
    let mut tracker = TerminalTimestampState::default();

    parser.advance(&mut term, b"abcdefghij");
    tracker.observe(&term, "10:00:00.004".to_string());
    parser.advance(&mut term, b"\r\nnext");
    tracker.observe(&term, "10:00:00.005".to_string());

    term.resize(TermSize { cols: 20, rows: 6 });
    tracker.sync_to_term(&term);
    let visible = tracker.visible(&term);

    assert!(
        visible
            .iter()
            .any(|timestamp| timestamp.as_deref() == Some("10:00:00.004"))
    );
    assert!(
        visible
            .iter()
            .any(|timestamp| timestamp.as_deref() == Some("10:00:00.005"))
    );

    term.resize(TermSize { cols: 5, rows: 4 });
    tracker.sync_to_term(&term);
    let visible = tracker.visible(&term);
    assert!(
        visible
            .iter()
            .any(|timestamp| timestamp.as_deref() == Some("10:00:00.004"))
    );
    assert!(
        visible
            .iter()
            .any(|timestamp| timestamp.as_deref() == Some("10:00:00.005"))
    );
}

#[test]
fn encodes_navigation_keys_for_terminal_modes() {
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("up"), TermMode::NONE),
        Some(b"\x1b[A".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("up"), TermMode::APP_CURSOR),
        Some(b"\x1bOA".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("shift-left"), TermMode::NONE),
        Some(b"\x1b[1;2D".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("ctrl-right"), TermMode::NONE),
        Some(b"\x1b[1;5C".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("shift-tab"), TermMode::NONE),
        Some(b"\x1b[Z".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("f12"), TermMode::NONE),
        Some(b"\x1b[24~".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("ctrl-f12"), TermMode::NONE),
        Some(b"\x1b[24;5~".to_vec())
    );
}

#[test]
fn scroll_wheel_routes_to_the_expected_terminal_layer() {
    assert_eq!(
        wheel_route(TermMode::empty(), false),
        WheelRoute::LocalScrollback
    );
    assert_eq!(
        wheel_route(TermMode::MOUSE_DRAG, false),
        WheelRoute::MouseReport
    );
    assert_eq!(
        wheel_route(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL, false),
        WheelRoute::AlternateScroll
    );
    assert_eq!(
        wheel_route(
            TermMode::MOUSE_DRAG | TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL,
            true,
        ),
        WheelRoute::LocalScrollback
    );
}

#[test]
fn trackpad_scroll_waits_for_a_complete_terminal_line() {
    let mut scroll_acc = 0.;
    let pixels = |y| ScrollDelta::Pixels(Point::new(px(0.), px(y)));
    let lines = |y| ScrollDelta::Lines(Point::new(0., y));

    assert_eq!(
        wheel_lines_for_phase(TouchPhase::Started, pixels(5.), &mut scroll_acc, 10.),
        None
    );
    assert_eq!(
        wheel_lines_for_phase(TouchPhase::Moved, pixels(5.), &mut scroll_acc, 10.),
        None
    );
    assert_eq!(
        wheel_lines_for_phase(TouchPhase::Moved, pixels(5.), &mut scroll_acc, 10.),
        Some(1)
    );
    assert_eq!(
        wheel_lines_for_phase(TouchPhase::Cancelled, pixels(5.), &mut scroll_acc, 10.),
        None
    );
    assert_eq!(
        wheel_lines_for_phase(TouchPhase::Moved, lines(0.5), &mut scroll_acc, 10.),
        None
    );
    assert_eq!(
        wheel_lines_for_phase(TouchPhase::Moved, lines(0.5), &mut scroll_acc, 10.),
        Some(1)
    );
}

#[test]
fn alternate_scroll_follows_application_cursor_mode() {
    assert_eq!(
        alternate_scroll_sequence(TermMode::empty(), b'A'),
        [0x1b, b'[', b'A']
    );
    assert_eq!(
        alternate_scroll_sequence(TermMode::APP_CURSOR, b'A'),
        [0x1b, b'O', b'A']
    );
}

#[test]
fn encodes_control_and_printable_keys() {
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("a"), TermMode::NONE),
        Some(b"a".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("1"), TermMode::NONE),
        Some(b"1".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("-"), TermMode::NONE),
        Some(b"-".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("é"), TermMode::NONE),
        Some("é".as_bytes().to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("a"), TermMode::DISAMBIGUATE_ESC_CODES),
        Some(b"a".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("space"), TermMode::NONE),
        Some(b" ".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("ctrl-c"), TermMode::NONE),
        Some(vec![3])
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("ctrl-@"), TermMode::NONE),
        Some(vec![0])
    );
    #[cfg(target_os = "macos")]
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("cmd-l"), TermMode::REPORT_ALL_KEYS_AS_ESC),
        Some(vec![0x0c])
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("alt-x"), TermMode::NONE),
        Some(b"\x1bx".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("shift-a"), TermMode::NONE),
        Some(b"A".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("shift-1"), TermMode::NONE),
        Some(b"!".to_vec())
    );
}

#[test]
fn kitty_raw_images_are_normalized_to_png() {
    let rgba = kitty_raw_to_png(&[255, 0, 0, 255], 1, 1, 4).expect("RGBA PNG");
    assert_eq!(terminal_image_format(&rgba), Some(gpui::ImageFormat::Png));
    let rgb = kitty_raw_to_png(&[0, 255, 0], 1, 1, 3).expect("RGB PNG");
    assert_eq!(terminal_image_format(&rgb), Some(gpui::ImageFormat::Png));
    assert!(kitty_raw_to_png(&[0, 0, 0], 2, 1, 3).is_none());
}

#[test]
fn kitty_zlib_images_are_bounded_and_decoded() {
    use std::io::Write;

    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(b"kitty image data").unwrap();
    let compressed = encoder.finish().unwrap();
    assert_eq!(
        kitty_zlib_decode(&compressed),
        Some(b"kitty image data".to_vec())
    );
    assert!(kitty_zlib_decode(b"not zlib").is_none());
}

#[test]
fn kitty_graphics_chunks_keep_the_first_action() {
    assert_eq!(kitty_image_action("a=t,m=1", None), "t");
    assert_eq!(kitty_image_action("m=0", Some("t")), "t");
    assert_eq!(kitty_image_action("m=0", None), "t");
}

#[test]
fn kitty_placeholders_decode_ids_coordinates_and_inheritance() {
    assert_eq!(kitty_placeholder_diacritic_value('\u{0305}'), Some(0));
    assert_eq!(kitty_placeholder_diacritic_value('\u{030d}'), Some(1));
    assert_eq!(kitty_placeholder_diacritic_value('\u{030e}'), Some(2));
    assert_eq!(kitty_placeholder_diacritic_value('\u{0300}'), None);

    let mut first = Cell {
        c: KITTY_PLACEHOLDER_CHAR,
        fg: Color::Indexed(42),
        ..Cell::default()
    };
    first.set_underline_color(Some(Color::Indexed(7)));
    let (placeholder, state) = decode_kitty_placeholder(&first, "\u{0305}\u{0305}", 4, 8, None)
        .expect("first placeholder");
    assert_eq!(placeholder.image_id, 42);
    assert_eq!(placeholder.placement_id, Some(7));
    assert_eq!((placeholder.row, placeholder.column), (0, 0));

    let mut second = Cell {
        c: KITTY_PLACEHOLDER_CHAR,
        fg: Color::Indexed(42),
        ..Cell::default()
    };
    second.set_underline_color(Some(Color::Indexed(7)));
    let (placeholder, _) =
        decode_kitty_placeholder(&second, "", 4, 9, Some(state)).expect("inherited placeholder");
    assert_eq!((placeholder.row, placeholder.column), (0, 1));
    assert_eq!(placeholder.placement_id, Some(7));

    let high_byte = Cell {
        c: KITTY_PLACEHOLDER_CHAR,
        fg: Color::Indexed(42),
        ..Cell::default()
    };
    let (placeholder, _) =
        decode_kitty_placeholder(&high_byte, "\u{0305}\u{0305}\u{030e}", 4, 10, None)
            .expect("high image id byte");
    assert_eq!(placeholder.image_id, 42 | (2 << 24));
}

#[test]
fn terminal_snapshot_extracts_kitty_placeholders_from_the_grid() {
    let config = Config {
        scrolling_history: SCROLLBACK,
        ..Default::default()
    };
    let mut term: Term<NoopListener> = Term::new(
        config,
        &TermSize { cols: 4, rows: 2 },
        NoopListener::default(),
    );
    let mut parser: Processor = Processor::new();
    let bytes = format!(
        "\x1b[38;5;42m{p}{r0}{c0}{p}{r0}{c1}\r\n{p}{r1}{c0}\x1b[0m",
        p = KITTY_PLACEHOLDER_CHAR,
        r0 = '\u{0305}',
        r1 = '\u{030d}',
        c0 = '\u{0305}',
        c1 = '\u{030d}',
    );
    parser.advance(&mut term, bytes.as_bytes());

    let snapshot = snapshot_visible(&term, None, 4, false, &[]);
    assert_eq!(snapshot.kitty_placeholders.len(), 3);
    assert_eq!(
        snapshot
            .kitty_placeholders
            .iter()
            .map(|placeholder| (placeholder.row, placeholder.column))
            .collect::<Vec<_>>(),
        vec![(0, 0), (0, 1), (1, 0)]
    );
    assert!(snapshot.rows[0][0].kitty_placeholder);
    assert!(
        terminal_text_runs(&snapshot.rows[0])
            .iter()
            .all(|run| run.start_col >= 2)
    );
}

#[test]
fn encodes_kitty_keyboard_modes() {
    let disambiguate = TermMode::DISAMBIGUATE_ESC_CODES;
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("ctrl-c"), disambiguate),
        Some(b"\x1b[99;5u".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("alt-x"), disambiguate),
        Some(b"\x1b[120;3u".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("escape"), disambiguate),
        Some(b"\x1b[27u".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("enter"), disambiguate),
        Some(b"\r".to_vec())
    );

    let report_all = TermMode::REPORT_ALL_KEYS_AS_ESC;
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("a"), report_all),
        Some(b"\x1b[97u".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("up"), report_all),
        Some(b"\x1b[A".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("ctrl-up"), report_all),
        Some(b"\x1b[1;5A".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("f13"), report_all),
        Some(b"\x1b[57376u".to_vec())
    );

    let report_events = report_all | TermMode::REPORT_EVENT_TYPES;
    assert_eq!(
        encode_keystroke_with_event(&keystroke("a"), report_events, 2),
        Some(b"\x1b[97;1:2u".to_vec())
    );
    assert_eq!(
        encode_keystroke_with_event(&keystroke("a"), report_events, 3),
        Some(b"\x1b[97;1:3u".to_vec())
    );

    assert_eq!(
        encode_modify_other_keys(&keystroke("ctrl-;"), 2),
        Some(b"\x1b[27;5;59~".to_vec())
    );
    assert_eq!(encode_modify_other_keys(&keystroke("ctrl-c"), 1), None);
    assert_eq!(
        encode_modify_other_keys(&keystroke("a"), 3),
        Some(b"\x1b[27;1;97~".to_vec())
    );

    let disambiguate_events =
        disambiguate | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS;
    assert_eq!(
        encode_kitty_keystroke(&keystroke("a"), disambiguate_events, 3),
        None
    );
    assert_eq!(
        encode_kitty_keystroke(&keystroke("backspace"), disambiguate_events, 3),
        None
    );
    assert_eq!(
        encode_kitty_keystroke(&keystroke("up"), disambiguate_events, 3),
        Some(b"\x1b[1;1:3A".to_vec())
    );

    let enhanced = report_all
        | TermMode::REPORT_EVENT_TYPES
        | TermMode::REPORT_ALTERNATE_KEYS
        | TermMode::REPORT_ASSOCIATED_TEXT;
    assert_eq!(
        encode_keystroke_with_mode(&keystroke("shift-a"), enhanced),
        Some(b"\x1b[97:65;2:1;65u".to_vec())
    );
}

#[test]
fn encodes_mouse_protocols_and_buttons() {
    let plain_mode = TermMode::MOUSE_REPORT_CLICK;
    assert_eq!(mouse_button_code(MouseButton::Left), Some(0));
    assert_eq!(mouse_button_code(MouseButton::Middle), Some(1));
    assert_eq!(mouse_button_code(MouseButton::Right), Some(2));
    assert_eq!(
        encode_mouse_report(0, 0, 0, true, &Modifiers::default(), plain_mode, false),
        Some(vec![0x1b, b'[', b'M', 32, 33, 33])
    );

    let sgr_mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
    let modifiers = Modifiers {
        shift: true,
        ..Default::default()
    };
    assert_eq!(
        encode_mouse_report(2, 7, 4, true, &modifiers, sgr_mode, false),
        Some(b"\x1b[<6;8;5M".to_vec())
    );
    assert_eq!(
        encode_mouse_report(2, 7, 4, false, &modifiers, sgr_mode, false),
        Some(b"\x1b[<7;8;5m".to_vec())
    );

    let utf8_mode = TermMode::MOUSE_REPORT_CLICK | TermMode::UTF8_MOUSE;
    assert_eq!(
        encode_mouse_report(0, 95, 0, true, &Modifiers::default(), utf8_mode, false),
        Some(vec![0x1b, b'[', b'M', 32, 0xc2, 0x80, 33])
    );
}

#[test]
fn terminal_parser_tracks_application_and_mouse_modes() {
    let config = Config {
        scrolling_history: SCROLLBACK,
        kitty_keyboard: true,
        ..Default::default()
    };
    let mut term: Term<NoopListener> = Term::new(
        config,
        &TermSize { cols: 80, rows: 24 },
        NoopListener::default(),
    );
    let mut parser: Processor = Processor::new();
    parser.advance(&mut term, b"\x1b[?1h\x1b[?1002h\x1b[?1006h");

    let mode = *term.mode();
    assert!(mode.contains(TermMode::APP_CURSOR));
    assert!(mode.contains(TermMode::MOUSE_DRAG));
    assert!(mode.contains(TermMode::SGR_MOUSE));
}

#[test]
fn terminal_listener_forwards_terminal_responses() {
    let (listener, _, _, responses) = NoopListener::for_bridge(80, 24);
    listener.send_event(Event::PtyWrite("\x1b[0n".to_string()));

    assert_eq!(
        take_protocol_responses(&responses),
        vec![b"\x1b[0n".to_vec()]
    );
}

#[test]
fn terminal_listener_buffers_ui_side_effects() {
    let (listener, _, side_effects, _) = NoopListener::for_bridge(80, 24);
    listener.send_event(Event::Title("OpenCode".to_string()));

    let mut effects = side_effects.lock().expect("side effect queue");
    assert!(matches!(
        effects.pop_front(),
        Some(TerminalSideEffect::Title(title)) if title == "OpenCode"
    ));
}

#[test]
fn protocol_responses_survive_a_saturated_input_queue_in_order() {
    let (listener, _, _, responses) = NoopListener::for_bridge(80, 24);
    listener.send_event(Event::PtyWrite("one".to_string()));
    listener.send_event(Event::PtyWrite("two".to_string()));
    listener.send_event(Event::PtyWrite("three".to_string()));

    let (input_tx, input_rx) = async_channel::bounded(1);
    input_tx
        .try_send(InputCmd::Write(b"user".to_vec()))
        .expect("fill input queue");
    let mut pending = VecDeque::new();
    pending.extend(
        take_protocol_responses(&responses)
            .into_iter()
            .map(InputCmd::Write),
    );

    while let Some(command) = pending.pop_front() {
        match input_tx.try_send(command) {
            Ok(()) => {}
            Err(TrySendError::Full(command)) => {
                pending.push_front(command);
                break;
            }
            Err(TrySendError::Closed(_)) => panic!("input queue unexpectedly closed"),
        }
    }

    assert!(matches!(
        input_rx.try_recv().expect("user input"),
        InputCmd::Write(bytes) if bytes == b"user"
    ));
    let mut observed = Vec::new();
    while let Some(command) = pending.pop_front() {
        input_tx.try_send(command).expect("drain response queue");
        let command = input_rx.try_recv().expect("queued response");
        match command {
            InputCmd::Write(bytes) => observed.push(bytes),
            InputCmd::Resize { .. } | InputCmd::Close => panic!("unexpected command"),
        }
    }
    assert_eq!(
        observed,
        vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]
    );
}

#[test]
fn protocol_responses_are_flushed_before_close() {
    let (listener, _, _, responses) = NoopListener::for_bridge(80, 24);
    listener.send_event(Event::PtyWrite("reply".to_string()));

    let (input_tx, input_rx) = async_channel::bounded(1);
    input_tx
        .try_send(InputCmd::Write(b"user".to_vec()))
        .expect("fill input queue");
    let mut pending = VecDeque::new();
    for response in take_protocol_responses(&responses) {
        queue_input_nonblocking(&input_tx, &mut pending, InputCmd::Write(response));
    }
    queue_input_nonblocking(&input_tx, &mut pending, InputCmd::Close);

    assert!(matches!(
        input_rx.try_recv().expect("user input"),
        InputCmd::Write(bytes) if bytes == b"user"
    ));
    flush_pending_commands(&input_tx, &mut pending);
    assert!(matches!(
        input_rx.try_recv().expect("protocol response"),
        InputCmd::Write(bytes) if bytes == b"reply"
    ));
    flush_pending_commands(&input_tx, &mut pending);
    assert!(matches!(
        input_rx.try_recv().expect("close"),
        InputCmd::Close
    ));
    assert!(pending.is_empty());
}

#[test]
fn osc52_policy_denies_remote_reads_and_rejects_oversized_payloads() {
    assert!(!osc52_load_allowed(false));
    assert!(osc52_load_allowed(true));
    assert_eq!(osc52_mode(false), Osc52::OnlyCopy);
    assert_eq!(osc52_mode(true), Osc52::CopyPaste);
    assert!(osc52_text_within_limit("safe"));
    let oversized = "x".repeat(MAX_OSC52_CLIPBOARD_BYTES + 1);
    assert!(!osc52_text_within_limit(&oversized));

    let formatter: Arc<dyn Fn(&str) -> String + Sync + Send> =
        Arc::new(|_| "x".repeat(MAX_OSC52_RESPONSE_BYTES + 1));
    assert!(format_osc52_response(&formatter, "safe").is_none());
}

#[test]
fn ime_cursor_position_accounts_for_scrollback_offset() {
    assert_eq!(cursor_viewport_position(2, 9, 0, 24, 80), Some((9, 2)));
    assert_eq!(cursor_viewport_position(0, 99, 3, 24, 80), Some((79, 3)));
    assert_eq!(cursor_viewport_position(-4, 0, 3, 24, 80), None);
    assert_eq!(cursor_viewport_position(0, 0, 0, 0, 80), None);
}

#[test]
fn ime_marked_text_length_uses_utf16_units() {
    let text = "中😀文";
    assert_eq!(utf16_len(text), 4);
    assert_eq!(utf16_len("中文"), 2);
}

#[test]
fn terminal_render_keeps_wide_characters_on_their_grid_columns() {
    let config = Config {
        scrolling_history: SCROLLBACK,
        ..Default::default()
    };
    let mut term: Term<NoopListener> = Term::new(
        config,
        &TermSize { cols: 8, rows: 2 },
        NoopListener::default(),
    );
    let mut parser: Processor = Processor::new();
    parser.advance(&mut term, "a中b".as_bytes());

    let snapshot = snapshot_visible(&term, None, 8, true, &[]);
    assert!(snapshot.rows[0][1].wide);
    assert!(snapshot.rows[0][2].spacer);
    assert_eq!(cursor_visual_span(&snapshot.rows[0], 1), (1, 2, 1));
    assert_eq!(cursor_visual_span(&snapshot.rows[0], 2), (1, 2, 1));

    let runs = terminal_text_runs(&snapshot.rows[0][..4]);
    let positions: Vec<_> = runs
        .iter()
        .map(|run| {
            (
                run.start_col,
                run.cell_count,
                run.force_width_cells,
                run.text.clone(),
            )
        })
        .collect();
    assert_eq!(
        positions,
        vec![
            (0, 1, 1, "a".to_string()),
            (1, 2, 2, "中".to_string()),
            (3, 1, 1, "b".to_string()),
        ]
    );
}

#[test]
fn terminal_snapshot_applies_inverse_after_palette_lookup() {
    let config = Config {
        scrolling_history: SCROLLBACK,
        ..Default::default()
    };
    let mut term: Term<NoopListener> = Term::new(
        config,
        &TermSize { cols: 4, rows: 1 },
        NoopListener::default(),
    );
    let mut parser: Processor = Processor::new();
    parser.advance(&mut term, b"\x1b[31;47mA\x1b[7mB\x1b[27mC");

    let snapshot = snapshot_visible(&term, None, 4, false, &[]);
    let red = default_palette(&NamedColor::Red);
    let white = default_palette(&NamedColor::White);

    assert_eq!(snapshot.rows[0][0].fg, red);
    assert_eq!(snapshot.rows[0][0].bg, white);
    assert_eq!(snapshot.rows[0][1].fg, white);
    assert_eq!(snapshot.rows[0][1].bg, red);
    assert_eq!(snapshot.rows[0][2].fg, red);
    assert_eq!(snapshot.rows[0][2].bg, white);
}

#[test]
fn terminal_snapshot_preserves_text_decorations_and_hidden_text() {
    let config = Config {
        scrolling_history: SCROLLBACK,
        ..Default::default()
    };
    let mut term: Term<NoopListener> = Term::new(
        config,
        &TermSize { cols: 3, rows: 1 },
        NoopListener::default(),
    );
    let mut parser: Processor = Processor::new();
    parser.advance(&mut term, b"\x1b[4;9mX\x1b[0m\x1b[8mY");

    let snapshot = snapshot_visible(&term, None, 3, false, &[]);
    assert_eq!(snapshot.rows[0][0].underline, UnderlineKind::Solid);
    assert!(snapshot.rows[0][0].strikeout);
    assert_eq!(snapshot.rows[0][1].fg, snapshot.rows[0][1].bg);
}

#[test]
fn terminal_snapshot_maps_plain_urls_to_cell_columns() {
    let config = Config {
        scrolling_history: SCROLLBACK,
        ..Default::default()
    };
    let mut term: Term<NoopListener> = Term::new(
        config,
        &TermSize { cols: 64, rows: 1 },
        NoopListener::default(),
    );
    let mut parser: Processor = Processor::new();
    parser.advance(
        &mut term,
        "中文 www.first.test https://second.test".as_bytes(),
    );

    let snapshot = snapshot_visible(&term, None, 64, false, &[]);
    assert_eq!(
        snapshot.urls,
        vec![
            (0, 5, 19, "www.first.test".to_string()),
            (0, 20, 39, "https://second.test".to_string()),
        ]
    );
    assert!(snapshot.rows[0][5].is_url);
    assert!(snapshot.rows[0][18].is_url);
    assert!(!snapshot.rows[0][19].is_url);
    assert!(snapshot.rows[0][20].is_url);
    assert!(snapshot.rows[0][38].is_url);
    assert!(!snapshot.rows[0][39].is_url);
}

#[test]
fn terminal_snapshot_preserves_osc8_hyperlink_targets() {
    let config = Config {
        scrolling_history: SCROLLBACK,
        ..Default::default()
    };
    let mut term: Term<NoopListener> = Term::new(
        config,
        &TermSize { cols: 8, rows: 1 },
        NoopListener::default(),
    );
    let mut parser: Processor = Processor::new();
    parser.advance(
        &mut term,
        b"\x1b]8;;https://example.com\x07Crossh\x1b]8;;\x07",
    );

    let snapshot = snapshot_visible(&term, None, 8, false, &[]);
    assert_eq!(
        snapshot.rows[0][0].hyperlink.as_deref(),
        Some("https://example.com")
    );
    assert_eq!(
        snapshot.urls.first(),
        Some(&(0, 0, 6, "https://example.com".to_string()))
    );
}

#[test]
fn bold_basic_colors_use_bright_palette_entries() {
    let cell = Cell {
        fg: Color::Named(NamedColor::Red),
        ..Cell::default()
    };
    let style = effective_cell_style(
        &Cell {
            flags: CellFlags::BOLD,
            ..cell
        },
        &alacritty_terminal::term::color::Colors::default(),
        default_palette(&NamedColor::Foreground),
        default_palette(&NamedColor::Background),
    );
    assert_eq!(style.fg, default_palette(&NamedColor::BrightRed));
}

/// 隔离测试：把真实 shell 输出（含 OSC 标题 / 颜色 / bracketed-paste /
/// \r\n 换行）喂给 `vte::ansi::Processor + alacritty Term`，验证 grid 里
/// 是否真的写入了 `ls` 的结果。用于把「解析」与「渲染」两个环节分开定位。
#[test]
fn term_parses_real_shell_ls_output() {
    let config = Config {
        scrolling_history: SCROLLBACK,
        ..Default::default()
    };
    let size = TermSize { cols: 80, rows: 10 };
    let mut term: Term<NoopListener> = Term::new(config, &size, NoopListener::default());
    let mut parser: Processor = Processor::new();

    // 取自 connect_and_run_ls 诊断的真实字节流（提示符 + echo ls + 结果 + 新提示符）。
    let bytes: &[u8] = b"\x1b[?2004h\x1b]0;ubuntu@vps: ~\x07\x1b[01;32mubuntu@vps\x1b[00m:\x1b[01;34m~\x1b[00m$ ls\r\n\x1b[?2004l\r\x1b[0m\x1b[01;34mbackup\x1b[0m  \x1b[01;34mcard\x1b[0m  \x1b[01;34medunest\x1b[0m\r\n\x1b[?2004h\x1b]0;ubuntu@vps: ~\x07\x1b[01;32mubuntu@vps\x1b[00m:\x1b[01;34m~\x1b[00m$ ";

    parser.advance(&mut term, bytes);

    // 打印整个屏幕 + scrollback 顶部若干行，便于诊断。
    let grid = term.grid();
    let cols = grid.columns();
    let screen = grid.screen_lines();
    println!("=== screen {}x{} ===", cols, screen);
    let mut screen_text = String::new();
    for r in 0..screen {
        let row = &grid[Line(r as i32)];
        let s: String = (0..cols).map(|c| row[Column(c)].c).collect();
        let t = s.trim_end();
        if !t.is_empty() {
            println!("row {:2}: {:?}", r, t);
        }
        screen_text.push_str(&s);
    }

    assert!(screen_text.contains("backup"), "grid missing 'backup'");
    assert!(screen_text.contains("card"), "grid missing 'card'");
}

/// 关键测试：模拟 GUI 的 maybe_resize 流程 —— 先解析输出，再 resize term，
/// 然后用 snapshot_visible 的逻辑（display_offset 决定可见区）检查 ls 结果
/// 是否还在可见区。用于定位「resize 后内容消失」类问题。
#[test]
fn term_resize_keeps_ls_visible() {
    let config = Config {
        scrolling_history: SCROLLBACK,
        ..Default::default()
    };
    let mut term: Term<NoopListener> = Term::new(
        config,
        &TermSize { cols: 80, rows: 10 },
        NoopListener::default(),
    );
    let mut parser: Processor = Processor::new();

    let bytes: &[u8] = b"\x1b[?2004h\x1b]0;ubuntu@vps: ~\x07\x1b[01;32mubuntu@vps\x1b[00m:\x1b[01;34m~\x1b[00m$ ls\r\n\x1b[?2004l\r\x1b[0m\x1b[01;34mbackup\x1b[0m  \x1b[01;34mcard\x1b[0m\r\n\x1b[?2004h\x1b]0;ubuntu@vps: ~\x07\x1b[01;32mubuntu@vps\x1b[00m:\x1b[01;34m~\x1b[00m$ ";
    parser.advance(&mut term, bytes);

    let grid = term.grid();
    println!(
        "before resize: display_offset={} total={} screen={}",
        grid.display_offset(),
        grid.total_lines(),
        grid.screen_lines()
    );

    // 模拟 maybe_resize：80x10 -> 100x30。
    term.resize(TermSize {
        cols: 100,
        rows: 30,
    });

    let grid = term.grid();
    println!(
        "after resize: display_offset={} total={} screen={}",
        grid.display_offset(),
        grid.total_lines(),
        grid.screen_lines()
    );

    // 用 snapshot_visible 的逻辑读取可见区。
    let display_offset = grid.display_offset();
    let cols = grid.columns();
    let rows = grid.screen_lines();
    let top_visible = Line(-(display_offset as i32));
    let mut all = String::new();
    for r in 0..rows {
        let line = Line(top_visible.0 + r as i32);
        let row = &grid[line];
        let s: String = (0..cols).map(|c| row[Column(c)].c).collect();
        let t = s.trim_end();
        if !t.is_empty() {
            println!("visible row {:2}: {:?}", r, t);
        }
        all.push_str(&s);
    }

    assert!(
        all.contains("backup"),
        "after resize, visible area missing 'backup'"
    );
    assert!(
        all.contains("card"),
        "after resize, visible area missing 'card'"
    );
}

/// 验证把字节流切成极小 chunk（模拟 drain 分批 advance）后，
/// parser 仍能把 ls 结果正确写入 grid（跨 chunk 的 OSC/CSI 不断）。
#[test]
fn term_parses_chunked_output() {
    let config = Config {
        scrolling_history: SCROLLBACK,
        ..Default::default()
    };
    let mut term: Term<NoopListener> = Term::new(
        config,
        &TermSize { cols: 80, rows: 10 },
        NoopListener::default(),
    );
    let mut parser: Processor = Processor::new();

    let bytes: &[u8] = b"\x1b[?2004h\x1b]0;ubuntu@vps: ~\x07\x1b[01;32mubuntu@vps\x1b[00m:\x1b[01;34m~\x1b[00m$ ls\r\n\x1b[?2004l\r\x1b[0m\x1b[01;34mbackup\x1b[0m  \x1b[01;34mcard\x1b[0m\r\n\x1b[?2004h\x1b]0;ubuntu@vps: ~\x07\x1b[01;32mubuntu@vps\x1b[00m:\x1b[01;34m~\x1b[00m$ ";

    // 最严格：每字节一个 chunk。
    for chunk in bytes.chunks(1) {
        parser.advance(&mut term, chunk);
    }

    let grid = term.grid();
    let cols = grid.columns();
    let screen = grid.screen_lines();
    let mut screen_text = String::new();
    for r in 0..screen {
        let row = &grid[Line(r as i32)];
        let s: String = (0..cols).map(|c| row[Column(c)].c).collect();
        let t = s.trim_end();
        if !t.is_empty() {
            println!("chunked row {:2}: {:?}", r, t);
        }
        screen_text.push_str(&s);
    }

    assert!(screen_text.contains("backup"), "chunked: missing backup");
    assert!(screen_text.contains("card"), "chunked: missing card");
}
