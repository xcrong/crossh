//! `NoteWindow` 的行为测试。
//!
//! 从 `window.rs` 移出：`scripts/check-architecture.sh` 限制单文件 2000 行，
//! 窗口逻辑本身已接近上限，测试不再占正文预算。`use super::*` 仍能拿到
//! `window` 模块的私有项（`NoteWindow` 的字段、`clamp_list_pane_width` 等）。

use super::*;
use crate::test_support::expected;
use crossh_note::Note;
use gpui::TestAppContext;

/// 查找动作会走到编辑器的命中/高亮渲染，那条路径读 `GlobalState`。
/// `NoteWindow::new` 不初始化它（生产路径由 `crossh_editor::init` 负责），
/// 所以凡是触发查找面板的测试都要先补上，否则 panic。
fn init_editor_state(cx: &mut TestAppContext) {
    cx.update(crossh_editor::init);
}

#[gpui::test]
fn note_window_historic_note_loads_in_edit_state(cx: &mut TestAppContext) {
    let window = cx.add_window(NoteWindow::new_for_test);
    cx.run_until_parked();
    window
        .update(cx, |note_window, window, cx| {
            let historic = Note {
                id: 1,
                content: "hello 历史笔记".to_string(),
                pinned: false,
                created_at: 0,
                updated_at: 0,
            };
            note_window.notes = vec![historic.clone()];
            note_window.select_note_focused(historic.id, window, cx);
            assert_eq!(note_window.notes.len(), 1);
            assert_eq!(note_window.selected_id, Some(historic.id));
            assert_eq!(
                note_window.content_state.read(cx).value().to_string(),
                "hello 历史笔记"
            );
            assert!(!note_window.preview);
            assert!(
                note_window
                    .content_state
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            note_window.content_state.update(cx, |state, cx| {
                state.set_value_simple("hello 历史笔记 modified", cx);
            });
            assert_eq!(
                note_window.content_state.read(cx).value().to_string(),
                "hello 历史笔记 modified"
            );
        })
        .unwrap();
}

#[gpui::test]
fn note_window_select_via_list_focuses_content(cx: &mut TestAppContext) {
    let window = cx.add_window(NoteWindow::new_for_test);
    cx.run_until_parked();
    window
        .update(cx, |note_window, window, cx| {
            let n1 = Note {
                id: 1,
                content: "first".to_string(),
                pinned: false,
                created_at: 0,
                updated_at: 0,
            };
            let n2 = Note {
                id: 2,
                content: "second".to_string(),
                pinned: false,
                created_at: 0,
                updated_at: 1,
            };
            note_window.notes = vec![n2.clone(), n1.clone()];
            note_window.selected_id = Some(n2.id);
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple(n2.content.clone(), cx));
            note_window.select_note_focused(n1.id, window, cx);
            assert_eq!(note_window.selected_id, Some(n1.id));
            assert_eq!(
                note_window.content_state.read(cx).value().to_string(),
                "first"
            );
            assert!(
                note_window
                    .content_state
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            assert!(!note_window.preview);
        })
        .unwrap();
}

#[gpui::test]
fn note_status_dirty_and_save_disabled(cx: &mut TestAppContext) {
    let window = cx.add_window(NoteWindow::new_for_test);
    cx.run_until_parked();
    window
        .update(cx, |note_window, _window, cx| {
            note_window.notes = Vec::new();
            note_window.selected_id = None;
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple("", cx));
            assert!(!note_window.is_dirty(cx));
            assert!(note_window.is_save_disabled(cx));
            assert_eq!(
                note_window.status_title(cx),
                expected("note.status.no_notes")
            );
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple("hello", cx));
            assert!(note_window.is_dirty(cx));
            assert!(!note_window.is_save_disabled(cx));
            assert_eq!(
                note_window.status_title(cx),
                expected("note.status.new_note")
            );
            let note = Note {
                id: 1,
                content: "hello".to_string(),
                pinned: false,
                created_at: 0,
                updated_at: 0,
            };
            note_window.notes = vec![note.clone()];
            note_window.selected_id = Some(note.id);
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple(note.content.clone(), cx));
            assert!(!note_window.is_dirty(cx));
            assert!(note_window.is_save_disabled(cx));
            assert_eq!(note_window.status_title(cx), "hello");
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple("hello world", cx));
            assert!(note_window.is_dirty(cx));
            assert!(!note_window.is_save_disabled(cx));
        })
        .unwrap();
}

#[gpui::test]
fn note_status_title_fallbacks(cx: &mut TestAppContext) {
    let window = cx.add_window(NoteWindow::new_for_test);
    cx.run_until_parked();
    window
        .update(cx, |note_window, _window, cx| {
            note_window.notes = vec![Note {
                id: 1,
                content: "".to_string(),
                pinned: false,
                created_at: 0,
                updated_at: 0,
            }];
            note_window.selected_id = None;
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple("", cx));
            assert_eq!(
                note_window.status_title(cx),
                expected("note.status.no_selection")
            );
        })
        .unwrap();
}

