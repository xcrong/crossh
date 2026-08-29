//! Note Viewer UI：由独立的 `crossh-note` 进程承载。

use gpui::{App, KeyBinding, actions};

actions!(
    note_window,
    [
        CloseNoteWindow,
        NewNote,
        DeleteNote,
        TogglePreview,
        SaveNote
    ]
);

const NOTE_WINDOW_CONTEXT: &str = "NoteWindow";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", CloseNoteWindow, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("cmd-n", NewNote, Some(NOTE_WINDOW_CONTEXT)),
    ]);
}
mod markdown;
mod window;
pub(crate) use window::open_note_window;
