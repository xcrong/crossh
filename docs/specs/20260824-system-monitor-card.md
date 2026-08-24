# 系统状态监视器（状态栏浮动卡片）

> 复制本文件到 `docs/specs/YYYYMMDD-<slug>.md`，填写后进入评审。
> 只描述行为与验收，不写实现方案。语言与项目文档保持一致。

## 元数据

- 状态：`in-progress`
- 创建：2026-08-24
- 相关 ADR：docs/adr/0002-logic-ui-layering.md（逻辑零 gpui、视图依赖逻辑）；ADR 0010 为同类分层先例。不新增边界决策
- 相关 issue / 路线图项：无
- CI 平台影响：`macOS + Linux + Windows`（新增跨平台采样依赖；渲染布局三平台一致，采样可用性差异按契约 6 降级）

## 背景

应用底部状态栏目前只提供开关类入口（设置、侧栏、时间戳、compose、快捷命令），用户无法在不离开终端的情况下了解本机资源状况。参照常见终端工作台的「系统监视器」小部件，在状态栏右侧增加一个入口按钮，弹出一张浮动小卡片展示 CPU / Memory / Disk / Network 四组指标，即可满足"顺手看一眼"的诉求。

用户已确认两个范围决策：

1. 面板形态为**浮动小卡片**（非停靠面板，不改变终端可用宽度）。
2. 监控范围**仅本机**；远程 SSH 会话的系统状态不在本期范围。

## 目标

1. 状态栏右侧新增一个 toggle 按钮，点击在状态栏上方弹出/收起一张系统监视浮动卡片。
2. 卡片固定展示四组指标：CPU（总占用率 + 负载）、Memory（已用/总量 + 占用率）、Disk（主磁盘已用/可用）、Network（下行/上行速率）。
3. 卡片可见时按固定周期刷新，隐藏后采样完全停止，无后台任务残留。
4. 采样逻辑为纯逻辑模块，零 `gpui` 依赖，可被三平台 CI 测试覆盖。

## 非目标

- 不做停靠面板、宽度拖拽、`available_main_width` 联动（与 quick_commands 不同）。
- 不做远程 SSH 会话的系统采样。
- 不做历史曲线图、每核 CPU 明细、进程列表、电池/电源、磁盘 I/O 速率。
- 不做设置项：刷新周期固定，卡片显隐不持久化。
- 不新增 `crossh-ui-component` 通用组件；卡片为 workspace feature 私有视图（仅一个消费者，不抽象）。

## 行为契约

1. 当用户点击状态栏右侧的系统监视按钮时，应该切换卡片的显示/隐藏，观察到按钮的 `selected` 视觉状态与卡片可见性一致。
2. 当卡片可见时，应该以浮层形式渲染在状态栏上方、靠右对齐（右缘与状态栏右缘保留小边距），观察到卡片为 absolute 定位、不挤压/不推移终端、侧栏与快捷命令栏的布局，终端可用宽度不变。
3. 当卡片可见时，应该展示四组指标：CPU 总占用率与系统负载、Memory 已用/总量与占用率、主磁盘已用/可用容量、Network 下行/上行速率，观察到每组数值均为当前本机采样值且带可读单位（百分比、GB、MB/s）。
4. 当给定两次采样输入（前值与当前值）时，速率类字段应该按增量差值计算并随输入变化，观察到纯逻辑函数对相同输入产出确定结果（注入快照 + 时间推进，不依赖真实系统负载）。
5. 当卡片被隐藏（按钮切换或窗口关闭）时，应该停止周期采样并取消后台任务，观察到隐藏后再推进一个刷新周期，采样快照代数不再递增，过期任务写入被拒绝。
   （真实负载下的刷新观察移入验收清单「用户可观察效果人工确认」：卡片可见时按固定周期（2 秒）刷新，`yes > /dev/null` 使 CPU 占用明显上升。）
6. 当采样失败或某项数据在当前平台不可用时，应该在对应位置显示占位符（`--`），观察到应用不 panic、卡片其余部分正常渲染。
7. 当应用重启时，应该默认隐藏卡片，观察到显隐状态不写入 `settings.toml`、不随标签/会话切换改变（应用级状态）。
8. 当窗口收缩到最小尺寸时，卡片应该保持完整可见，观察到卡片不超出视口右缘、无裁切与重叠。
9. 当 `crossh-core` 新增采样模块时，应该保持零 `gpui` 依赖，观察到 `scripts/check-architecture.sh` 通过且采样快照结构可被纯逻辑单测覆盖（单位换算、速率增量计算、不可用占位）。

## 边界与错误

