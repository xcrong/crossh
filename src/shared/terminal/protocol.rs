//! Incremental terminal-side protocol handling that is intentionally kept
//! separate from the screen parser. Zed's terminal core owns the terminal
//! grid; this parser observes side-channel protocols and unwraps the
//! passthrough form used by tmux.

use base64::Engine;

const MAX_STRING_BYTES: usize = 8 * 1024 * 1024;
const MAX_NOTIFICATION_TEXT_BYTES: usize = 8 * 1024;
const MAX_IMAGE_BYTES: usize = 6 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtocolEvent {
    Title(String),
    Bell,
    ClipboardStore(String),
    ClipboardQuery(u8),
    KeyboardModeSet {
        bits: u8,
        behavior: u8,
    },
    KeyboardModePush {
        bits: u8,
    },
    KeyboardModePop(u16),
    KeyboardModeQuery,
    PrimaryDeviceAttributesQuery,
    SecondaryDeviceAttributesQuery,
    DeviceStatusQuery,
    CursorPositionQuery,
    Cwd(String),
    /// The command about to be executed, emitted by Crossh's local shell hook.
    Command(String),
    Shell(ShellEvent),
    Notification {
        title: String,
        body: String,
    },
    /// One part of Kitty's OSC 99 notification protocol. Kitty commonly sends
    /// the title and body as separate OSCs with the same notification id.
    NotificationPart {
        id: String,
        title: Option<String>,
        body: Option<String>,
        complete: bool,
        occasion: Option<NotificationOccasion>,
        report_activation: Option<bool>,
        report_close: Option<bool>,
        focus_on_activation: Option<bool>,
        expiry_ms: Option<i64>,
        buttons: Option<Vec<String>>,
    },
    KittyNotificationQuery {
        id: String,
    },
    KittyNotificationClose {
        id: String,
    },
    KittyNotificationAliveQuery {
        id: String,
    },
    KittyNotificationAliveResponse {
        id: String,
        alive: Vec<String>,
    },
    /// Windows Terminal's OSC 9;4 task progress protocol.
    Progress {
        state: u8,
        progress: Option<u8>,
    },
    Image(ImagePayload),
    KittyGraphics(KittyGraphicsPayload),
    /// A complete Sixel DCS, including its introducer and string terminator.
    Sixel(Vec<u8>),
    Decrqss(Vec<u8>),
    XtGetTcap(Vec<u8>),
    UrxvtMouse(bool),
    /// xterm modifyOtherKeys level (0 through 3).
    ModifyOtherKeys(u8),
    ModifyOtherKeysQuery,
    /// Report the pixel dimensions of the terminal window (`CSI 14 t`).
    WindowSizeQuery,
    /// Report the character dimensions of the terminal text area (`CSI 18/19 t`).
    TextAreaSizeQuery,
    /// Report the pixel dimensions of one terminal character cell (`CSI 16 t`).
    CellSizeQuery,
    /// Bytes carried inside tmux's `DCS tmux; ... ST` wrapper.
    Passthrough(Vec<u8>),
    /// The terminal reset sequence (`RIS`) clears screen-attached graphics.
    Reset,
    /// Erase operations that, by terminal graphics convention, clear visible
    /// image placements as well as the corresponding screen contents.
    ClearImages,
    /// Switch between the main and alternate screen buffers.
    ScreenBufferSwitch(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationOccasion {
    Always,
    Unfocused,
    Invisible,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImagePayload {
    pub data: Vec<u8>,
    pub width: Option<ImageDimension>,
    pub height: Option<ImageDimension>,
    pub preserve_aspect_ratio: bool,
    pub do_not_move_cursor: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageDimension {
    Cells(usize),
    Pixels(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KittyGraphicsPayload {
    pub control: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellEvent {
    PromptStart,
    PromptEnd,
    CommandStart,
    CommandFinished { status: Option<i32> },
}

#[derive(Default)]
pub struct TerminalProtocolParser {
    state: State,
    utf8_continuations: u8,
}

#[derive(Default)]
enum State {
    #[default]
    Ground,
    Escape,
    Csi(Vec<u8>),
    String(StringState),
    StringEscape(StringState),
}

struct StringState {
    kind: StringKind,
    payload: Vec<u8>,
    overflowed: bool,
}

#[derive(Clone, Copy)]
enum StringKind {
    Osc,
    Dcs,
    Apc,
    Ignore,
}

impl TerminalProtocolParser {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<ProtocolEvent> {
        let mut events = Vec::new();
        for &byte in bytes {
            self.step(byte, &mut events);
        }
        events
    }

    fn step(&mut self, byte: u8, events: &mut Vec<ProtocolEvent>) {
        let utf8_continuation = self.utf8_continuations > 0 && (0x80..=0xbf).contains(&byte);
        if utf8_continuation {
            self.utf8_continuations -= 1;
        } else if (0xc2..=0xf4).contains(&byte) {
            self.utf8_continuations = match byte {
                0xc2..=0xdf => 1,
                0xe0..=0xef => 2,
                _ => 3,
            };
        } else if self.utf8_continuations != 0 {
            self.utf8_continuations = 0;
        }
        let state = std::mem::replace(&mut self.state, State::Ground);
        match state {
            State::Ground => match byte {
                0x07 => events.push(ProtocolEvent::Bell),
                0x1b => self.state = State::Escape,
                0x90 if !utf8_continuation => {
                    self.state = State::String(new_string(StringKind::Dcs))
                }
                0x9b if !utf8_continuation => self.state = State::Csi(Vec::new()),
                0x9d if !utf8_continuation => {
                    self.state = State::String(new_string(StringKind::Osc))
                }
                0x9e | 0x9f if !utf8_continuation => {
                    self.state = State::String(new_string(if byte == 0x9f {
                        StringKind::Apc
                    } else {
                        StringKind::Ignore
                    }))
                }
                _ => {}
            },
            State::Escape => match byte {
                b']' => self.state = State::String(new_string(StringKind::Osc)),
                b'P' => self.state = State::String(new_string(StringKind::Dcs)),
                b'_' => self.state = State::String(new_string(StringKind::Apc)),
                b'X' | b'^' => self.state = State::String(new_string(StringKind::Ignore)),
                b'[' => self.state = State::Csi(Vec::new()),
                b'c' => events.push(ProtocolEvent::Reset),
                0x1b => self.state = State::Escape,
                _ => {}
            },
            State::Csi(mut payload) => {
                if byte == 0x18 || byte == 0x1a || (byte == 0x9c && !utf8_continuation) {
                    return;
                }
                if (0x40..=0x7e).contains(&byte) {
                    events.extend(parse_csi(&payload, byte));
                } else if byte == 0x1b {
                    self.state = State::Escape;
                } else if payload.len() < MAX_STRING_BYTES {
                    payload.push(byte);
                    self.state = State::Csi(payload);
                } else {
                    self.state = State::Csi(payload);
                }
            }
            State::String(mut string) => {
                if byte == 0x18 || byte == 0x1a {
                    return;
                }
                if byte == 0x9c && !utf8_continuation {
                    events.extend(finish_string(string));
                } else if byte == 0x1b {
                    self.state = State::StringEscape(string);
                } else if byte == 0x07 && matches!(string.kind, StringKind::Osc) {
                    events.extend(finish_string(string));
                } else {
                    append_byte(&mut string, byte);
                    self.state = State::String(string);
                }
            }
            State::StringEscape(mut string) => {
                if byte == b'\\' || (byte == 0x9c && !utf8_continuation) {
                    events.extend(finish_string(string));
                } else if byte == 0x1b {
                    append_byte(&mut string, 0x1b);
                    self.state = State::String(string);
                } else {
                    append_byte(&mut string, 0x1b);
                    self.state = State::String(string);
                    self.step(byte, events);
                }
            }
        }
    }
}

fn new_string(kind: StringKind) -> StringState {
    StringState {
        kind,
        payload: Vec::new(),
        overflowed: false,
    }
}

fn append_byte(string: &mut StringState, byte: u8) {
    if string.payload.len() < MAX_STRING_BYTES {
        string.payload.push(byte);
    } else {
        string.overflowed = true;
    }
}

fn finish_string(string: StringState) -> Vec<ProtocolEvent> {
    if string.overflowed {
        return Vec::new();
    }
    match string.kind {
        StringKind::Osc => parse_osc(&string.payload),
        StringKind::Dcs => parse_dcs(&string.payload),
        StringKind::Apc => parse_apc(&string.payload),
        StringKind::Ignore => Vec::new(),
    }
}

fn parse_osc(payload: &[u8]) -> Vec<ProtocolEvent> {
    let Some(separator) = payload.iter().position(|byte| *byte == b';') else {
        return Vec::new();
    };
    let command = &payload[..separator];
    let value = &payload[separator + 1..];

    match command {
        b"0" | b"2" => {
            let title = String::from_utf8_lossy(value);
            if title.is_empty() {
                vec![ProtocolEvent::Title(String::new())]
            } else {
                vec![ProtocolEvent::Title(title.into_owned())]
            }
        }
        b"52" => parse_osc52(value),
        b"7" => {
            let value = String::from_utf8_lossy(value);
            cwd_from_osc7(&value)
                .map(|cwd| vec![ProtocolEvent::Cwd(cwd)])
                .unwrap_or_default()
        }
        b"9" => {
            if value.starts_with(b"4;") {
                return parse_progress(value);
            }
            notification(b"", value)
        }
        b"777" => parse_osc777(value),
        b"99" => parse_kitty_notification(value),
        b"133" => parse_osc133(value),
        b"1337" => parse_osc1337(value),
        _ => Vec::new(),
    }
}

fn parse_osc52(value: &[u8]) -> Vec<ProtocolEvent> {
    let Some(separator) = value.iter().position(|byte| *byte == b';') else {
        return Vec::new();
    };
    let selector = &value[..separator];
    if !matches!(selector, b"c" | b"p" | b"s" | b"0") {
        return Vec::new();
    }
    let encoded = &value[separator + 1..];
    if encoded == b"?" {
        return vec![ProtocolEvent::ClipboardQuery(selector[0])];
    }
    let Ok(decoded) = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(encoded))
    else {
        return Vec::new();
    };
    if decoded.len() > 1024 * 1024 {
        return Vec::new();
    }
    let Ok(text) = String::from_utf8(decoded) else {
        return Vec::new();
    };
    vec![ProtocolEvent::ClipboardStore(text)]
}

fn parse_osc1337(value: &[u8]) -> Vec<ProtocolEvent> {
    if let Some(encoded) = value.strip_prefix(b"crossh-command=") {
        let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
            return Vec::new();
        };
        let Ok(command) = String::from_utf8(decoded) else {
            return Vec::new();
        };
        if command.is_empty() || command.len() > 16 * 1024 || command.contains('\0') {
            return Vec::new();
        }
        return vec![ProtocolEvent::Command(command)];
    }
    parse_iterm_image(value)
}

fn parse_progress(value: &[u8]) -> Vec<ProtocolEvent> {
    let mut fields = value.split(|byte| *byte == b';');
    if fields.next() != Some(b"4") {
        return Vec::new();
    }
    let Some(state) = fields
        .next()
        .filter(|field| !field.is_empty())
        .and_then(|field| std::str::from_utf8(field).ok())
        .and_then(|field| field.parse::<u8>().ok())
        .filter(|state| *state <= 4)
    else {
        return Vec::new();
    };
    let progress = fields
        .next()
        .filter(|field| !field.is_empty())
        .and_then(|field| std::str::from_utf8(field).ok())
        .and_then(|field| field.parse::<u8>().ok())
        .filter(|progress| *progress <= 100);
    if state != 0 && state != 3 && progress.is_none() {
        return Vec::new();
    }
    vec![ProtocolEvent::Progress { state, progress }]
}

fn parse_osc777(value: &[u8]) -> Vec<ProtocolEvent> {
    let mut fields = value.splitn(3, |byte| *byte == b';');
    if fields.next() != Some(b"notify") {
        return Vec::new();
    }
    let title = fields.next().unwrap_or_default();
    let body = fields.next().unwrap_or_default();
    notification(title, body)
}

fn parse_kitty_notification(value: &[u8]) -> Vec<ProtocolEvent> {
    let Some(separator) = value.iter().position(|byte| *byte == b';') else {
        return Vec::new();
    };
    let metadata = &value[..separator];
    let payload = &value[separator + 1..];
    let mut id = String::new();
    let mut payload_kind = None;
    let mut encoded = false;
    let mut complete = true;
    let mut occasion = None;
    let mut report_activation = None;
    let mut report_close = None;
    let mut focus_on_activation = None;
    let mut expiry_ms = None;

    for field in metadata.split(|byte| *byte == b':') {
        let Some(separator) = field.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let key = &field[..separator];
        let value = &field[separator + 1..];
        match key {
            b"i" => {
                id = sanitize_notification_id(&String::from_utf8_lossy(value)).unwrap_or_default()
            }
            b"p" => payload_kind = Some(value),
            b"e" => encoded = value == b"1",
            b"d" => complete = value != b"0",
            b"o" => {
                occasion = match value {
                    b"always" => Some(NotificationOccasion::Always),
                    b"unfocused" => Some(NotificationOccasion::Unfocused),
                    b"invisible" => Some(NotificationOccasion::Invisible),
                    _ => occasion,
                }
            }
            b"a" => {
                for action in value.split(|byte| *byte == b',') {
                    match action {
                        b"report" => report_activation = Some(true),
                        b"-report" => report_activation = Some(false),
                        b"focus" => focus_on_activation = Some(true),
                        b"-focus" => focus_on_activation = Some(false),
                        _ => {}
                    }
                }
            }
            b"c" => report_close = Some(value == b"1"),
            b"w" => {
                expiry_ms = std::str::from_utf8(value)
                    .ok()
                    .and_then(|value| value.parse::<i64>().ok())
                    .filter(|value| *value >= -1);
            }
            _ => {}
        }
    }
    if payload_kind == Some(b"?") {
        return vec![ProtocolEvent::KittyNotificationQuery { id }];
    }

    let payload = if encoded {
        let Ok(decoded) = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(payload))
        else {
            return Vec::new();
        };
        decoded
    } else {
        payload.to_vec()
    };
    if payload.len() > MAX_NOTIFICATION_TEXT_BYTES * 4 {
        return Vec::new();
    }
    if payload_kind == Some(b"close") {
        return vec![ProtocolEvent::KittyNotificationClose { id }];
    }
    if payload_kind == Some(b"alive") {
        let payload = clean_text(&payload);
        if payload.is_empty() {
            return vec![ProtocolEvent::KittyNotificationAliveQuery { id }];
        }
        let alive = payload
            .split(',')
            .filter_map(sanitize_notification_id)
            .collect();
        return vec![ProtocolEvent::KittyNotificationAliveResponse { id, alive }];
    }

    let (title, body, buttons) = match payload_kind {
        Some(b"title") => (Some(clean_text(&payload)), None, None),
        Some(b"body") => (None, Some(clean_text(&payload)), None),
        Some(b"buttons") => {
            let buttons = String::from_utf8_lossy(&payload)
                .split('\u{2028}')
                .map(str::as_bytes)
                .map(clean_text)
                .filter(|button| !button.is_empty())
                .collect::<Vec<_>>();
            (None, None, Some(buttons))
        }
        None if !complete => (Some(clean_text(&payload)), None, None),
        None => (None, Some(clean_text(&payload)), None),
        // Icon, sound, application metadata, urgency and future payloads are
        // intentionally ignored until the corresponding platform capability
        // exists in Crossh.
        _ => return Vec::new(),
    };

    vec![ProtocolEvent::NotificationPart {
        id,
        title,
        body,
        complete,
        occasion,
        report_activation,
        report_close,
        focus_on_activation,
        expiry_ms,
        buttons,
    }]
}

fn sanitize_notification_id(value: &str) -> Option<String> {
    let id = value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '+' | '.')
        })
        .take(128)
        .collect::<String>();
    (!id.is_empty()).then_some(id)
}

fn parse_osc133(value: &[u8]) -> Vec<ProtocolEvent> {
    let mut fields = value.split(|byte| *byte == b';');
    let Some(marker) = fields.next() else {
        return Vec::new();
    };
    match marker {
        b"A" => vec![ProtocolEvent::Shell(ShellEvent::PromptStart)],
        b"B" => vec![ProtocolEvent::Shell(ShellEvent::PromptEnd)],
        b"C" => vec![ProtocolEvent::Shell(ShellEvent::CommandStart)],
        b"D" => {
            let status = fields.next().and_then(|field| {
                std::str::from_utf8(field)
                    .ok()
                    .and_then(|field| field.parse::<i32>().ok())
            });
            vec![ProtocolEvent::Shell(ShellEvent::CommandFinished { status })]
        }
        _ => Vec::new(),
    }
}

fn parse_dcs(payload: &[u8]) -> Vec<ProtocolEvent> {
    if let Some(query) = payload.strip_prefix(b"$q") {
        return vec![ProtocolEvent::Decrqss(query.to_vec())];
    }
    if let Some(query) = payload.strip_prefix(b"+q") {
        return vec![ProtocolEvent::XtGetTcap(query.to_vec())];
    }
    if is_sixel_dcs(payload) {
        let mut sequence = Vec::with_capacity(payload.len() + 4);
        sequence.extend_from_slice(b"\x1bP");
        sequence.extend_from_slice(payload);
        sequence.extend_from_slice(b"\x1b\\");
        return vec![ProtocolEvent::Sixel(sequence)];
    }
    let Some(payload) = payload.strip_prefix(b"tmux;") else {
        return Vec::new();
    };

    let mut decoded = Vec::with_capacity(payload.len());
    let mut index = 0;
    while index < payload.len() {
        if payload[index] == 0x1b && payload.get(index + 1) == Some(&0x1b) {
            decoded.push(0x1b);
            index += 2;
        } else {
            decoded.push(payload[index]);
            index += 1;
        }
    }
    if decoded.is_empty() {
        Vec::new()
    } else {
        vec![ProtocolEvent::Passthrough(decoded)]
    }
}

fn is_sixel_dcs(payload: &[u8]) -> bool {
    if payload.first() == Some(&b'q') {
        return true;
    }
    let Some(q_index) = payload.iter().position(|byte| *byte == b'q') else {
        return false;
    };
    q_index > 0
        && payload[..q_index]
            .iter()
            .all(|byte| (0x30..=0x3f).contains(byte))
}

fn parse_apc(payload: &[u8]) -> Vec<ProtocolEvent> {
    let Some(payload) = payload.strip_prefix(b"G") else {
        return Vec::new();
    };
    let (control, encoded) = if let Some(separator) = payload.iter().position(|byte| *byte == b';')
    {
        (&payload[..separator], &payload[separator + 1..])
    } else {
        (payload, &payload[payload.len()..])
    };
    let data = if encoded.is_empty() {
        Vec::new()
    } else {
        let Ok(data) = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(encoded))
        else {
            return Vec::new();
        };
        data
    };
    vec![ProtocolEvent::KittyGraphics(KittyGraphicsPayload {
        control: String::from_utf8_lossy(control).into_owned(),
        data,
    })]
}

