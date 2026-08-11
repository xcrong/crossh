# crossh-theme

职责：提供 UI 与终端共享的无 UI 颜色 token 和 RGB 表示。

边界：

- 保持纯数据和纯函数，不依赖 GPUI；renderer 负责转换为具体颜色类型。
- 颜色定义集中在本 crate，避免各个 view 自行漂移。

公开入口：`Rgb` 及 `canvas`、`surface`、`text`、`accent`、`warning`、`danger` 等 palette 函数。

快速验证：`cargo test -p crossh-theme`