- 采样任务生命周期：首次打开卡片时启动，隐藏/退出时取消；重复快速点击按钮不得叠加多个采样任务。
- CPU 占用率需要两次采样间隔才能计算；首次打开的第一帧允许显示占位符，不得显示错误的 0% 或阻塞渲染。
- 网络速率为两次采样间的增量差值；本次采样值小于上次时视为计数器回绕（重连/计数重置），该帧显示 `--`，不得出现负数。
- 多磁盘机器固定取系统盘（macOS 根卷 `/`、Windows 系统盘、Linux `/`），不逐盘列出；系统盘不可用时显示 `--`。
- 卡片打开期间窗口失焦、最小化不导致采样堆积；恢复后继续正常刷新。
- 应用退出时卡片与采样任务随窗口销毁，无 panic 与资源泄漏日志。

## 接口与状态变更

- 新增依赖：`sysinfo` `0.37`（与 Zed 依赖树中已有的 0.37.x 传递副本对齐，避免引入第三份拷贝；CPU/Memory/Disk/Network 三平台支持，`load_average` 平台差异按契约 6 降级），加入根 `Cargo.toml` 并锁定 `Cargo.lock`。
- `crates/crossh-core` 新增系统采样模块：导出系统快照结构（CPU 占用、负载、内存、主磁盘、网络速率）与采样接口，无 UI 依赖。
- `AppShell` 新增应用级字段：卡片可见性 + 最新采样快照；新增 toggle 方法。
- 图标：新增 Lucide `Activity`（`crates/crossh-assets/assets/icons/activity.svg`，源 `https://raw.githubusercontent.com/lucide-icons/lucide/1.27.0/icons/activity.svg`），`IconName` 映射与 `THIRD_PARTY_NOTICES.md` 同步。
- i18n：`locales/en.yml` / `locales/zh-CN.yml` 新增按钮 tooltip 与卡片内标签文案。
- 无设置项、无持久化格式变更。

## 平台影响

- 渲染布局三平台一致（GPUI 绘制）；采样可用性随平台/版本有差异（如部分平台无 `load_average`），一律按契约 6 以 `--` 占位降级。本机只验证 macOS arm64。
- Linux（X11/Wayland）与 Windows 的卡片渲染与采样由现有 GitHub Actions workspace 测试 job 覆盖；契约 9 的纯逻辑单测在三平台运行。
- 本机无法验证的部分：非 macOS 平台的采样可用性降级路径（契约 6 的真实触发）由 CI 平台运行时人工确认，本地仅以单测覆盖占位逻辑。

## 涉及纪律

- [x] Logic must not depend on UI（层级）——采样模块放 `crossh-core`，零 `gpui` import。
- [x] Feature-owned settings——本期无设置项；状态为 `AppShell` 应用级字段，不进集中设置。
- [x] 图标纪律（Lucide 1.27.0 官方 SVG，IconName 映射）——下载原文件，不改写 path。
- [x] 文件规模 < 2000 行（scripts/check-architecture.sh）——卡片渲染独立新文件，不改大现有大文件。
- [ ] 工程笔记 / ADR 同步义务——无结构性决策，预期不触发；若调试中发现非显然根因再补笔记。
- [x] 响应式 UI（最小窗口尺寸可用性）——契约 8 覆盖最小窗口。

## 影响模块

- `crates/crossh-core/src/`（新增采样模块 + 单测）
- `Cargo.toml`（新增 `sysinfo` 依赖）
- `src/features/workspace/`（新增卡片渲染文件；`shell.rs` / `shell_render.rs` 挂载浮层；`view.rs` 状态栏按钮）
- `crates/crossh-assets/`（`activity.svg` + `IconName` 映射）、`THIRD_PARTY_NOTICES.md`
- `locales/en.yml` / `locales/zh-CN.yml`
- `docs/testing.md`（新增 Monitor 行为矩阵行）

## 验收清单

- [ ] spec 评审通过（AI 评审 + 人批准）
- [ ] 行为契约全部固化为失败测试并确认失败原因正确（Red）
- [ ] 最小实现通过聚焦测试（Green）
- [ ] `cargo fmt --check`
- [ ] `scripts/check-architecture.sh`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] 声明的平台 CI job 通过（非本机平台：提交后由 Actions 验证，spec 状态
      保持 in-progress 直到通过）
- [ ] 结构性决策提炼进 ADR（如有）并登记 docs/architecture.md
- [ ] 调试根因合并进 docs/engineering-notes/（如有）
- [ ] 新增行为合并进 docs/testing.md 关键行为矩阵（如有）
- [ ] 用户可观察效果人工确认（针对 UI/交互变更）
