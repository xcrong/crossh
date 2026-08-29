# crossh-core

职责：提供与 UI 无关的领域模型和本地系统协议，包括终端 shell、Git、命令历史和连接状态。

边界：

- 不依赖 GPUI，也不负责视图状态。
- 公共数据和纯逻辑可被应用层复用。

公开入口：`terminal`、`commands`、`git`、`ConnectionState`、`format`、`system_stats`。

快速验证：`cargo test -p crossh-core`
