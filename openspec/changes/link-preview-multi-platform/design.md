# Design

## Context

链接预览在 macOS 已上线并稳定（`shell/link_preview.rs`，748 行 + 测试）。它依赖三条 macOS 事实：wry 的 WKWebView 子视图、`Window: HasWindowHandle` 暴露的 `ns_view`、以及「原生子视图恒在 GPUI 之上」这一点（见 `docs/engineering-notes/gpui-native-webview-overlay.md`）。

当前平台分化是硬编码出来的：`Cargo.toml:66-68` 按 target 门控 wry，`shell/mod.rs` + `view.rs` 里共 8 处 `#[cfg(target_os = "macos")]` 把模块、字段、Esc 处理、`LinkOpener` 安装、状态栏入口逐个摘掉。Linux / Windows 因此连这个能力的存在都看不到。

## Goals / Non-Goals

**Goals**

- 一份平台无关的行为契约（见 `specs/link-preview/spec.md`）。
- Windows 与 macOS 一样在应用内预览；Linux 明确回退且零新依赖。
- 平台分支收敛到单一文件，后续加平台只改一处。

**Non-Goals**

- 不在 Linux 嵌入任何 webview（含 X11 上的 GTK 浮窗方案，见 D2 的替代项）。
- 不支持 Wayland。
- 不做多预览实例、下载面板、devtools、前进/后退按钮等新 UI；继续单例复用。
- 不改 GPUI 上游/SDK，不为预览引入窗口合成树。

## Decisions

### D1 支持集合：macOS + Windows，Linux 明确不支持

Windows 可行：wry 的 `WebViewBuilder::build_as_child` 只要求 `HasWindowHandle`，Windows 分支从它取出 `RawWindowHandle::Win32(HWND)` 后自建容器子窗口（`wry-0.57.1/src/webview2/mod.rs:101-112`，`new_in_hwnd(parent, ..., is_child = true)`）。`set_bounds` / `set_visible` / `focus_parent` 三平台同签名（`src/lib.rs:2264/2269/2283`，`webview2/mod.rs:1678`），现有调用代码可原样复用。

Linux 现在不接，但不是不可行：wry 的 `build_as_child` 在 Linux 只要求 `HasWindowHandle` 给出 `RawWindowHandle::Xlib`（`wry-0.57.1/src/webkitgtk/mod.rs:125-153`），内部用 `gdk_x11_window_foreign_new_for_display` 把外部 X11 窗口包成 GTK toplevel（`webkitgtk/mod.rs:228-248`）——GPUI 的 X11 窗口也算合格的父窗口。代价是三步：先 `gtk::init()`（否则直接 panic），在 GPUI 事件循环里推进 `gtk::main_iteration_do`，并把 wry 特性打开到 `os-webview` + `x11`（于是拉进 `gtk 0.18` / `webkit2gtk` / `soup3` / `javascriptcore-rs`，运行期还要求系统有 WebKitGTK）。Wayland 才是硬限制：`new_x11` 只认 `RawWindowHandle::Xlib`，Wayland 下只能走 `WebViewBuilderExtUnix::build_gtk(gtk::Fixed)`（`lib.rs:2028-2042`），而 GPUI 没有 GTK widget 可以放。

替代方案：**Linux 也尝试（X11 路径）。** 否决理由是成本而不是不可能：要引 GTK/WebKitGTK 依赖、要在 GPUI 事件循环里跑第二个主循环、要重做 Linux 打包（AppImage 需自带或要求系统 WebKitGTK），而收益只是把链接预览从浏览器搬进窗口。这些都直接违背 v0.42.0 已承诺的「Linux 打包依赖不变」。如果将来要做，应单独立项，并把 GTK 事件循环整合与打包方案一起设计。

### D2 Windows 复用 wry 子窗口路径，不引入合成树

本仓库 pin 的 gpui 没有 `enable_window_composition` / `WindowCompositionSurface`（工程笔记「根因」一节），因此不存在「把 webview 当纹理贴进 GPUI 内容层」的选项。Windows 侧只能和 macOS 一样走子窗口，并接受同一组硬约束：原生视图恒在 GPUI 之上、不受裁剪、位置靠 `set_bounds` 喂。

替代方案：**改用 WebView2 composition controller。** wry 不暴露该模式（`webview2/mod.rs` 只走 child HWND），要走就得绕开 wry 直接绑 `webview2-com`，等于自建一次 webview 集成。否决：为了消除一条 macOS 已经接受了的约束，多养一套平台专用代码。

### D3 平台分支的所有权集中到 `link_preview.rs`

现状是调用点各自 `cfg`，代价是加平台要改 8 处、且每处都得想清楚「不支持的平台这里该怎么办」。改成：

- `shell/mod.rs:54` 的模块声明、`:211-214` 的字段、`:399` 的 Esc 处理、`:1280` 的 `LinkOpener` 安装、`view.rs:993` 的状态栏入口全部**去掉 cfg**，无条件存在；平台差异只剩模块声明一处：
  ```rust
  #[cfg(any(target_os = "macos", windows))]
  #[path = "link_preview.rs"]
  pub(crate) mod link_preview;
  #[cfg(not(any(target_os = "macos", windows)))]
  #[path = "link_preview_unsupported.rs"]
  pub(crate) mod link_preview;
  ```
