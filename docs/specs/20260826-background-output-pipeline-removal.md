# Spec：后台任务输出捕获管道移除

## 元数据

- 编号：20260826-background-output-pipeline-removal
- 来源：2026-08-26 简化审计 N-17（`BackgroundTaskEvent.{output, exit_code}` 全程零读，`apply_event` 显式 `drop`）
- 状态：**done**（approved 人在环：用户于 2026-08-26 简化扫描轮中在"删除整条管道 / 接线展示 / 暂不处理"三选项中明确选择删除；本 spec 为该决策的记录与实现契约）

## 意图

后台任务（本地与远端）的输出捕获管道没有任何读者：`BackgroundTaskManager::apply_event` 收到事件后立即丢弃 `output` 与 `exit_code`，唯一外部消费者（workspace shell）只读 `id`/`status` 打日志。每条本地后台命令为此白付 2 个采集线程 + 一把 `Arc<Mutex<String>>`；远端路径同样全程累积后丢弃。删除整条管道。

## 非目标

- 不新增"查看后台任务输出"功能（若未来需要，按新 spec 重新设计流式展示，不复用本管道）。
- 不改动任务面板的状态语义（Running/Stopping/Succeeded/Failed/Terminated 不变）。
- 不改动 `start_remote` 的远端命令执行、停止信号与退出状态推导。

## 行为契约

1. `BackgroundTaskEvent` 收缩为 `{ id, status }`；`RemoteCommandEvent` 同步收缩。构造点（本地 spawn、远端 run_remote_command、连接不可用回退、shell 转发、全部测试）随之更新。
2. 本地后台命令的 stdout/stderr 改为 `Stdio::null()`：不派生采集线程，子进程输出不捕获。前提不变量：stdin 保持 null（后台任务不与终端抢输入）不变。
3. 远端命令循环继续消费 channel 消息（Data/ExtendedData 丢弃、ExitStatus/Close 照常处理）直到关闭——只停止累积，不提前 break。
4. 级联删除：本地 `spawn_output_reader`/`append_output`/`MAX_OUTPUT_BYTES`、远端 `append_remote_output`/`MAX_REMOTE_COMMAND_OUTPUT` 及其 UTF-8 边界测试、`crossh_core::format::truncate_to_limit`（两个生产消费者全灭后自身仅剩测试消费）及其测试。
5. 测试契约收窄：`background_manager_runs_a_command_and_reports_output` 改为断言退出状态与任务清理（不再断言输出内容），更名 `background_manager_runs_a_command_and_reports_status`。

## 平台影响

- macOS arm64：本地验证覆盖。
- Windows/Linux：`shell_command` 的 windows 分支同步改 `Stdio::null()`，逻辑对称；由 GitHub Actions 对应 job 验证。

## 验收清单

- [x] `cargo clippy --workspace --all-targets` 零警告（无 unused import 残留）。
- [x] `cargo test --workspace` 全绿（硬门禁）：550 passed / 0 failed。
- [x] 全仓 grep 确认 `output`/`exit_code` 字段与 `MAX_OUTPUT_BYTES`/`truncate_to_limit` 等级联符号零残留。
- [x] `scripts/check-architecture.sh` 通过。
