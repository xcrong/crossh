# Note 测试不该打开用户真实笔记库

## 症状

`crossh-note-view` 的测试随机失败，报错内容看起来毫不相干：

```
---- window::tests::note_window_historic_note_loads_in_edit_state stdout ----
thread '...' panicked at crates/crossh-note-view/src/window.rs:1395:17:
assertion `left == right` failed
  left: Some(7)
 right: Some(1)
```

该测试手动把 `note_window.notes = vec![Note { id: 1, .. }]` 设成一条笔记，然后断言
`selected_id == Some(1)`。但断言拿到的却是 `Some(7)` —— 真实库里某条笔记的 id。

单独跑通过，连跑几次偶尔失败；增加测试数量后失败频率明显上升。

## 根因

所有测试都用 `cx.add_window(NoteWindow::new)` 构造窗口，而 `NoteWindow::new` 会调
`NoteStore::open_default()`，打开并写入

```
~/Library/Application Support/crossh/note.db
```

后果有两个：

1. **测试互相干扰** —— Cargo 默认并行跑同一二进制里的测试，多个窗口并发读写同一个
   SQLite 文件。某个测试的 `reload_notes` 会把另一个测试刚塞进 `notes` 的数据顶掉。
2. **测试污染用户数据** —— `cargo test` 真的会往用户真实笔记库写测试笔记。

测试的原有写法（手动覆盖 `notes`）是在跟这个行为对抗：先被真实库填充，再被手动覆盖，
中间存在竞态窗口。

## 规则

- GPUI 视图测试里，只要被测逻辑不依赖持久化，就用 `new_for_test` 之类的入口
  构造窗口，**不打开真实 store**。`crossh-note-view` 的做法是把 `new` 拆成
  `new_uncoupled`（只做 UI 状态）+ `new`（额外挂 store 并 reload），
  测试走前者。
- 确实需要 store 的测试（如防抖重查）用 `tempfile::TempDir` + `NoteStore::open`
  开临时库，并在断言里去掉 `if store.is_some()` 这类"有库才断言"的软化分支
  —— 软化分支正是让这个测试一直用真实库的原因。
- 判断标准：**测试失败信息与被测逻辑无关时，先怀疑共享外部状态**（真实用户目录、
  真实网络、固定端口、固定文件路径）。
- 判断是否踩到这条的方法：`cargo test -p <crate> <单个测试名>` 通过，
  但整套跑偶发失败，就是并行 + 共享状态的典型特征。

## 验证

```bash
cargo test -p crossh-note-view
```

修复后连跑 5 次全绿，且测试耗时从 0.10s 降到 0.01s（不再开 SQLite）。

## 关键词

`NoteStore::open_default`, `note.db`, `add_window`, `TestAppContext`,
`cargo test` 并行, 测试污染用户数据, `tempfile`, `TempDir`, `Some(7)`,
`selected_id`, `reload_notes`, `flaky`, `note_view`, `跨测试污染`
