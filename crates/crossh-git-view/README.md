# crossh-git-view

职责：GPUI Git Viewer 功能：会话状态、窗口、输入、键位，以及 Changes / History / Branches / Stashes / 冲突解决的渲染。

边界：

- 只被 `src/bin/crossh-git.rs` 消费；主 `crossh` 二进制通过 sibling 进程使用 Git，不编译本 crate 的视图。
- Git 协议解析与仓库操作归 `crossh-core`；本 crate 只拥有视图状态与渲染。

公开入口：`init`、`open_git_window`。

快速验证：`cargo test -p crossh-git-view`
