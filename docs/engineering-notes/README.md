# 工程经验索引

这里记录已经确认、可复用的调试经验，目的是让相似问题能够通过症状或关键词快速定位。架构约束和长期决策仍然记录在 `docs/adr/`；这里不重复 ADR，也不记录未经验证的猜测。

使用方式：先搜索下表中的症状和关键词，只读取匹配的主题文档。解决新的非显然问题后，优先更新已有主题；只有当问题属于新的技术边界时才新增文件。

| 主题 | 典型症状 | 关键词 | 文档 |
| --- | --- | --- | --- |
| GPUI 首窗口与 CLI 生命周期 | Dock 有图标但没有窗口；GUI 命令阻塞终端；关闭隐藏窗口后命令才退出 | `GPUI`, `open_window`, `defer`, `Dock`, `CLI`, `detached process`, `cold start` | [GPUI 窗口启动生命周期](gpui-window-startup.md) |
| GPUI Flex 滚动容器 | 设置 `max_h` 和 `overflow_y_scroll` 后滚轮仍无效；长列表被压缩 | `GPUI`, `flex_shrink_0`, `overflow_y_scroll`, `max_h`, 滚轮无效 | [GPUI Flex 滚动容器](gpui-flex-scroll.md) |
| GPUI 轮询与大列表性能 | Git 视图静止时周期性卡顿；多终端持续启动 Git；长 diff 滚动变慢 | `GPUI`, `Git`, `polling`, `notify`, `UniformList`, `diff`, 卡顿 | [GPUI 轮询与大列表性能](gpui-polling-performance.md) |
| Git NUL 协议路径解析 | 含连续空格或 Tab 的文件无法暂存；重命名文件的变更计数为零 | `Git`, `porcelain v2`, `numstat`, `-z`, `NUL`, 文件名空格 | [Git NUL 协议路径](git-nul-paths.md) |
| SSH 连接生命周期与路径逃逸 | 释放连接后 CPU 100%；关标签后服务器会话残留；agent write 写到工作区外；主机密钥变更后接受按钮无效 | `SSH`, `busy loop`, `select!`, `WeakEntity`, `symlink`, `allow_missing`, `host key changed`, `known_hosts` | [SSH 连接生命周期与路径逃逸](ssh-lifecycle-and-path-escape.md) |
| Cargo 发布后 workspace 版本与锁文件不一致；tag 发布漏提交 `Cargo.lock` | `Cargo.lock`, `release.sh`, `cargo metadata`, 版本发布 | [Cargo 锁文件发布同步](cargo-lock-release-sync.md) |