fn sample_note(id: i64, content: &str) -> Note {
    Note {
        id,
        content: content.to_string(),
        pinned: false,
        created_at: 0,
        updated_at: 0,
    }
}

#[gpui::test]
fn note_dirty_switch_without_store_keeps_moving(cx: &mut TestAppContext) {
    let window = cx.add_window(NoteWindow::new_for_test);
    cx.run_until_parked();
    window
        .update(cx, |note_window, _window, cx| {
            // 无库时无法落库但不阻塞切换（确定性路径，不碰真实笔记库）。
            let n1 = sample_note(1, "first");
            let n2 = sample_note(2, "second");
            note_window.notes = vec![n1.clone(), n2.clone()];
            note_window.selected_id = Some(n1.id);
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple(n1.content.clone(), cx));
            // 脏编辑后切换：直接切到 n2，不丢 dirty（无库则放行）。
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple("first 草稿", cx));
            assert!(note_window.is_dirty(cx));
            note_window.select_note(n2.id, cx);
            assert_eq!(note_window.selected_id, Some(n2.id));
            assert_eq!(
                note_window.content_state.read(cx).value().to_string(),
                "second"
            );
            // 同一条重复点击不折腾编辑器。
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple("second 草稿", cx));
            note_window.select_note(n2.id, cx);
            assert_eq!(
                note_window.content_state.read(cx).value().to_string(),
                "second 草稿"
            );
        })
        .unwrap();
}

#[gpui::test]
fn note_delete_requires_second_confirm(cx: &mut TestAppContext) {
    let window = cx.add_window(NoteWindow::new_for_test);
    cx.run_until_parked();
    window
        .update(cx, |note_window, window, cx| {
            let n1 = sample_note(1, "first");
            let n2 = sample_note(2, "second");
            note_window.notes = vec![n1.clone(), n2.clone()];
            note_window.selected_id = Some(n1.id);
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple(n1.content.clone(), cx));
            // 第一次：只标记，不删除。
            note_window.delete_current(window, cx);
            assert_eq!(note_window.selected_id, Some(n1.id));
            assert_eq!(note_window.pending_delete_id, Some(n1.id));
            assert_eq!(
                note_window.content_state.read(cx).value().to_string(),
                "first"
            );
            // 第二次：真删。库中 n1 已不在（此处手动模拟库侧删除），
            // reload 后自动选中剩余第一条 n2。
            note_window.notes = vec![n2.clone()];
            note_window.delete_current(window, cx);
            assert_eq!(note_window.pending_delete_id, None);
            assert_eq!(note_window.selected_id, Some(n2.id));
            assert_eq!(
                note_window.content_state.read(cx).value().to_string(),
                "second"
            );
            // 空库时真删：选中清空、编辑器清空。
            note_window.delete_current(window, cx);
            assert_eq!(note_window.pending_delete_id, Some(n2.id));
            note_window.notes = Vec::new();
            note_window.delete_current(window, cx);
            assert_eq!(note_window.selected_id, None);
            assert_eq!(note_window.pending_delete_id, None);
            assert_eq!(note_window.content_state.read(cx).value().to_string(), "");
        })
        .unwrap();
}

