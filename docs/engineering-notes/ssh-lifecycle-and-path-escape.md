# SSH 连接生命周期与路径逃逸

## 症状

1. 远程 `sleep 300`（或长 SFTP/转发）运行期间，App 释放连接句柄后 CPU 空转 100%。
2. 关闭全部终端/转发标签后，服务器端 SSH 会话仍长期占用（`who` 里残留连接）。
3. agent 的 `write` 工具可向工作区外写文件。
4. 服务器重装后主机密钥变更，UI 的「接受」按钮无效。

## 根因与规则

### 1. `tokio::select!` 中已关闭 channel 分支恒就绪 → 忙轮询
`command_rx.recv()` 返回 `Err`（连接句柄已 drop）时，`Err(_)` 分支若无 await
就立即进入下一轮 `select!`。只要还有一条活跃 channel（远程命令/SFTP/转发
计数 > 0），select 永不挂起，单核 100% 空转直到该 channel 结束。
**规则**：cmd 通道关闭后必须转入纯 `ended_rx` 等待循环（`while let Some(ended)
= ended_rx.recv().await`），而不是继续 select 空转。

### 2. 连接池持有强引用 → 会话永不释放
`ConnectionManager::connections` 若存 `Entity<Connection>` 强引用，App 退出前
连接永不 Drop，`Connection::drop → handle.shutdown()` 不会触发。SSH 会话、
worker 线程、TCP 连接全部残活。
**规则**：池只存 `WeakEntity`；连接生命周期与使用方（标签/转发面板/后台命令
持有的强引用）一致——最后一个使用者释放即断开。注意后台命令的
`remote_background_controls` 也持强引用，远程任务运行期间连接必须保持。

### 3. `path.exists()` 对悬空符号链接返回 false → 允许写入并跟随链接逃逸
`workspace_path(..., allow_missing=true)` 只校验「最近存在的祖先」在工作区内，
最终组件若是悬空链接则按普通缺失路径放行；随后 `fs::write` 跟随链接在工作区
外创建文件。目录中间的符号链接同理（`exists()` 跟随解析后祖先链可能仍显示在
工作区内）。
**规则**：`allow_missing` 路径按组件队列逐跳解析符号链接（每跳校验目标落在
工作区内，悬空目标按词法规范化后同样校验；跳数上限 40 防环），而不是只检查
「组件本身是链接」的浅层形态——多跳链（`hop1 → hop2 → 工作区外`）的第一跳
落在工作区内并不代表链条终点安全。

### 4. 主机密钥变更的确认决定被丢弃
`check_known_hosts` 返回 `Err(KeyChanged)` 时旧代码只 `ask_host_key` 后无条件
返回 `Ok(false)`，UI 的「接受本次连接」是无效按钮。
**规则**：变更密钥允许 `AcceptOnce`（本次会话信任，绝不写入 known_hosts——
那会把 MITM 密钥固化）；`AcceptAlways` 在变更路径等同拒绝，UI 也不展示该
按钮。

## 验证

- `cargo test -p crossh-ssh -p crossh-agent -p crossh-core`：
  忙轮询无回归；`write_rejects_dangling_symlink_escape` 等 6 条符号链接用例
  （含 `write_rejects_dangling_two_hop_symlink_chain` 与
  `write_rejects_two_hop_symlink_chain_to_existing_outside_dir`）；
  悬空/越界/多跳链拒绝、内部链接正常写入。
- `cargo test --bin crossh`：`replaced_prompt_explicitly_rejects_the_previous_request`
  证明旧 prompt 被覆盖时显式 Reject 而非静默丢弃。
- 忙轮询本身无单元测试（需要真实 SSH 会话复现），以代码审查结论为准。

## 关键词

`SSH`, `russh`, `busy loop`, `select!`, `CPU 100%`, `connection pool`, `WeakEntity`,
`symlink`, `dangling`, `allow_missing`, `write tool`, `host key changed`,
`known_hosts`, `AcceptOnce`