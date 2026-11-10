# Review: link-preview-multi-platform

- 评审日期：2026-11-08
- 评审对象：`proposal.md` / `specs/link-preview/spec.md` / `design.md` / `tasks.md`
- 结论：**带条件通过**

## 核对方式

行为主张逐条对真实代码与依赖源码核对（`rg` / `read` / `wry-0.57.1` 源码），不采信提案自述。核对过的关键事实：

| 主张 | 证据 | 结论 |
| --- | --- | --- |
| 平台分化是 8 处手工 `cfg` 造成 | `shell/mod.rs:54,211,213,309,311,399,1280` + `view.rs:993` | 成立 |
| Windows 上 wry 接受任意 HWND | `wry-0.57.1/src/webview2/mod.rs:101-112`（取 `RawWindowHandle::Win32` → `new_in_hwnd(..., is_child=true)`） | 成立 |
| Windows 不需要 `os-webview` feature | `wry-0.57.1/src/lib.rs:391-392` 的 `webview2` 模块只按 `target_os = "windows"` 门控，不受 feature 约束 | 成立，`default-features = false` 可留 |
| Linux 当前不接 | 早期评审引 `wry-0.57.1/src/webkitgtk/mod.rs:255/693/1233`（那是 `new_gtk` / `build_gtk` 的 `IsA<gtk::Container>` 约束），**用错了**；真正的 `build_as_child` 路径是 `webkitgtk/mod.rs:125-153`（只认 `RawWindowHandle::Xlib`） | 已纠正，见 R6 |
| 三平台 API 同签名可复用 | `wry-0.57.1/src/lib.rs:2264/2269/2283` + `webview2/mod.rs:1678` | 成立 |
| Windows 坐标语义与 macOS 一致 | `webview2/mod.rs:1639-1646` 用 `hwnd_dpi` 把逻辑尺寸转物理 | 成立，复用 `logical_rect()` 正确 |
| 创建失败会回退浏览器 | `link_preview.rs:342-358` 返回 `false`，`crates/crossh-terminal-view/src/link_opener.rs:31-38` 随即 `cx.open_url` | 成立，R5 不需要新路径 |
| 仓库只有 tag 触发的 release CI | `.github/workflows/` 仅 `release.yml`（`on: push: tags: v*`） | 成立，D5 的补 CI 要求必要 |
| aarch64 Windows 已非门禁 | `release.yml:115-116` `continue_on_error: true` | 成立，T3.2 正确 |
| COM 初始化由 wry 自理且忽略失败 | `webview2/mod.rs:121` `let _ = CoInitializeEx(..., COINIT_APARTMENTTHREADED)` | 成立，D6 的未验证项属实 |

## 发现

### R1（已修）规格把「确认框」写进浮层避让，代码里没有

`spec.md` 原先把浮层列为「命令面板、右键菜单、重命名框、确认框」，但 `link_preview.rs:469-472` 的 `covered` 只含 `command_palette` / `context_menu` / `rename_editor` / `default_command_editor`，退出确认与关标签确认都不在其中——规格声称了一个不存在的行为。

**处理**：规格改为实际四类；确认框重叠是既有缺口，不在本变更扩范围（本变更只做平台对齐），记入下方遗留项。

### R2（已修）规格里有一个不可观察的场景

原文 R5 场景「运行期 webview 丢失 → 清空预览状态」无法验证：wry 没有 destroyed 回调，`LinkPreview` 是 `Rc<wry::WebView>` 单例（`link_preview.rs:271-277`），生命周期跟 shell 绑定，不存在「运行期丢失」这个可触发的状态。

**处理**：改为可观察的「已打开时切换链接 → 复用同一原生视图」（`open_link_preview` 的复用分支，`link_preview.rs:349-357`）。

### R3（已修）任务里的 Linux stub 清单漏了 `render_link_preview`

`shell_render.rs:130` 无条件调用 `link_preview::render_link_preview`。原任务只列了 `build` / `show` / `hide` / `render_status_toggle`，照此实现 Linux 会直接编译失败。

**处理**：tasks 1.2 补上 `render_link_preview` 返回空元素；design D3 同步。

### R4（已修）「去掉 8 处 cfg」会顺带改变 `LinkOpener` 安装条件，但没人说明是否安全

`shell/mod.rs:1280` 的 cfg 一去掉，Linux 也会安装 opener。核对后是安全的：stub 的 `open_link_preview` 返回 `false`，`open_link` 于是 `cx.open_url`（`link_opener.rs:31-38`），行为与不安装等价。但方案原先没说这件事，实施者容易以为还要额外加 cfg。

**处理**：design D3 与 tasks 1.3 写明「照旧安装、靠 stub 返回 false 回退」。

### R6（已修）评审自己的事实错误：Linux 并非「嵌不了」

首轮评审把 wry 的 `new_gtk` / `build_gtk` 约束（`IsA<gtk::Container>`，`webkitgtk/mod.rs:255/693/1233`）当成了 `build_as_child` 的前置条件，据此断言 Linux 在结构上不可行。实际上 `build_as_child` 在 Linux 走的是 `new_x11`（`webkitgtk/mod.rs:125-153`），只要一个 `RawWindowHandle::Xlib`，wry 内部用 `gdk_x11_window_foreign_new_for_display` 把外部 X11 窗口包成 GTK toplevel。

