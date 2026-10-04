//! Note Viewer UI：由独立的 `crossh-note` 进程承载。

use gpui::{App, KeyBinding, actions};

/// 查找面板自己的 key context。查找框获得焦点后，Enter / Shift+Enter / Escape
/// 都要归查找面板管（翻匹配、关面板），不能冒泡到窗口或编辑器。
pub const FIND_CONTEXT: &str = "NoteFind";

actions!(
    note_window,
    [
        CloseNoteWindow,
        NewNote,
        DeleteNote,
        TogglePreview,
        SaveNote,
        SelectNextNote,
        SelectPrevNote,
        EscapePreview,
        ToggleFind,
        FindNext,
        FindPrev,
        CloseFind
    ]
);

const NOTE_WINDOW_CONTEXT: &str = "NoteWindow";

pub fn init(cx: &mut App) {
    // 初始化 crossh-editor 的 Input 键位（TextArea/Input 的 undo/移动/选择等）
    crossh_editor::init(cx);
    cx.bind_keys([
        // 关闭走 ⌘W / Ctrl+W（与平台惯例一致）。Escape 不再关窗：旧绑定会让
        // 冒泡上来的 Escape 直接 remove_window，绕过窗口的脏保护、丢掉草稿。
        KeyBinding::new("cmd-w", CloseNoteWindow, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("ctrl-w", CloseNoteWindow, Some(NOTE_WINDOW_CONTEXT)),
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
        // Escape 只退出预览：预览是只读视图，退回编辑是安全且可逆的。
        // 关窗改用 ⌘W（见上），由 request_close 的脏检查把关。
        KeyBinding::new("escape", EscapePreview, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("escape", EscapePreview, Some("Input")),
        // 当 Input/Textarea 聚焦时（context=Input），仍允许窗口级动作冒泡
        KeyBinding::new("cmd-w", CloseNoteWindow, Some("Input")),
        KeyBinding::new("ctrl-w", CloseNoteWindow, Some("Input")),
        KeyBinding::new("cmd-n", NewNote, Some("Input")),
        KeyBinding::new("cmd-s", SaveNote, Some("Input")),
        KeyBinding::new("cmd-d", DeleteNote, Some("Input")),
        KeyBinding::new("cmd-shift-p", TogglePreview, Some("Input")),
        KeyBinding::new("ctrl-shift-p", TogglePreview, Some("Input")),
        // 笔记内查找：Cmd/Ctrl+F 开面板，F3 / Cmd+G 翻匹配。
        KeyBinding::new("cmd-f", ToggleFind, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("ctrl-f", ToggleFind, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("cmd-f", ToggleFind, Some("Input")),
        KeyBinding::new("ctrl-f", ToggleFind, Some("Input")),
        KeyBinding::new("f3", FindNext, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("cmd-g", FindNext, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("ctrl-g", FindNext, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("f3", FindNext, Some("Input")),
        KeyBinding::new("cmd-g", FindNext, Some("Input")),
        KeyBinding::new("ctrl-g", FindNext, Some("Input")),
        KeyBinding::new("shift-f3", FindPrev, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("shift-cmd-g", FindPrev, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("shift-ctrl-g", FindPrev, Some(NOTE_WINDOW_CONTEXT)),
        KeyBinding::new("shift-f3", FindPrev, Some("Input")),
        KeyBinding::new("shift-cmd-g", FindPrev, Some("Input")),
        KeyBinding::new("shift-ctrl-g", FindPrev, Some("Input")),
        // 查找框聚焦时：Enter 下一个、Shift+Enter 上一个、Escape 关面板。
        KeyBinding::new("enter", FindNext, Some(FIND_CONTEXT)),
        KeyBinding::new("shift-enter", FindPrev, Some(FIND_CONTEXT)),
        KeyBinding::new("escape", CloseFind, Some(FIND_CONTEXT)),
    ]);
}
mod find_panel;
mod format;
mod markdown;
mod window;
pub use window::open_note_window;
