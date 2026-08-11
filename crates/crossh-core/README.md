# crossh-core

职责：提供与 UI 无关的领域模型和本地系统协议，包括 SSH 配置、终端 shell、Git、命令历史和连接状态。

边界：

- 不依赖 GPUI，也不负责 SSH 网络传输或视图状态。
- 公共数据和纯逻辑可被应用层与后台 transport 复用。

公开入口：`config::{SshConfig, HostConfig}`、`terminal`、`commands`、`git`、`ConnectionState`。

快速验证：`cargo test -p crossh-core`
