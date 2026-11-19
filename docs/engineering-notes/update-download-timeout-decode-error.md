# 自更新失败：error decoding response body

## 症状

- 设置 → Updates 显示 `Update failed: update request failed: error decoding response body`。
- 检查更新或下载安装包时都可能出现；网络越慢越容易出现，重试后有时又能成功。

## 根因

`crates/crossh-update/src/client.rs` 之前让 manifest 与安装包共用同一个 reqwest client：

```rust
reqwest::Client::builder()
    .timeout(Duration::from_secs(30))
```

`.timeout` 是**整个请求**的总时限，包含读响应 body。macOS 安装包约 14–15 MB，链路低于约 3.7–4.0 Mbps 时无法在 30 秒内读完。超时后 reqwest 的 `Response::chunk()` 会把 body 层错误再包成 `Kind::Decode`，它的 `Display` 正是 `error decoding response body`；真正的原因在 source 链里：

```text
update request failed: error decoding response body
  -> request or response body error
  -> operation timed out
```

`UpdateError::Request` 只显示顶层消息，所以界面看到的是误导性的“解码失败”。这跟 `stable.json` 损坏无关；JSON/签名错误会显示 `update manifest is invalid: ...`。

## 持久规则

- 大文件下载不要设置总超时；用 `connect_timeout` 限制建连，用 `read_timeout` 限制两次数据之间的最长间隔。
- 小 manifest 可以保留总超时，但传输类错误要按通道重试。
- 面向用户的更新错误要展开 source 链（`UpdateError::describe()`），否则超时会被误报为解码错误。
- GUI 从 Finder/Dock 启动不带 shell 的 `HTTPS_PROXY`；reqwest 需要启用 `system-proxy` 才能读取 macOS 系统代理设置。

## 验证

- 用一个“先回 Content-Length、再停顿”的本地服务器 + 200 ms 总超时复现：顶层显示 `error decoding response body`，source 链包含 `operation timed out`。
- 修复后通过真实加速源下载 v0.45.2 macOS 包（14 MB），大小与 SHA-256 校验通过。
- `cargo test -p crossh-update`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --check` 通过。

## 关键词

`error decoding response body`, `update request failed`, `Update failed`, `自更新`, `更新失败`, `下载超时`, `30 秒`, `reqwest`, `read_timeout`, `connect_timeout`, `system-proxy`, `stable.json`, `gh-proxy`
