# Proposal

## Why

链接预览是 v0.42.0 里唯一一个平台绑定的用户可见能力：能力本身在 macOS 已经稳定（悬浮卡片、地址栏、`Esc`、http(s) 白名单、状态栏入口），但 Linux / Windows 用户点同一个终端链接只能被交回系统浏览器——同一次点击在不同平台有两套行为。

而这种分化是**代码结构**造成的，不是所有平台都真的做不到：

- `Cargo.toml:66-68` 把 `wry` 限在 `cfg(target_os = "macos")`；
- `shell/mod.rs:54` 的模块声明、`:211-214` 的两个字段、`:399` 的 Esc 处理、`:1280` 的 `LinkOpener` 安装，加 `view.rs:993` 的状态栏入口，一共有 8 处手工 `#[cfg(target_os = "macos")]`；
- 但 wry 在 Windows 上接受**任意** `RawWindowHandle::Win32`（`wry-0.57.1/src/webview2/mod.rs:101-112`，`new_in_hwnd(..., is_child = true)` 自建 child HWND），代码基本可以复用；
- **Linux 现在不接，但原因是成本而不是能力**：wry 在 Linux 上确实能嵌（X11 窗口句柄就行），代价是要开 `os-webview` + `x11`，于是拉进 GTK / WebKitGTK、要求先 `gtk::init()` 并把 GTK 主循环塞进 GPUI 事件循环；Wayland 则只能走 `build_gtk(gtk::Fixed)`，GPUI 没有 GTK widget 可用。本变更不引这些依赖（v0.42.0 承诺 Linux 打包依赖不变），链接继续交系统浏览器。

也就是说：Windows 缺的是接线，Linux 是「能接但代价超出本变更」。把这两件事混在一个「平台差异」注释里，会让后来者每次加平台都得重新推导一遍。

## What Changes

- **`link-preview` 变成平台无关能力**：先定义一份统一的可观察契约（入口形态、卡片生命周期、`Esc`、协议白名单、状态栏入口、回退行为），再按平台各自满足。
- **Windows 接入应用内预览**：wry 依赖从 `cfg(target_os = "macos")` 扩到 `cfg(any(target_os = "macos", windows))`，复用现有卡片、地址栏、IPC `Esc`、页面加载回写、`set_bounds` 逻辑；只补 Win32 侧差异（焦点归属、DPI 换算、原生子窗口与 GPUI 的层级）。
- **Linux 明确为回退**：不引入 GTK/WebKitGTK（保住 v0.42.0「Linux 打包依赖不变」的承诺），链接继续交系统浏览器，状态栏入口不显示；Wayland 在任何情况下都只能回退。
- **平台判定收敛成单一来源**：把 8 处散落的 macOS `cfg` 收成一处判定，加平台时不再逐处 grep。
- **验证通道写清楚**：macOS 本机验证；Windows 只由 GitHub Actions 编译 + 人工冒烟（本机不跑跨目标构建，符合 `AGENTS.md` 的本地责任边界）。
- **BREAKING**：无。macOS 行为不变；Linux 行为不变；Windows 从「系统浏览器」变成「应用内卡片」，属能力新增。

## Capabilities

### New Capabilities

- `link-preview`: 链接预览的平台契约——支持的平台上给出应用内卡片预览，不支持的平台回退系统浏览器且不出现无效入口；包含入口形态、卡片生命周期、协议白名单、`Esc` 关闭、浮层避让，以及各平台可观察的差异声明。

### Modified Capabilities

无（`openspec/specs/` 目前只有 `note-search`）。

## Impact

- `Cargo.toml`：`wry` 的 target 门控从 macOS 扩到 macOS + Windows；Linux 仍是零新增依赖。
- `src/features/workspace/shell/link_preview.rs`：平台判定、Win32 焦点/DPI 差异；`shell/mod.rs`（8 处 `cfg`、字段与初始化）、`src/features/workspace/view.rs:646/650/993`（入口与浮层避让）随之解绑。
- `src/features/workspace/shell/mod.rs:1280` 的 `LinkOpener` 安装条件放宽到「预览可用平台」，Linux 保持不安装 → 终端链接继续走系统浏览器。
- `.github/workflows/release.yml`：Windows job 会开始编译 wry/WebView2 绑定（编译期只需 MSVC + `windows` crate；运行期需要系统 WebView2 运行时，缺失时应回退浏览器而不是崩）。
- 文档：`README.md` 特性行、`docs/engineering-notes/gpui-native-webview-overlay.md`（补 Windows 侧层级/焦点结论）、后续 release notes 更新 v0.42.0 里那句「链接预览为 macOS 独有」。
- 风险集中在一个平台：Windows 的原生子窗口与 GPUI 渲染层级（工程笔记 `gpui-native-webview-overlay.md:53` 记的 child HWND + DirectComposition 冲突）。它无法在本机验证，只能在 Actions 与人工冒烟里收敛。