真实代价是：先 `gtk::init()`（否则 panic）、在 GPUI 事件循环里推进 `gtk::main_iteration_do`、开 `os-webview` + `x11` 并引入 `gtk` / `webkit2gtk` / `soup3` 与系统 WebKitGTK；Wayland 下只能 `build_gtk(gtk::Fixed)`（GPUI 无 GTK widget）。

**处理**：`proposal.md` / `design.md` D1 / `README.md` / `link_preview.rs` / `link_preview_unsupported.rs` / 工程笔记的措辞全部改为「不接是成本决策，不是能力缺失」。决策本身（Linux 保持回退）不变，因此评审结论仍为「带条件通过」；但否决理由必须写成成本，否则后来者会凭错误前提排除一条可行路径。

教训：引用依赖源码时必须以「调用入口 → 实际分支」逐跳核对，不能拿相邻同类函数的 trait bound 当证据。

### R5（记录，不阻塞）规格与任务对 Windows 的断言边界清楚

D6 明确「先接线后观察」，没有预先把 `GPUI_DISABLE_DIRECT_COMPOSITION` 或 GPUI 侧改动写进范围，这符合「不为可能不存在的问题固化架构」。相应地必须接受：**Windows 在人工冒烟通过之前不算已验证**，发布说明不得写成「已支持」。

## 必须关闭的条件

- **C1 新增 Windows + Linux PR CI 必须与本变更同时落地。** 仓库现在没有任何 PR CI（仅 `release.yml`），Windows 编译回归要到发 tag 才暴露，Linux 的 stub 更是只有 Linux 会编；若 CI 没进同一个变更，本变更不得 archive。
- **C2 Windows 冒烟结论必须回写 `docs/engineering-notes/gpui-native-webview-overlay.md`。** 至少覆盖：卡片可见、网页不被 GPUI 覆盖、点进网页能取焦、网页内 `Esc` 关闭、DPI 100%/150% 对齐、`set_visible(false)` 后浮层可见。结论不能只留在 PR 描述里。
- **C3 若冒烟暴露 DirectComposition 层级冲突，GPUI 侧改动必须另立 change。** 不允许在本变更里顺手改渲染/窗口层。

## 遗留项（本次不做，需另立变更）

- 退出确认、关标签确认框与网页矩形重叠时，原生视图不会被隐藏（`covered` 未覆盖确认框）——修一行即可，但属 macOS 现有行为变更，留给独立提案。
- WebView2 运行时缺失时是否给一次性提示（当前按 R5 静默回退）。
- Linux 若将来要应用内预览：X11 路径技术上成立（Xlib 窗口 + `gtk::init` + 推进 `main_iteration_do`），难点在 GTK/WebKitGTK 依赖与打包，以及 Wayland 无路径；应作为独立变更设计。

## 实施期偏离记录（2026-11-08）

评审通过后实施时发现两处与原文不符，已同步回 `design.md` / `tasks.md`：

1. **stub 不能与 `link_preview.rs` 同文件**（D3 已改）。`link_preview.rs` 全程引用 `wry` 类型，Linux 上 wry 不存在，整份文件都编译不了。改为独立文件 `shell/link_preview_unsupported.rs`，由 `shell/mod.rs` 的两条 `#[path = ...]` 声明选择模块，调用点仍然零 `cfg`。
2. **CI 必须同时覆盖 Linux**（D5 已改）。stub 是 Linux 独有的编译产物，只加 Windows 检查等于没有验证过它。`ci.yml` 现有 `windows-check` 与 `linux-check` 两个 job，均只跑 `cargo check`。

另有一处顺手简化：`view.rs` 的 `render_link_preview_toggle` 包装函数删除，状态栏直接调用 `link_preview::render_status_toggle`（stub 已返回 `None`，包装不再承担平台差异）。

## 条件状态

- **C1（Windows/Linux PR CI）**：**已关闭**（2026-11-10）。工作流 `.github/workflows/ci.yml` 首次运行就抓到真实回归：`shell/link_preview_unsupported.rs:48` 的 `div().into_any_element()` 缺 `IntoElement` 导入，Linux `cargo check` 报 E0599（run `42542315773`；macOS 因 `#[path]` 换成另一份实现，从不编译该文件，所以本地一直是绿的）；补导入后 main push 上 Windows / Linux check 双绿（run `42733714933`）。仓库目前没有 PR 流程，绿的是 main push 运行；发布链路也由此恢复，v0.43.0 三个平台六个构建全部成功（run `42734925271`）。
- **C2（Windows 冒烟结论回写工程笔记）**：清单已写入 `docs/engineering-notes/gpui-native-webview-overlay.md`，**未关闭** —— 需实机执行并回写结论。
- **C3（DirectComposition 冲突另立变更）**：约束有效，尚未触发。

本机已验证：`cargo check --bin crossh`、`cargo test --bin crossh link_preview`（3 passed）、`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`scripts/check-architecture.sh`。Windows / Linux 目标未在本机构建（`AGENTS.md` 的本地责任边界）。因此本变更暂不归档。
