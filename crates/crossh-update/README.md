# crossh-update

职责：负责 release manifest、下载校验、归档解包和独立 updater 的进程替换。

边界：

- 不依赖 GPUI；网络、归档和安装逻辑留在 crate 内，UI 只消费候选版本和错误。
- `crossh-updater` 二进制通过 `run_from_args` 使用同一套安装实现。

公开入口：`fetch_manifest`、`download_artifact`、`UpdateManifest`、`UpdateCandidate`、`spawn_updater`、`run_from_args`。

快速验证：`cargo test -p crossh-update`
