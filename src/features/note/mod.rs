//! Note Viewer UI：由独立的 `crossh-note` 进程承载。

use gpui::{App, KeyBinding, actions};

actions!(
    note_window,
    [
        CloseNoteWindow,
        NewNote,
        DeleteNote,
        TogglePreview,
        SaveNote,
        SelectNextNote,
        SelectPrevNote
    ]
);

const NOTE_WINDOW_CONTEXT: &str = "NoteWindow";

pub(crate) fn init(cx: &mut App) {
    // 初始化 crossh-editor 的 Input 键位（TextArea/Input 的 undo/移动/选择等）
    crossh_editor::init(cx);
    cx.bind_keys([
        KeyBinding::new("escape", CloseNoteWindow, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("cmd-n", NewNote, Some(NOTE_WINDOW_CONTEXT)),
        // 预览切换：macOS 用 cmd-shift-p，Linux 用 ctrl-shift-p；两处都绑上，
        // 在哪个平台多余的那个都无害（与现有 cmd-n/cmd-s/cmd-d 写法一致）。
        KeyBinding::new("cmd-shift-p", TogglePreview, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("ctrl-shift-p", TogglePreview, Some(NOTE_WINDOW_CONTEXT)),
        // 列表导航：Up/Down 与 ctrl-p/ctrl-n。
        // 说明：编辑器聚焦时 Input context 更具体（它自带 Up/Down 移动光标），
        // 因此 Up/Down 只在焦点不在编辑器时切列表；ctrl-p/ctrl-n 编辑器未占用，
        // 通过祖先 NoteWindow context 冒泡，编辑时也能用。
        KeyBinding::new("up", SelectPrevNote, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("down", SelectNextNote, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("ctrl-p", SelectPrevNote, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("ctrl-n", SelectNextNote, Some(NOTE_WINDOW_CONTEXT)),
        // 当 Input/Textarea 聚焦时（context=Input），仍允许窗口级动作冒泡
        KeyBinding::new("escape", CloseNoteWindow, Some("Input")),
        KeyBinding::new("cmd-n", NewNote, Some("Input")),
        KeyBinding::new("cmd-s", SaveNote, Some("Input")),
        KeyBinding::new("cmd-d", DeleteNote, Some("Input")),
        KeyBinding::new("cmd-shift-p", TogglePreview, Some("Input")),
        KeyBinding::new("ctrl-shift-p", TogglePreview, Some("Input")),
    ]);
}
mod markdown;
mod window;
pub(crate) use window::open_note_window;