fn parse_csi(payload: &[u8], final_byte: u8) -> Vec<ProtocolEvent> {
    if matches!(final_byte, b'h' | b'l') && matches!(payload, b"?47" | b"?1047" | b"?1049") {
        return vec![ProtocolEvent::ScreenBufferSwitch(final_byte == b'h')];
    }
    if final_byte == b'J' && matches!(payload, b"2" | b"3") {
        return vec![ProtocolEvent::ClearImages];
    }
    if final_byte == b't' {
        return match payload {
            b"14" => vec![ProtocolEvent::WindowSizeQuery],
            b"16" => vec![ProtocolEvent::CellSizeQuery],
            b"18" | b"19" => vec![ProtocolEvent::TextAreaSizeQuery],
            _ => Vec::new(),
        };
    }
    if final_byte == b'u' {
        if payload == b"?" {
            return vec![ProtocolEvent::KeyboardModeQuery];
        }
        if let Some(value) = payload.strip_prefix(b"<") {
            let count = parse_u16(value).unwrap_or(1);
            return vec![ProtocolEvent::KeyboardModePop(count)];
        }
        if let Some(value) = payload.strip_prefix(b">") {
            return vec![ProtocolEvent::KeyboardModePush {
                bits: parse_u8(value).unwrap_or_default(),
            }];
        }
        if let Some(value) = payload.strip_prefix(b"=") {
            let mut fields = value.split(|byte| *byte == b';');
            let bits = fields.next().and_then(parse_u8).unwrap_or_default();
            let behavior = fields.next().and_then(parse_u8).unwrap_or(1);
            return vec![ProtocolEvent::KeyboardModeSet { bits, behavior }];
        }
    }
    if final_byte == b'c' {
        return match payload {
            b"" => vec![ProtocolEvent::PrimaryDeviceAttributesQuery],
            b">" => vec![ProtocolEvent::SecondaryDeviceAttributesQuery],
            _ => Vec::new(),
        };
    }
    if final_byte == b'n' {
        return match payload {
            b"5" => vec![ProtocolEvent::DeviceStatusQuery],
            b"6" => vec![ProtocolEvent::CursorPositionQuery],
            _ => Vec::new(),
        };
    }
    if final_byte == b'm' {
        if payload == b"?4" {
            return vec![ProtocolEvent::ModifyOtherKeysQuery];
        }
        if matches!(payload, b">0" | b">0;0") {
            return vec![ProtocolEvent::ModifyOtherKeys(0)];
        }
        // Vim exits modifyOtherKeys with the empty level form `CSI > 4 ; m`.
        // Treat it like the explicit level-zero variants so the next Ctrl-L
        // is not encoded as a literal modifyOtherKeys sequence.
        if matches!(payload, b">4" | b">4;") {
            return vec![ProtocolEvent::ModifyOtherKeys(0)];
        }
        if let Some(level) = payload
            .strip_prefix(b">4;")
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(|value| value.parse::<u8>().ok())
            .filter(|level| *level <= 3)
        {
            return vec![ProtocolEvent::ModifyOtherKeys(level)];
        }
    }
    if payload == b"?1015" {
        return match final_byte {
            b'h' => vec![ProtocolEvent::UrxvtMouse(true)],
            b'l' => vec![ProtocolEvent::UrxvtMouse(false)],
            _ => Vec::new(),
        };
    }
    Vec::new()
}