#[gpui::test]
fn note_move_selection_clamps_and_focuses(cx: &mut TestAppContext) {
    let window = cx.add_window(NoteWindow::new_for_test);
    cx.run_until_parked();
    window
        .update(cx, |note_window, window, cx| {
            let n1 = sample_note(1, "first");
            let n2 = sample_note(2, "second");
            note_window.notes = vec![n1.clone(), n2.clone()];
            note_window.selected_id = None;
            note_window.move_selection(1, window, cx);
            assert_eq!(note_window.selected_id, Some(n1.id));
            note_window.move_selection(1, window, cx);
            assert_eq!(note_window.selected_id, Some(n2.id));
            // 越界钳制：已在末尾则不动。
            note_window.move_selection(1, window, cx);
            assert_eq!(note_window.selected_id, Some(n2.id));
            note_window.move_selection(-1, window, cx);
            assert_eq!(note_window.selected_id, Some(n1.id));
            // 越界钳制：已在开头则不动。
            note_window.move_selection(-1, window, cx);
            assert_eq!(note_window.selected_id, Some(n1.id));
            assert_eq!(
                note_window.content_state.read(cx).value().to_string(),
                "first"
            );
            assert!(
                note_window
                    .content_state
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            // 空列表直接忽略，不 panic。
            note_window.notes = Vec::new();
            note_window.selected_id = None;
            note_window.move_selection(1, window, cx);
            assert_eq!(note_window.selected_id, None);
        })
        .unwrap();
}

#[gpui::test]
fn note_store_error_shows_in_title(cx: &mut TestAppContext) {
    let window = cx.add_window(NoteWindow::new_for_test);
    cx.run_until_parked();
    window
        .update(cx, |note_window, _window, cx| {
            note_window.store_error = Some("store open failed: test".to_string());
            note_window.notes = Vec::new();
            note_window.selected_id = None;
            assert_eq!(
                note_window.status_title(cx),
                expected("note.status.no_store")
            );
        })
        .unwrap();
}

#[gpui::test]
fn note_search_reload_is_debounced(cx: &mut TestAppContext) {
    use std::time::Duration;

    // 用临时库而不是真实笔记库：断言要确定，且不污染用户数据。
    let dir = tempfile::TempDir::new().unwrap();
    let store = NoteStore::open(dir.path().join("note.db")).unwrap();
    store.create("alpha").unwrap();

    let window = cx.add_window(NoteWindow::new_for_test);
    cx.run_until_parked();
    window
        .update(cx, |note_window, window, cx| {
            note_window.store = Some(store);
            note_window.refresh_list();
            // 塞入哨兵：同步 reload 会立刻把它清掉。
            note_window.notes.push(sample_note(-999, "哨兵"));
            note_window.search_query = TextEditingState::new("zzz-无匹配");
            window.focus(&note_window.search_focus, cx);
            note_window.on_search_changed(cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |note_window, _window, _cx| {
            // 代际已推进，但防抖期内未查库：哨兵仍在。
            assert_eq!(note_window.search_generation, 1);
            assert_eq!(note_window.search_query.value, "zzz-无匹配");
            assert!(note_window.notes.iter().any(|n| n.id == -999));
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    window
        .update(cx, |note_window, _window, _cx| {
            // 防抖到期后 reload 生效：哨兵被查询结果取代（此处无匹配，故为空）。
            assert!(
                !note_window.notes.iter().any(|n| n.id == -999),
                "防抖到期后应重新查库，哨兵不再留在列表里"
            );
        })
        .unwrap();
}

#[gpui::test]
fn note_search_escape_clears_before_close(cx: &mut TestAppContext) {
    let window = cx.add_window(NoteWindow::new_for_test);
    cx.run_until_parked();
    window
        .update(cx, |note_window, window, cx| {
            note_window.search_query = TextEditingState::new("abc");
            window.focus(&note_window.search_focus, cx);
            // 有内容：清空并截断（不关窗口）。
            assert!(note_window.clear_search_on_escape(window, cx));
            assert!(note_window.search_query.value.is_empty());
            // 已空：放行冒泡（关窗口逻辑不变）。
            assert!(!note_window.clear_search_on_escape(window, cx));
        })
        .unwrap();
}

#[gpui::test]
fn note_find_toggles_and_matches_content(cx: &mut TestAppContext) {
    let window = cx.add_window(NoteWindow::new_for_test);
    cx.run_until_parked();
    window
        .update(cx, |note_window, window, cx| {
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple("alpha beta alpha", cx));
            assert!(!note_window.find_open);

            // 开面板：查询为空时不产生匹配。
            note_window.toggle_find(window, cx);
            assert!(note_window.find_open);
            assert!(note_window.content_state.read(cx).search_session().open);

            // 敲入查询 -> 引擎算出 2 处匹配。
            note_window.find_query = TextEditingState::new("alpha");
            note_window.apply_find_query(cx);
            assert_eq!(
                note_window
                    .content_state
                    .read(cx)
                    .search_session()
                    .matcher
                    .matched_ranges()
                    .len(),
                2
            );

            // 翻到下一处：选中区间应落在第二个 alpha。
            note_window.find_next(cx);
            let selected = note_window.content_state.read(cx).selected_range();
            assert_eq!(selected, 11..16);

            // 无匹配的查询不产生命中，选区也不应被改动。
            note_window.find_query = TextEditingState::new("zzz");
            note_window.apply_find_query(cx);
            assert!(
                note_window
                    .content_state
                    .read(cx)
                    .search_session()
                    .matcher
                    .matched_ranges()
                    .is_empty()
            );
            note_window.find_next(cx);
            assert_eq!(
                note_window.content_state.read(cx).selected_range(),
                11..16,
                "无匹配时 find_next 不得改写选区"
            );

            // 关面板：匹配会话关闭。
            note_window.close_find(window, cx);
            assert!(!note_window.find_open);
            assert!(!note_window.content_state.read(cx).search_session().open);
        })
        .unwrap();
}

#[gpui::test]
fn note_switching_notes_resets_find(cx: &mut TestAppContext) {
    init_editor_state(cx);
    let window = cx.add_window(NoteWindow::new_for_test);
    cx.run_until_parked();
    window
        .update(cx, |note_window, window, cx| {
            let n1 = sample_note(1, "first note");
            let n2 = sample_note(2, "second note");
            note_window.notes = vec![n1.clone(), n2.clone()];
            note_window.selected_id = Some(n1.id);
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple(n1.content.clone(), cx));
            note_window.toggle_find(window, cx);
            note_window.find_query = TextEditingState::new("note");
            note_window.apply_find_query(cx);

            // 换笔记：查询与匹配必须一并重置，否则高亮落在旧文本的位置上。
            note_window.select_note(n2.id, cx);
            assert!(!note_window.find_open);
            assert!(note_window.find_query.value.is_empty());
            assert!(!note_window.content_state.read(cx).search_session().open);
        })
        .unwrap();
}

#[gpui::test]
fn note_find_closes_when_entering_preview(cx: &mut TestAppContext) {
    init_editor_state(cx);
    let window = cx.add_window(NoteWindow::new_for_test);
    cx.run_until_parked();
    window
        .update(cx, |note_window, window, cx| {
            note_window.toggle_find(window, cx);
            assert!(note_window.find_open);
            note_window.toggle_preview(window, cx);
            assert!(note_window.preview);
            assert!(!note_window.find_open, "预览是只读视图，不该留着查找会话");
        })
        .unwrap();
}

#[gpui::test]
fn note_editor_uses_inner_padding_not_outer_card(cx: &mut TestAppContext) {
    // 回归：编辑区原先在 Textarea 外面套 `p_2` + 边框 + 圆角做"卡片"，
    // 于是分隔线与行号之间空出 8px，且多出一圈与左侧列表冲突的圆角框。
    // 现在外壳是齐边的，呼吸空间由 `set_editor_paddings` 提供。
    let window = cx.add_window(NoteWindow::new_for_test);
    cx.run_until_parked();
    window
        .update(cx, |note_window, _window, cx| {
            let paddings = note_window.content_state.read(cx).editor_paddings();
            assert_eq!(
                paddings.left,
                px(EDITOR_PADDING_X),
                "行号栏左侧应留出与其它编辑器一致的呼吸空间"
            );
            assert_eq!(paddings.top, px(EDITOR_PADDING_Y));
            // 右侧比左侧宽，给滚动条让位，滚动条不会压住文字。
            assert!(
                paddings.right > paddings.left,
                "右侧需额外给滚动条留位，实际 {:?}",
                paddings
            );
        })
        .unwrap();
}

#[test]
fn list_pane_width_clamps_to_bounds() {
    assert_eq!(
        clamp_list_pane_width(LIST_PANE_DEFAULT_WIDTH),
        LIST_PANE_DEFAULT_WIDTH
    );
    assert_eq!(
        clamp_list_pane_width(10.),
        LIST_PANE_MIN_WIDTH,
        "窄窗下不得低于最小宽度，否则搜索框和置顶按钮会被压没"
    );
    assert_eq!(
        clamp_list_pane_width(9999.),
        LIST_PANE_MAX_WIDTH,
        "过宽会挤掉正文区"
    );
}
/// 开一个挂着临时库的窗口，走与生产一致的 `attach_store` 路径。
fn window_with_store(cx: &mut TestAppContext, store: NoteStore) -> gpui::WindowHandle<NoteWindow> {
    let window = cx.add_window(|window, cx| NoteWindow::with_store_for_test(window, cx, store));
    cx.run_until_parked();
    window
}

#[gpui::test]
fn note_save_stores_content_verbatim(cx: &mut TestAppContext) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = NoteStore::open(dir.path().join("note.db")).unwrap();
    let window = window_with_store(cx, store);
    // 首行两空格缩进（Markdown 缩进代码块）与结尾空行都是正文的一部分。
    // 旧实现三处 `value().trim()` 会在每次保存时把它们吃掉。
    let source = "  ```\n代码块\n```  \n\n";
    window
        .update(cx, |note_window, window, cx| {
            note_window.new_note(window, cx);
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple(source, cx));
            note_window.save_current(window, cx);
            let id = note_window.selected_id.expect("保存后应拿到笔记 id");
            let saved = note_window
                .store
                .as_ref()
                .unwrap()
                .get(id)
                .unwrap()
                .unwrap();
            assert_eq!(saved.content, source, "落库必须逐字节保留原文");
            assert!(!note_window.is_dirty(cx), "库里就是原文，保存后不该还判脏");

            // 切走再切回：编辑器读回的仍是同一份原文。
            note_window.new_note(window, cx);
            note_window.select_note_focused(id, window, cx);
            assert_eq!(note_window.content_state.read(cx).value().as_str(), source);
        })
        .unwrap();
}

#[gpui::test]
fn note_dirty_compares_raw_text_not_trimmed(cx: &mut TestAppContext) {
    // 回归（上一轮评审）：库内已是带首尾空白的原文时，编辑器内容与原文字节相同，
    // 不该判脏。旧实现 trim 后比较，这类笔记永远显示“未保存”，按一次保存还吃掉空白。
    let window = cx.add_window(NoteWindow::new_for_test);
    cx.run_until_parked();
    window
        .update(cx, |note_window, window, cx| {
            let note = sample_note(1, "  indented first line\n\n");
            note_window.notes = vec![note.clone()];
            note_window.select_note_focused(note.id, window, cx);
            assert_eq!(
                note_window.content_state.read(cx).value().as_str(),
                note.content
            );
            assert!(!note_window.is_dirty(cx), "原文相同不算脏");
            assert!(note_window.is_save_disabled(cx));

            // 真的多了一个字符才算脏。
            note_window.content_state.update(cx, |s, cx| {
                s.set_value_simple("  indented first line\n\nmore", cx)
            });
            assert!(note_window.is_dirty(cx));
            assert!(!note_window.is_save_disabled(cx));
        })
        .unwrap();
}

#[gpui::test]
fn note_autosave_writes_after_debounce(cx: &mut TestAppContext) {
    use std::time::Duration;

    let dir = tempfile::TempDir::new().unwrap();
    let store = NoteStore::open(dir.path().join("note.db")).unwrap();
    let note = store.create("first").unwrap();
    let window = window_with_store(cx, store);
    window
        .update(cx, |note_window, window, cx| {
            note_window.select_note_focused(note.id, window, cx);
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple("first 自动保存", cx));
            assert!(note_window.is_dirty(cx));
            // `set_value_simple` 不发 `InputEvent::Change`，这里直接调订阅里调度的那个函数。
            note_window.schedule_autosave(cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |note_window, _window, _cx| {
            assert_eq!(
                note_window
                    .store
                    .as_ref()
                    .unwrap()
                    .get(note.id)
                    .unwrap()
                    .unwrap()
                    .content,
                "first",
                "防抖期内不得落库"
            );
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_millis(900));
    cx.run_until_parked();
    window
        .update(cx, |note_window, _window, cx| {
            assert_eq!(
                note_window
                    .store
                    .as_ref()
                    .unwrap()
                    .get(note.id)
                    .unwrap()
                    .unwrap()
                    .content,
                "first 自动保存"
            );
            assert!(!note_window.is_dirty(cx), "落库成功后不该还脏");
            // 不重排列表：就地更新，条目数与选中项保持不变。
            assert_eq!(note_window.selected_id, Some(note.id));
            assert_eq!(note_window.notes.len(), 1);
        })
        .unwrap();
}

#[gpui::test]
fn note_autosave_keeps_only_last_change(cx: &mut TestAppContext) {
    use std::time::Duration;

    let dir = tempfile::TempDir::new().unwrap();
    let store = NoteStore::open(dir.path().join("note.db")).unwrap();
    let note = store.create("first").unwrap();
    let window = window_with_store(cx, store);
    window
        .update(cx, |note_window, window, cx| {
            note_window.select_note_focused(note.id, window, cx);
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple("v1", cx));
            note_window.schedule_autosave(cx);
        })
        .unwrap();
    // 第一次防抖还没到期就继续敲字：上一代作废，只落最后一次。
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    window
        .update(cx, |note_window, _window, cx| {
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple("v2", cx));
            note_window.schedule_autosave(cx);
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    window
        .update(cx, |note_window, _window, _cx| {
            assert_eq!(
                note_window
                    .store
                    .as_ref()
                    .unwrap()
                    .get(note.id)
                    .unwrap()
                    .unwrap()
                    .content,
                "first",
                "上一代作废后不得落库，也不能落中间值"
            );
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    window
        .update(cx, |note_window, _window, _cx| {
            assert_eq!(
                note_window
                    .store
                    .as_ref()
                    .unwrap()
                    .get(note.id)
                    .unwrap()
                    .unwrap()
                    .content,
                "v2"
            );
        })
        .unwrap();
}

#[gpui::test]
fn note_autosave_does_not_write_when_not_dirty(cx: &mut TestAppContext) {
    use std::time::Duration;

    let dir = tempfile::TempDir::new().unwrap();
    let store = NoteStore::open(dir.path().join("note.db")).unwrap();
    let note = store.create("first").unwrap();
    let window = window_with_store(cx, store);
    window
        .update(cx, |note_window, window, cx| {
            note_window.select_note_focused(note.id, window, cx);
            assert!(!note_window.is_dirty(cx));
            // 探针：把库里的行软删掉。任何一次写库都会拿到 `not_found` 并挂上错误码，
            // 所以“没碰库”这件事可以被观察到。
            note_window.store.as_ref().unwrap().delete(note.id).unwrap();
            note_window.schedule_autosave(cx);
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_millis(900));
    cx.run_until_parked();
    window
        .update(cx, |note_window, _window, _cx| {
            assert_eq!(note_window.persist_error_code, None, "不脏时不该尝试落库");
        })
        .unwrap();
}

#[gpui::test]
fn note_delete_can_be_undone(cx: &mut TestAppContext) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = NoteStore::open(dir.path().join("note.db")).unwrap();
    let a = store.create("first").unwrap();
    let _b = store.create("second").unwrap();
    let window = window_with_store(cx, store);
    window
        .update(cx, |note_window, window, cx| {
            note_window.select_note_focused(a.id, window, cx);
            // 两次点击：第一次只标记，第二次真删。
            note_window.delete_current(window, cx);
            assert_eq!(note_window.pending_delete_id, Some(a.id));
            note_window.delete_current(window, cx);
            assert_eq!(
                note_window.pending_restore_id,
                Some(a.id),
                "软删后应留下撤销目标"
            );
            {
                let store = note_window.store.as_ref().unwrap();
                assert!(store.get(a.id).unwrap().is_some(), "软删只打时间戳，行还在");
                assert!(!store.list().unwrap().iter().any(|n| n.id == a.id));
            }

            note_window.undo_delete(a.id, window, cx);
            assert_eq!(note_window.pending_restore_id, None);
            {
                let store = note_window.store.as_ref().unwrap();
                assert!(
                    store.list().unwrap().iter().any(|n| n.id == a.id),
                    "撤销后应回到列表"
                );
            }
            assert_eq!(note_window.selected_id, Some(a.id));
            assert_eq!(note_window.content_state.read(cx).value().as_str(), "first");
        })
        .unwrap();
}

#[gpui::test]
fn note_open_keeps_recent_soft_deletes_for_undo(cx: &mut TestAppContext) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = NoteStore::open(dir.path().join("note.db")).unwrap();
    let note = store.create("first").unwrap();
    store.delete(note.id).unwrap();
    // 打开库时会清理超过保留期的软删行：刚删的必须留着，否则撤销必然失败。
    let window = window_with_store(cx, store);
    window
        .update(cx, |note_window, _window, _cx| {
            let store = note_window.store.as_ref().unwrap();
            assert!(
                store.get(note.id).unwrap().is_some(),
                "保留期内的软删行不能被清理"
            );
            assert!(note_window.notes.is_empty(), "软删行不出现在列表里");
        })
        .unwrap();
}

#[gpui::test]
fn note_close_releases_when_target_deleted_elsewhere(cx: &mut TestAppContext) {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("note.db");
    let store = NoteStore::open(&path).unwrap();
    let note = store.create("first").unwrap();
    // 旁观连接：窗口关掉后拿不到它的 store，靠这条连接读同一份库文件断言草稿落了地。
    let observer = NoteStore::open(&path).unwrap();
    let window = window_with_store(cx, store);
    window
        .update(cx, |note_window, window, cx| {
            note_window.select_note_focused(note.id, window, cx);
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple("draft nowhere to go", cx));
            // 另一个窗口/实例把这篇文章删了：update 从此恒返回 `not_found`。
            note_window.store.as_ref().unwrap().delete(note.id).unwrap();
            note_window.request_close(window, cx);
            // `not_found` 现在回退成 create，草稿有了去处，不再挂错误。
            assert_eq!(note_window.persist_error_code, None);
        })
        .unwrap();
    assert!(
        window.update(cx, |_, _, _| ()).is_err(),
        "笔记已被删除时关窗必须放行，不能把窗口锁成关不掉"
    );
    assert!(
        observer
            .list()
            .unwrap()
            .iter()
            .any(|n| n.content == "draft nowhere to go"),
        "关窗时草稿必须落成新笔记，而不是随窗口一起丢掉"
    );
}

#[gpui::test]
fn note_draft_becomes_new_note_when_target_deleted_elsewhere(cx: &mut TestAppContext) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = NoteStore::open(dir.path().join("note.db")).unwrap();
    let note = store.create("first").unwrap();
    let window = window_with_store(cx, store);
    window
        .update(cx, |note_window, window, cx| {
            note_window.select_note_focused(note.id, window, cx);
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple("draft kept", cx));
            // 目标行被外部删掉：update 恒返回 not_found。若就此失败，
            // select_note / new_note / reload_notes 的脏保护会全部早退，窗口变只读死局。
            note_window.store.as_ref().unwrap().delete(note.id).unwrap();
            note_window.save_current(window, cx);
            assert_eq!(
                note_window.persist_error_code, None,
                "not_found 应回退为 create，而不是挂错误横幅"
            );
            assert!(!note_window.is_dirty(cx), "落库成功后不该还脏");
            let saved: Vec<Note> = note_window
                .store
                .as_ref()
                .unwrap()
                .list()
                .unwrap()
                .into_iter()
                .filter(|n| n.content == "draft kept")
                .collect();
            assert_eq!(saved.len(), 1, "草稿应成为一条新笔记");
            assert_eq!(
                note_window.selected_id,
                Some(saved[0].id),
                "选中应指向新 id，窗口不再卡在原 id 上"
            );
            assert_ne!(note_window.selected_id, Some(note.id));
        })
        .unwrap();
}

#[gpui::test]
fn note_select_unknown_id_keeps_editor_untouched(cx: &mut TestAppContext) {
    let window = cx.add_window(NoteWindow::new_for_test);
    cx.run_until_parked();
    window
        .update(cx, |note_window, _window, cx| {
            let n1 = sample_note(1, "first");
            note_window.notes = vec![n1.clone()];
            note_window.selected_id = Some(n1.id);
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple(n1.content.clone(), cx));
            // id 不在列表里（例如刚被外部删掉的行被点中）：不许把选中挂到一个没有
            // 对应正文的 id 上，否则 is_dirty 退化为“非空即脏”，
            // autosave 会把上一条笔记的正文写进它。
            note_window.select_note(999, cx);
            assert_eq!(note_window.selected_id, None, "找不到就不该选中");
            assert_eq!(
                note_window.content_state.read(cx).value().as_str(),
                "first",
                "编辑器正文不得被改动"
            );
        })
        .unwrap();
}

