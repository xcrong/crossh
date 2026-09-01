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
    // 初始化 crossh-editor 的 Input 键位（TextArea/Input 的 undo/移动/选择等）
    crossh_editor::init(cx);
    cx.bind_keys([
        KeyBinding::new("escape", CloseNoteWindow, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("cmd-n", NewNote, Some(NOTE_WINDOW_CONTEXT)),
        // 当 Input/Textarea 聚焦时（context=Input），仍允许窗口级动作冒泡
        KeyBinding::new("escape", CloseNoteWindow, Some("Input")),
        KeyBinding::new("cmd-n", NewNote, Some("Input")),
        KeyBinding::new("cmd-s", SaveNote, Some("Input")),
        KeyBinding::new("cmd-d", DeleteNote, Some("Input")),
    ]);
}
mod markdown;
mod window;
pub(crate) use window::open_note_window;
