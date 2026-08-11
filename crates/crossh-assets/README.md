# crossh-assets

职责：嵌入并按稳定路径提供 Crossh 图标和应用 logo。

边界：

- 只负责 asset lookup，不依赖 GPUI，也不允许业务视图直接读取图标文件路径。
- 图标 SVG 必须来自仓库约定的 pinned Lucide release。

公开入口：`load`、`LOGO_PATH`、`IconName`、`IconName::ALL`、`IconName::path`。

快速验证：`cargo test -p crossh-assets`
