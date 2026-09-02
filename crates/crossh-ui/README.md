# crossh-ui

职责：提供共享的 GPUI 资源源、图标、主题、上下文菜单和输入/tooltip widgets。

边界：

- 依赖 `crossh-assets`（嵌入图标）和 Zed GPUI，内置 `palette` 调色板；不负责 feature 状态、SSH、AI 或更新逻辑。
- feature view 通过这些 primitives 组合界面，业务状态仍归 feature 所有。

公开入口：`assets`、`context_menu`、`icons`、`theme`、`widgets` 模块，以及 `icons::{icon, logo}`。

快速验证：`cargo check -p crossh-ui --all-targets`
