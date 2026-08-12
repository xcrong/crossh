# GPUI 轮询与大列表性能

## 症状

Git 视图静止时仍周期性卡顿；打开多个本地终端后 CPU 与 `git` 子进程持续增加；长 diff 滚动时输入和窗口拖动响应变慢。

## 根因

轮询若每次都写回相同状态并调用 `notify`，会强制整棵 GPUI 视图重新渲染。Git 窗口此前会在每次扫描后重新读取、解析并逐行构造当前 diff；工作区曾每秒为所有本地会话启动一次 `git status`，慢仓库会让这些请求重叠。常规 flex 容器也会在每一帧创建全部 diff 行。

## 持久规则

后台轮询必须合并在途请求，只在可见状态实际变化时 `notify`。同一份 Git porcelain 输出应同时提供变更列表和分支状态，避免重复进程。手动刷新可以强制重读 diff；周期刷新只在当前选中条目的元数据变化时重读。长且固定行高的内容使用 `UniformList`，并开启 `Unconstrained` 水平尺寸以保留横向滚动；不能为每一行做同步文本测量。

## 验证

用单元测试验证刷新状态机合并重复请求、未变化扫描不触发 diff 重读、合并扫描仍包含分支信息。检查 Git diff 视图使用 `UniformList`，并以长行回归测试确认横向滚动范围存在。运行 `cargo test --bin crossh`、`cargo clippy --bin crossh --all-targets -- -D warnings` 和 `scripts/check-architecture.sh`。

关键词：`GPUI`、`Git`、`polling`、`notify`、`UniformList`、`diff`、卡顿、性能、横向滚动
