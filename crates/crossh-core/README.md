# crossh-core

职责：提供与 UI 无关的领域模型和本地系统协议，包括终端 shell、Git、命令历史和连接状态。

边界：

- 不依赖 GPUI，也不负责视图状态。
- 公共数据和纯逻辑可被应用层复用。

公开入口：`terminal`、`commands`、`git`、`ConnectionState`、`format`、`system_stats`、`text_editing`、`input_handler`、`i18n`，以及 `git_launcher`、`note_launcher`、`editor_launcher`。

信任边界：`editor_launcher::editor_command` 可指向任意二进制，属于本地用户自身信任（仅来自本机设置文件），不经过 shell 解析故无 shell 注入；每次启动都会把完整命令记到 `log::info!`（见 `/tmp/crossh/run.log`），首次使用新命令请先核对日志/设置页展示的完整命令。

快速验证：`cargo test -p crossh-core`