- 占位实现单独一个文件（`link_preview_unsupported.rs`）：`link_preview.rs` 全程引用 `wry` 类型，Linux 上 wry 不存在，**整份文件都编译不了**，stub 无法与它同文件共存。stub 提供 `LinkPreview` 类型、`open_link_preview`（返回 `false`）、`hide_link_preview`、`render_status_toggle`（`None`）、`render_link_preview`（空元素，`shell_render.rs:130` 无条件调用它）。
- `LinkOpener` 也跟着无条件安装：stub 的 `open_link_preview` 返回 `false`，`open_link` 便回退 `cx.open_url`（`crates/crossh-terminal-view/src/link_opener.rs:31-38`），行为与现在的「不安装」等价，不需要为安装条件再写一个 cfg。

替代方案：**保持 8 处 cfg，只把 `macos` 改成 `any(macos, windows)`。** 否决：改动量更大（8 处×2），且下次加平台又要重复一遍同样的推理；顺带会让「Linux 为什么不行」继续散落在调用点注释里，而不是集中在一个文件的模块文档。

替代方案：**build.rs 注入自定义 `cfg`（如 `link_preview_supported`）。** 否决：为了少写一个 `cfg(any(...))` 引入自定义 cfg 的注册与维护成本（`cargo:rustc-check-cfg`），不划算。

### D4 依赖按 target 门控，Linux 保持零新增依赖

```toml
[target.'cfg(any(target_os = "macos", windows))'.dependencies]
wry = { version = "0.57", default-features = false }
```

`default-features = false` 保持不动：Windows 侧不需要 wry 的 `os-webview`（该 feature 主要给 Linux 的 GTK backend 拉 `gtk`/`webkit2gtk`/`soup3`）。Windows 的 WebView2 绑定是 target 依赖，随 target 自动生效。

打包后果：Windows 产物**不**新增随包二进制，但运行期需要系统 WebView2 运行时（Win10 1803+ 与装有 Edge 的机器普遍自带）。缺失时走 R5 回退。

### D5 验证责任：macOS 本机 + 新增 Windows PR CI

仓库现在**只有** `release.yml`（`on: push: tags: v*`），没有 PR CI。也就是说 Windows 侧编译回归要等到发版才暴露，这与「改动影响其他平台必须补对应 CI 覆盖」的规则冲突。因此本变更必须附带：

- 新增 PR 级 CI（`.github/workflows/ci.yml`）：`windows-latest` 的 `cargo check --target x86_64-pc-windows-msvc`，加 `ubuntu-22.04` 的同名检查（复制 `release.yml` 的 apt 列表）——后者是**必需**的，因为 stub 文件只在 Linux 编译，不自那验证就等于没编过。两者都只做 `cargo check`，不跑测试。
- 人工冒烟清单写进 `docs/engineering-notes/gpui-native-webview-overlay.md`：卡片能显示、网页不被 GPUI 覆盖、点击网页取得焦点、`Esc` 从网页内关闭、DPI 100%/150% 下矩形对齐、`set_visible(false)` 后浮层可见。

本机（macOS arm64）不跑任何 Windows 目标构建，符合 `AGENTS.md` 的本地责任边界。

### D6 未验证风险：Windows 的原生子窗口与 GPUI 渲染层级

工程笔记记过 Windows 示例需要关 `GPUI_DISABLE_DIRECT_COMPOSITION`，说明 DirectComposition 与子窗口的层级关系在 Windows 上不是自动成立的。处置策略是**先接线、后观察**：

1. 按现状实现，冒烟时若网页不可见/被 GPUI 盖住，先确认是否与 GPUI 的 DirectComposition 路径有关；
2. 只有在确认冲突时才评估 `GPUI_DISABLE_DIRECT_COMPOSITION` 或 GPUI 侧 z-order 处理，并把结论回写工程笔记与 D2。

不预先改 GPUI：那会把一个「可能不存在」的问题固化进架构。

另有一条同类未验证项：wry 在 Windows 自己调 `CoInitializeEx(COINIT_APARTMENTTHREADED)` 并忽略返回值（`webview2/mod.rs:121`）。若 GPUI 主线程已是别的 COM 模型，创建可能失败——失败即回退浏览器（R5），属可观察、可接受的降级，但冒烟应确认。

## Risks / Trade-offs

- **Windows 上可能「能编译但不能用」**：CI 只能证明编译；UI 正确性靠人工冒烟，因此 Windows 预览在发布说明里应标注为需要实机确认的能力。
- **aarch64 Windows**：`release.yml` 里该 target 已是 `continue-on-error`，wry 在其上能否编译未知；不阻塞发布，但要在 PR CI 里只验 x86_64，避免把不稳定的 target 变成红灯。
- **Linux 用户仍然被送进浏览器**：这是刻意取舍（D1），规格里已写成明确要求，不是待补的洞。
- **单例 + 子窗口模型的债**：两个平台都受「原生视图不可裁剪」约束，将来若 GPUI 有了合成树，这块可以整体简化；本变更不预付这笔改造。

## Migration Plan

- 无数据/配置迁移。
- 行为迁移只发生在 Windows：同一链接从「系统浏览器」变为「应用内卡片」；失败回退保留。
- 回滚方式：把 `Cargo.toml` 的 target 门控与 D3 的模块 stub 改回 macOS-only 即可，无状态残留（预览是纯运行时状态）。

## Open Questions

- Windows 冒烟若确认 DirectComposition 冲突，是否需要单独的 GPUI 侧变更（届时按 `AGENTS.md` 另立 change，不在本变更内顺手改）。
- WebView2 运行时缺失时，是否要在状态栏给一次性提示（当前按 R5 静默回退，与 macOS 失败路径一致）。