fn parse_u8(value: &[u8]) -> Option<u8> {
    std::str::from_utf8(value).ok()?.parse().ok()
}

fn parse_u16(value: &[u8]) -> Option<u16> {
    std::str::from_utf8(value).ok()?.parse().ok()
}

fn parse_iterm_image(value: &[u8]) -> Vec<ProtocolEvent> {
    let Some(separator) = value.iter().position(|byte| *byte == b':') else {
        return Vec::new();
    };
    let header = &value[..separator];
    let encoded = &value[separator + 1..];
    let Some(file) = header.strip_prefix(b"File=") else {
        return Vec::new();
    };

    let mut inline = false;
    let mut width = None;
    let mut height = None;
    let mut preserve_aspect_ratio = true;
    let mut do_not_move_cursor = false;
    for field in file.split(|byte| *byte == b';') {
        let Some(separator) = field.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let key = &field[..separator];
        let value = &field[separator + 1..];
        match key {
            b"inline" => inline = value == b"1",
            b"width" => width = parse_image_dimension(value),
            b"height" => height = parse_image_dimension(value),
            b"preserveAspectRatio" => preserve_aspect_ratio = value != b"0",
            b"doNotMoveCursor" => do_not_move_cursor = value == b"1",
            _ => {}
        }
    }
    if !inline {
        return Vec::new();
    }

    let Ok(data) = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(encoded))
    else {
        return Vec::new();
    };
    if data.is_empty() || data.len() > MAX_IMAGE_BYTES {
        return Vec::new();
    }
    vec![ProtocolEvent::Image(ImagePayload {
        data,
        width,
        height,
        preserve_aspect_ratio,
        do_not_move_cursor,
    })]
}