#[gpui::test]
fn note_undo_restore_keeps_selection_when_filtered_out(cx: &mut TestAppContext) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = NoteStore::open(dir.path().join("note.db")).unwrap();
    let a = store.create("alpha matchable").unwrap();
    let _b = store.create("beta matchable").unwrap();
    // 上一次删除留下的撤销目标：内容不匹配待输入的搜索词。
    let hidden = store.create("gamma hidden").unwrap();
    store.delete(hidden.id).unwrap();

    let (view, cx) =
        cx.add_window_view(|window, cx| NoteWindow::with_store_for_test(window, cx, store));
    cx.run_until_parked();
    view.update_in(cx, |note_window, window, cx| {
        note_window.search_query = TextEditingState::new("matchable");
        note_window.reload_notes(cx);
        note_window.select_note_focused(a.id, window, cx);
        assert_eq!(note_window.selected_id, Some(a.id));

        note_window.undo_delete(hidden.id, window, cx);
        // 恢复的笔记被搜索挡在列表外：选中与正文都不得被抢走。
        assert_eq!(
            note_window.selected_id,
            Some(a.id),
            "撤销不得把用户从正在编辑的笔记上拉走"
        );
        assert_eq!(
            note_window.content_state.read(cx).value().as_str(),
            "alpha matchable"
        );
        // 也不能静默：给一条中性提示说明怎么看回它。
        let notice = note_window
            .list_notice
            .clone()
            .expect("撤销成功但当前视图看不到时必须给提示");
        assert_eq!(notice, expected("note.error.restored_hidden"));
        // 撤销按钮与“可撤销”标识已经消失，提示是用户唯一的反馈。
        assert_eq!(note_window.pending_restore_id, None);
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(
        cx.debug_bounds("note-list-notice").is_some(),
        "提示行必须真的渲染在列表顶部"
    );
}

#[gpui::test]
fn note_autosave_create_keeps_filtered_list_clean(cx: &mut TestAppContext) {
    use std::time::Duration;

    let dir = tempfile::TempDir::new().unwrap();
    let store = NoteStore::open(dir.path().join("note.db")).unwrap();
    store.create("alpha matchable").unwrap();
    let window = window_with_store(cx, store);
    window
        .update(cx, |note_window, window, cx| {
            note_window.search_query = TextEditingState::new("matchable");
            note_window.reload_notes(cx);
            note_window.new_note(window, cx);
            note_window
                .content_state
                .update(cx, |s, cx| s.set_value_simple("gamma hidden", cx));
            // autosave 不会 refresh_list，插入分支就是唯一的自救机会。
            note_window.schedule_autosave(cx);
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_millis(900));
    cx.run_until_parked();
    window
        .update(cx, |note_window, _window, _cx| {
            assert!(
                note_window
                    .store
                    .as_ref()
                    .unwrap()
                    .list()
                    .unwrap()
                    .iter()
                    .any(|n| n.content == "gamma hidden"),
                "首次自动保存必须真的落库"
            );
            assert!(
                note_window
                    .notes
                    .iter()
                    .all(|n| n.content != "gamma hidden"),
                "不匹配搜索词的新笔记不得插进过滤后的列表"
            );
        })
        .unwrap();
}

#[gpui::test]
fn note_recovery_notice_renders_above_list(cx: &mut TestAppContext) {
    cx.update(crossh_editor::init);
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("note.db");
    // 不是可用 SQLite：打开时会备份重建，`recovered_from` 指向备份文件。
    std::fs::write(&path, b"this is definitely not a sqlite database").unwrap();
    let store = NoteStore::open(&path).unwrap();
    let backup = store
        .recovered_from
        .clone()
        .expect("损坏库应被备份重建并给出备份路径");

    let (view, cx) =
        cx.add_window_view(|window, cx| NoteWindow::with_store_for_test(window, cx, store));
    view.update(cx, |note_window, _cx| {
        let notice = note_window
            .recovery_notice
            .clone()
            .expect("洗库后必须提示旧库去了哪");
        // 提示要带上备份路径，否则用户找不到旧数据。
        assert_eq!(
            notice,
            rust_i18n::t!(
                "note.error.recovered_backup",
                path = backup.display().to_string()
            )
            .to_string()
        );
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(
        cx.debug_bounds("note-recovery-notice").is_some(),
        "提示行必须真的渲染在列表顶部"
    );
}

#[gpui::test]
fn note_typing_triggers_autosave(cx: &mut TestAppContext) {
    use std::time::Duration;

    let dir = tempfile::TempDir::new().unwrap();
    let store = NoteStore::open(dir.path().join("note.db")).unwrap();
    let note = store.create("first").unwrap();
    let (view, cx) =
        cx.add_window_view(|window, cx| NoteWindow::with_store_for_test(window, cx, store));
    cx.run_until_parked();
    view.update(cx, |note_window, cx| note_window.select_note(note.id, cx));
    // 真敲键：`TextareaState` 的输入处理会发 `InputEvent::Change`，
    // 订阅里据此调度防抖落库（不经过任何测试专用的调度入口）。
    cx.simulate_input("abc");
    cx.executor().advance_clock(Duration::from_millis(900));
    cx.run_until_parked();
    view.update(cx, |note_window, _cx| {
        let content = note_window
            .store
            .as_ref()
            .unwrap()
            .get(note.id)
            .unwrap()
            .unwrap()
            .content;
        assert_ne!(content, "first", "敲键后防抖到期必须落库");
        assert!(content.contains("abc"), "落库内容应含刚敲的字符：{content}");
    });
}

#[gpui::test]
fn note_reload_after_search_does_not_duplicate_filtered_note(cx: &mut TestAppContext) {
    use std::time::Duration;

    // 回归（终审 P0）：搜索把当前选中的笔记过滤掉后，`reload_notes` 的自动选中
    // 若复用 `select_note`，它的脏保护会看到「`selected_id` 已被清成 None +
    // 编辑器里还留着上一篇正文」而判脏，把上一篇 `create` 成一条重复笔记。
    let dir = tempfile::TempDir::new().unwrap();
    let store = NoteStore::open(dir.path().join("note.db")).unwrap();
    let milk = store.create("买牛奶").unwrap();
    let meeting = store.create("meeting notes").unwrap();

    let window = window_with_store(cx, store);
    window
        .update(cx, |note_window, window, cx| {
            note_window.select_note_focused(milk.id, window, cx);
            assert_eq!(note_window.selected_id, Some(milk.id));
            assert_eq!(
                note_window.content_state.read(cx).value().as_str(),
                "买牛奶"
            );
            // 与 `note_search_reload_is_debounced` 同一入口：防抖到期后才 reload。
            note_window.search_query = TextEditingState::new("meeting");
            note_window.on_search_changed(cx);
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    window
        .update(cx, |note_window, _window, cx| {
            let listed = note_window.store.as_ref().unwrap().list().unwrap();
            assert_eq!(listed.len(), 2, "库里不得多出副本：{listed:?}");
            assert_eq!(
                listed.iter().filter(|n| n.content == "买牛奶").count(),
                1,
                "被搜索过滤掉的笔记不得被 create 成第二条"
            );
            // 列表只剩匹配项，选中跟着落到它身上。
            assert_eq!(note_window.notes.len(), 1);
            assert_eq!(note_window.selected_id, Some(meeting.id));
            assert_eq!(
                note_window.content_state.read(cx).value().as_str(),
                "meeting notes"
            );
        })
        .unwrap();
}

#[test]
fn note_precedes_matches_store_list_order() {
    // 必须与 `crates/crossh-note/src/lib.rs` 里 `NoteStore::list` 的
    // `ORDER BY pinned DESC, updated_at DESC, id DESC` 保持一致：
    // 就地插入的新笔记靠它找位置，排序规则一漂移，列表顺序就会跳。
    let pinned = Note {
        pinned: true,
        ..sample_note(1, "pinned")
    };
    let plain = Note {
        updated_at: 9_999,
        ..sample_note(2, "plain")
    };
    // 1) 置顶优先，与时间、id 都无关。
    assert!(note_precedes(&pinned, &plain));
    assert!(!note_precedes(&plain, &pinned));

    // 2) 同为未置顶：updated_at 大的在前。
    let newer = Note {
        updated_at: 200,
        ..sample_note(3, "newer")
    };
    let older = Note {
        updated_at: 100,
        ..sample_note(4, "older")
    };
    assert!(note_precedes(&newer, &older));
    assert!(!note_precedes(&older, &newer));

    // 3) 时间相同时：id 大的在前。
    let high_id = Note {
        updated_at: 100,
        ..sample_note(7, "high id")
    };
    let low_id = Note {
        updated_at: 100,
        ..sample_note(6, "low id")
    };
    assert!(note_precedes(&high_id, &low_id));
    assert!(!note_precedes(&low_id, &high_id));

    // 同一行不比它自己优先：否则就地插入找位置时会把新行插到自己后面。
    assert!(!note_precedes(&older, &older));
}
