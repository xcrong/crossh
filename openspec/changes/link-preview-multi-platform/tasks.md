# Tasks

> 状态：代码与文档已落地；`[ ]` 的是本机无法完成的验证（Windows 实机冒烟 / Linux 编译，均为 CI 或人工），关闭后本变更才能归档。

## 1. 收敛平台判定（行为不变）

- [x] 1.1 去掉调用点的 8 处 `#[cfg(target_os = "macos")]`：`shell/mod.rs:54`（模块声明）、`:211`/`:213`（字段）、`:309`/`:311`（初始化）、`:399`（Esc 处理）、`:1280`（`LinkOpener` 安装）、`view.rs:993`（状态栏入口），改为无条件存在。`view.rs:646/650` 是分栏快捷键字形，不在本变更范围。
- [x] 1.2 新增 `shell/link_preview_unsupported.rs` 作为无宿主平台的占位实现（`link_preview.rs` 全程引用 `wry`，Linux 上整份文件编译不了，stub 无法同文件），由 `shell/mod.rs` 用 `#[path]` 选择；stub 提供 `LinkPreview`、`open_link_preview`（`false`）、`hide_link_preview`、`render_status_toggle`（`None`）、`render_link_preview`（空元素，`shell_render.rs:130` 无条件调用）。
- [x] 1.3 `LinkOpener` 在 Linux 上照旧安装：stub 的 `open_link_preview` 返回 `false`，`open_link` 于是走 `cx.open_url`（`crates/crossh-terminal-view/src/link_opener.rs:31-38`）。
- [ ] 1.4 Linux 上 stub 无 `dead_code` / `unused` 警告（`clippy -D warnings` 已在 macOS 通过；Linux 侧待新增 CI 首跑确认）。

## 2. Windows 接通

- [x] 2.1 `Cargo.toml` 的 wry 门控改为 `[target.'cfg(any(target_os = "macos", windows))'.dependencies]`，保留 `default-features = false`。
- [x] 2.2 平台相关代码不分叉：`focus_parent` / `set_bounds` / `set_visible` 三平台同签名（`wry-0.57.1/src/lib.rs:2264/2269/2283`），按原样复用。
- [ ] 2.3 坐标系假设待实机确认：代码已按「传逻辑点、不自行乘 scale」复用 `logical_rect()`（依据 `webview2/mod.rs:1639-1646` 内部按 `hwnd_dpi` 换算），100% / 150% 两档待 Windows 冒烟。
- [x] 2.4 三条 handler（页面加载回写地址、导航白名单、`Esc` IPC）都是平台无关 API，无需改动。

## 3. CI 覆盖

- [x] 3.1 新增 `.github/workflows/ci.yml`：`pull_request` + `push: main` 触发，`windows-latest` 跑 `cargo check --target x86_64-pc-windows-msvc`。
- [x] 3.2 Linux 检查同文件（`ubuntu-22.04` + `release.yml` 同款 apt 列表）：stub 只在 Linux 编译，没有这条通道等于没验证过。
- [x] 3.3 不把 `aarch64-pc-windows-msvc` 放进门禁（`release.yml:115-116` 该 target 已是 `continue-on-error`）。

## 4. 文档

- [x] 4.1 `README.md` 新增链接预览特性行（macOS / Windows 应用内预览，Linux 与创建失败回退浏览器）。
- [x] 4.2 `docs/engineering-notes/gpui-native-webview-overlay.md` 更新「平台」条目（依赖门控、Windows `hwnd_dpi`、Linux 不接的真实代价：`gtk::init` + 主循环 + GTK/WebKitGTK 依赖，Wayland 无路径），补 Windows 冒烟清单与关键词。
- [x] 4.3 不改 v0.42.0 的历史发布说明；平台表述在下个版本的 release notes 里更新。

## 5. 验证

- [x] 5.1 `cargo test --bin crossh link_preview`：3 passed（`only_http_schemes_are_previewable` / `bare_hosts_get_https_prefix` / `explicit_foreign_schemes_are_not_taken_over`）。
- [x] 5.2 `cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`scripts/check-architecture.sh` 全绿。
- [ ] 5.3 macOS 实机冒烟：终端链接开卡片、浮层避让、网页内 `Esc`、地址栏回写、失败回退。
- [ ] 5.4 `openspec validate link-preview-multi-platform --strict`。
- [x] 5.5 评审先于 apply 落盘（`review.md` 为「带条件通过」，条件 C1–C3 见该文件）。