fn parse_image_dimension(value: &[u8]) -> Option<ImageDimension> {
    if value == b"auto" {
        return None;
    }
    if let Some(value) = value.strip_suffix(b"px") {
        return value
            .iter()
            .all(u8::is_ascii_digit)
            .then(|| ImageDimension::Pixels(parse_ascii_usize(value)))
            .filter(|dimension| !matches!(dimension, ImageDimension::Pixels(0)));
    }
    value
        .iter()
        .all(u8::is_ascii_digit)
        .then(|| ImageDimension::Cells(parse_ascii_usize(value)))
        .filter(|dimension| !matches!(dimension, ImageDimension::Cells(0)))
}

fn parse_ascii_usize(value: &[u8]) -> usize {
    value.iter().fold(0usize, |number, byte| {
        number
            .saturating_mul(10)
            .saturating_add((byte - b'0') as usize)
    })
}

fn notification(title: &[u8], body: &[u8]) -> Vec<ProtocolEvent> {
    let title = clean_text(title);
    let body = clean_text(body);
    if body.is_empty() {
        return Vec::new();
    }
    vec![ProtocolEvent::Notification { title, body }]
}

fn clean_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut output = String::with_capacity(text.len().min(MAX_NOTIFICATION_TEXT_BYTES));
    for character in text.chars() {
        if output.len() >= MAX_NOTIFICATION_TEXT_BYTES {
            break;
        }
        if !character.is_control() || matches!(character, '\n' | '\t') {
            output.push(character);
        }
    }
    output.trim().to_string()
}

