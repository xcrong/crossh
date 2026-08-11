# crossh-ssh

职责：封装 russh 运行时、连接池、认证、ProxyJump、SFTP、端口转发和远程命令通道。

边界：

- 依赖 `crossh-core` 的配置和状态，不依赖 GPUI。
- 网络、通道和后台任务留在本 crate；视图只消费公开事件与句柄。

公开入口：`ConnectionHandle`、`ConnEvent`、`AuthChoice`、`SftpCmd`、`SftpEvent`、`ssh_runtime`。

快速验证：`cargo test -p crossh-ssh`