fn cwd_from_osc7(value: &str) -> Option<String> {
    let path = if let Some(rest) = value.strip_prefix("file://") {
        if rest.starts_with('/') {
            rest.to_string()
        } else {
            rest.find('/').map(|index| rest[index..].to_string())?
        }
    } else {
        value.to_string()
    };
    let path = percent_decode(&path)?;
    std::path::Path::new(&path).is_absolute().then_some(path)
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hi = hex_value(*bytes.get(index + 1)?)?;
            let lo = hex_value(*bytes.get(index + 2)?)?;
            decoded.push((hi << 4) | lo);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_split_osc7_and_decodes_path() {
        let mut parser = TerminalProtocolParser::default();
        assert!(parser.feed(b"\x1b]7;file://localhost/Users/me").is_empty());
        assert_eq!(
            parser.feed(b"%20project\x07"),
            vec![ProtocolEvent::Cwd("/Users/me project".into())]
        );
    }

    #[test]
    fn parses_osc52_clipboard_writes_and_queries() {
        let mut parser = TerminalProtocolParser::default();
        assert_eq!(
            parser.feed(b"\x1b]52;c;SGVsbG8=\x07\x1b]52;p;?\x1b\\"),
            vec![
                ProtocolEvent::ClipboardStore("Hello".into()),
                ProtocolEvent::ClipboardQuery(b'p'),
            ]
        );
    }

    #[test]
    fn parses_shell_markers_and_exit_status() {
        let mut parser = TerminalProtocolParser::default();
        assert_eq!(
            parser.feed(b"\x1b]133;C\x07\x1b]133;D;127\x1b\\\x1b]133;A\x07"),
            vec![
                ProtocolEvent::Shell(ShellEvent::CommandStart),
                ProtocolEvent::Shell(ShellEvent::CommandFinished { status: Some(127) }),
                ProtocolEvent::Shell(ShellEvent::PromptStart),
            ]
        );
    }

    #[test]
    fn parses_crossh_command_marker() {
        let mut parser = TerminalProtocolParser::default();
        assert_eq!(
            parser.feed(b"\x1b]1337;crossh-command=Z2l0IHN0YXR1cw==\x07"),
            vec![ProtocolEvent::Command("git status".into())]
        );
    }

    #[test]
    fn parses_notifications() {
        let mut parser = TerminalProtocolParser::default();
        assert_eq!(
            parser.feed(b"\x1b]777;notify;Build;Done\x07"),
            vec![ProtocolEvent::Notification {
                title: "Build".into(),
                body: "Done".into(),
            }]
        );
        assert_eq!(
            parser.feed(b"\x1b]9;finished\x1b\\"),
            vec![ProtocolEvent::Notification {
                title: String::new(),
                body: "finished".into(),
            }]
        );
    }

    #[test]
    fn parses_windows_terminal_progress_states() {
        let mut parser = TerminalProtocolParser::default();
        assert_eq!(
            parser.feed(b"\x1b]9;4;1;42\x07\x1b]9;4;3\x1b\\\x1b]9;4;0\x07"),
            vec![
                ProtocolEvent::Progress {
                    state: 1,
                    progress: Some(42),
                },
                ProtocolEvent::Progress {
                    state: 3,
                    progress: None,
                },
                ProtocolEvent::Progress {
                    state: 0,
                    progress: None,
                },
            ]
        );
    }

    #[test]
    fn parses_kitty_notification_parts() {
        let mut parser = TerminalProtocolParser::default();
        assert_eq!(
            parser.feed(b"\x1b]99;i=build:d=0;Build\x1b\\"),
            vec![ProtocolEvent::NotificationPart {
                id: "build".into(),
                title: Some("Build".into()),
                body: None,
                complete: false,
                occasion: None,
                report_activation: None,
                report_close: None,
                focus_on_activation: None,
                expiry_ms: None,
                buttons: None,
            }]
        );
        assert_eq!(
            parser.feed(b"\x1b]99;i=build:p=body;Done\x1b\\"),
            vec![ProtocolEvent::NotificationPart {
                id: "build".into(),
                title: None,
                body: Some("Done".into()),
                complete: true,
                occasion: None,
                report_activation: None,
                report_close: None,
                focus_on_activation: None,
                expiry_ms: None,
                buttons: None,
            }]
        );
        assert_eq!(
            parser.feed(b"\x1b]99;;Finished\x1b\\"),
            vec![ProtocolEvent::NotificationPart {
                id: String::new(),
                title: None,
                body: Some("Finished".into()),
                complete: true,
                occasion: None,
                report_activation: None,
                report_close: None,
                focus_on_activation: None,
                expiry_ms: None,
                buttons: None,
            }]
        );
    }

    #[test]
    fn parses_kitty_notification_visibility_and_queries() {
        let mut parser = TerminalProtocolParser::default();
        assert_eq!(
            parser.feed(b"\x1b]99;i=build:o=always;Done\x1b\\"),
            vec![ProtocolEvent::NotificationPart {
                id: "build".into(),
                title: None,
                body: Some("Done".into()),
                complete: true,
                occasion: Some(NotificationOccasion::Always),
                report_activation: None,
                report_close: None,
                focus_on_activation: None,
                expiry_ms: None,
                buttons: None,
            }]
        );
        assert_eq!(
            parser.feed(b"\x1b]99;i=build:p=?;\x1b\\"),
            vec![ProtocolEvent::KittyNotificationQuery { id: "build".into() }]
        );
    }

    #[test]
    fn parses_kitty_notification_lifecycle_and_buttons() {
        let mut parser = TerminalProtocolParser::default();
        assert_eq!(
            parser.feed(
                b"\x1b]99;i=build:a=report,-focus:w=1500:p=buttons;Open\xe2\x80\xa8Logs\x1b\\"
            ),
            vec![ProtocolEvent::NotificationPart {
                id: "build".into(),
                title: None,
                body: None,
                complete: true,
                occasion: None,
                report_activation: Some(true),
                report_close: None,
                focus_on_activation: Some(false),
                expiry_ms: Some(1500),
                buttons: Some(vec!["Open".into(), "Logs".into()]),
            }]
        );
        assert_eq!(
            parser.feed(b"\x1b]99;i=build:p=close;\x1b\\"),
            vec![ProtocolEvent::KittyNotificationClose { id: "build".into() }]
        );
        assert_eq!(
            parser.feed(b"\x1b]99;i=build:p=alive;one,two\x1b\\"),
            vec![ProtocolEvent::KittyNotificationAliveResponse {
                id: "build".into(),
                alive: vec!["one".into(), "two".into()],
            }]
        );
        assert_eq!(
            parser.feed(b"\x1b]99;i=build:p=alive;\x1b\\"),
            vec![ProtocolEvent::KittyNotificationAliveQuery { id: "build".into() }]
        );
    }

    #[test]
    fn parses_iterm_inline_image_metadata() {
        let mut parser = TerminalProtocolParser::default();
        assert_eq!(
            parser.feed(b"\x1b]1337;File=name=preview;inline=1;width=12;height=40px:aGVsbG8=\x07"),
            vec![ProtocolEvent::Image(ImagePayload {
                data: b"hello".to_vec(),
                width: Some(ImageDimension::Cells(12)),
                height: Some(ImageDimension::Pixels(40)),
                preserve_aspect_ratio: true,
                do_not_move_cursor: false,
            })]
        );
    }

    #[test]
    fn parses_urxvt_mouse_mode() {
        let mut parser = TerminalProtocolParser::default();
        assert_eq!(
            parser.feed(b"\x1b[?1015h\x1b[?1015l"),
            vec![
                ProtocolEvent::UrxvtMouse(true),
                ProtocolEvent::UrxvtMouse(false)
            ]
        );
    }

    #[test]
    fn parses_modify_other_keys_mode() {
        let mut parser = TerminalProtocolParser::default();
        assert_eq!(
            parser.feed(b"\x1b[>4;2m\x1b[?4m\x1b[>4m"),
            vec![
                ProtocolEvent::ModifyOtherKeys(2),
                ProtocolEvent::ModifyOtherKeysQuery,
                ProtocolEvent::ModifyOtherKeys(0),
            ]
        );
        assert_eq!(
            parser.feed(b"\x1b[>4;3m"),
            vec![ProtocolEvent::ModifyOtherKeys(3)]
        );
        assert_eq!(
            parser.feed(b"\x1b[>4;0m\x1b[>0m\x1b[>0;0m"),
            vec![
                ProtocolEvent::ModifyOtherKeys(0),
                ProtocolEvent::ModifyOtherKeys(0),
                ProtocolEvent::ModifyOtherKeys(0),
            ]
        );

        let mut vim_parser = TerminalProtocolParser::default();
        assert_eq!(
            vim_parser.feed(b"\x1b[>4;2m\x1b[>4;m"),
            vec![
                ProtocolEvent::ModifyOtherKeys(2),
                ProtocolEvent::ModifyOtherKeys(0),
            ]
        );
    }

    #[test]
    fn parses_terminal_size_queries() {
        let mut parser = TerminalProtocolParser::default();
        assert!(parser.feed(b"\x1b[1").is_empty());
        assert_eq!(
            parser.feed(b"6t\x1b[14t\x1b[18t\x1b[19t"),
            vec![
                ProtocolEvent::CellSizeQuery,
                ProtocolEvent::WindowSizeQuery,
                ProtocolEvent::TextAreaSizeQuery,
                ProtocolEvent::TextAreaSizeQuery,
            ]
        );
    }

    #[test]
    fn parses_c1_forms_of_side_channel_protocols() {
        let mut parser = TerminalProtocolParser::default();
        assert!(parser.feed("中文".as_bytes()).is_empty());
        assert_eq!(
            parser.feed(b"\x9d9;x\xc2\x9cfinished\x9c"),
            vec![ProtocolEvent::Notification {
                title: String::new(),
                body: "xfinished".into(),
            }]
        );
        assert_eq!(
            parser.feed(b"\x9d9;finished\x9c\x9b16t"),
            vec![
                ProtocolEvent::Notification {
                    title: String::new(),
                    body: "finished".into(),
                },
                ProtocolEvent::CellSizeQuery,
            ]
        );
        let sixel = parser.feed(b"\x90q#0;2;100;0;0#0~-\x9c");
        assert!(matches!(sixel.as_slice(), [ProtocolEvent::Sixel(_)]));
    }

    #[test]
    fn parses_kitty_graphics_apc() {
        let mut parser = TerminalProtocolParser::default();
        assert_eq!(
            parser.feed(b"\x1b_Ga=T,f=100,i=7,m=0;aGVsbG8=\x1b\\"),
            vec![ProtocolEvent::KittyGraphics(KittyGraphicsPayload {
                control: "a=T,f=100,i=7,m=0".into(),
                data: b"hello".to_vec(),
            })]
        );
    }

    #[test]
    fn parses_sixel_dcs_without_treating_it_as_screen_text() {
        let mut parser = TerminalProtocolParser::default();
        let events = parser.feed(b"\x1bPq#0;2;100;0;0#0~-\x1b\\");
        assert_eq!(
            events,
            vec![ProtocolEvent::Sixel(
                b"\x1bPq#0;2;100;0;0#0~-\x1b\\".to_vec()
            )]
        );
        let ProtocolEvent::Sixel(sequence) = &events[0] else {
            unreachable!();
        };
        assert!(icy_sixel::SixelImage::decode(sequence).is_ok());
        assert!(parser.feed(b"q#0;2;100;0;0#0~-\x1b\\").is_empty());
    }

    #[test]
    fn recognizes_capability_queries_without_treating_them_as_text() {
        let mut parser = TerminalProtocolParser::default();
        assert_eq!(
            parser.feed(b"\x1bP$qm\x1b\\\x1bP+q544e\x1b\\"),
            vec![
                ProtocolEvent::Decrqss(b"m".to_vec()),
                ProtocolEvent::XtGetTcap(b"544e".to_vec())
            ]
        );
    }

    #[test]
    fn unwraps_tmux_passthrough_and_preserves_split_boundaries() {
        let mut parser = TerminalProtocolParser::default();
        assert!(parser.feed(b"\x1bPtmux;\x1b\x1b]9;fin").is_empty());
        assert_eq!(
            parser.feed(b"ished\x07\x1b\\"),
            vec![ProtocolEvent::Passthrough(b"\x1b]9;finished\x07".to_vec())]
        );
    }

    #[test]
    fn rejects_invalid_percent_escapes() {
        assert_eq!(percent_decode("a%20b"), Some("a b".into()));
        assert_eq!(percent_decode("a%2"), None);
        assert_eq!(percent_decode("a%GG"), None);
    }
}
