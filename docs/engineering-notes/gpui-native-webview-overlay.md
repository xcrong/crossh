# GPUI 窗口内嵌原生 WebView：层级、坐标与焦点

## 症状

- 想在 GPUI 窗口里内嵌网页（临时预览终端里的链接）。
- 直接按 `gpui_web` 找实现找不到；按"容器裁剪/圆角"写的样式对网页无效。
- 网页盖住了命令面板、右键菜单、tooltip；或者页面不跟随卡片定位。

## 根因

本仓库 pin 的 upstream gpui（`Cargo.toml` 的 `rev`）**没有窗口合成树**：
`enable_window_composition` / `WindowCompositionSurface` 均不存在（这是 Zed 的
`gpui-fast` 分支与相关 PR 才有的能力）。因此只能走 wry 的"子视图"路径：
`WebViewBuilder::build_as_child(&*window)` 把 WKWebView `addSubview` 到
`GPUIView`（`Window: raw_window_handle::HasWindowHandle`，AppKit 句柄的
`ns_view` 就是它）。原生子视图由 AppKit 合成，**永远画在 GPUI 之上，且不受
GPUI 的 `overflow_hidden` / `content_mask` / `rounded` 影响**，位置只能靠
`set_bounds` 自己喂。

## 规则

- **webview 矩形内不要指望 GPUI 内容可见**：卡片边框、header、scrim 都要
  排在 webview 矩形之外；与网页矩形重叠的 GPUI 浮层（命令面板/右键菜单/
  重命名框）必须先把原生视图 `set_visible(false)`，页面状态保留，恢复时再
  `set_visible(true)`（见 `shell/link_preview.rs` 的 `covered`）。
- **坐标**：GPUI `Bounds<Pixels>` 是逻辑点、y 向下；wry 的 `set_bounds(Rect)`
  在 macOS 内部按 `backingScaleFactor` 换算并做 y 翻转，所以传
  `LogicalPosition/LogicalSize` 即可，**不要自己乘 scale_factor**，也不要自己翻 y。
- **只在 bounds 变化时 `set_bounds`**：每次调用都会走 ObjC 并让 WKWebView 重排。
- **焦点**：GPUI 的键盘事件只到 `GPUIView`。原生视图被创建/隐藏时调用
  `wry::WebView::focus_parent()` 把 first responder 交回 GPUIView；页面内的
  Esc 由初始化脚本经 IPC 转回（`window.ipc.postMessage('close')` + `with_ipc_handler`）。
- **生命周期**：`Rc<wry::WebView>` 必须在父窗口销毁前释放；`WebView` 是 `!Send`，
  只能存在 UI 线程的 Entity/全局里。
- **地址栏可输入就必须收回 first responder**：用户点进网页后 WKWebView 才是
  first responder，此时点 GPUI 一侧的输入框只是 `window.focus(...)`，键盘仍在网页里。
  卡片容器统一在 `on_mouse_down` 里先 `focus_parent()` 再聚焦输入框。
- **地址栏是独立 Entity**：`LinkAddress` 自己实现 `EntityInputHandler`（复用
  `crossh_core::input_handler` 的 IME/选区助手），不进 `AppShell` 的输入字段分发表——
  那张表服务 shell 级浮层，卡片跌不到。
- **不要在 `AppShell` 的更新里再 `address.update(cx, ...)`**：GPUI 实体是 lease
  语义，同一实体二次 lease 会 panic（`cannot ... during another update`）。
  因此回车提交时地址归一化（补 `https://`）在 `LinkAddress` 自己的 lease 内完成并
  写入自身状态，只把归一化后的 URL 交给 `navigate_link_preview`；
  页面回调那一路（`with_on_page_load_handler` → `sync_link_preview_url`）先 lease
  `AppShell` 再 lease `LinkAddress`，无嵌套冲突。
- **状态栏入口常驻**：与“临时终端”并列的图标不随 `link_preview` 是否存在而隐藏
  （否则新会话里永远看不到这张入口）。还没有预览时点它开起始页 `https://cn.bing.com`
  （`START_URL`）；建原生视图失败才回退 `cx.open_url`，按钮不会变成死键。
- **页面自身导航会改写 `preview.url`**：重定向/页内链接后“在浏览器打开”与状态栏
  要跟当前页，所以 `with_on_page_load_handler` 回写 `preview.url`；地址栏只在
  用户未改写（`state.value == page`）时跟随，避免吞掉正在输入的网址。
- **平台**：`build_as_child` 在 Wayland 明确不支持；Windows 需要 child HWND 且
  示例要关 `GPUI_DISABLE_DIRECT_COMPOSITION`。本仓库在 macOS 与 Windows 编译该依赖
  （`[target.'cfg(any(target_os = "macos", windows))'.dependencies] wry`）。
  Linux **不是嵌不了**：X11 上 `build_as_child` 只要 `RawWindowHandle::Xlib`，wry 用
  `gdk_x11_window_foreign_new_for_display` 把外部 X11 窗口包成 GTK toplevel
  （`webkitgtk/mod.rs:125-153`），GPUI 的窗口也算合格父窗口；代价是必须先 `gtk::init()`、
  在 GPUI 事件循环里推进 `gtk::main_iteration_do`，并引入 `gtk` / `webkit2gtk` / `soup3`
  与系统 WebKitGTK。Wayland 下只能走 `build_gtk(gtk::Fixed)`，GPUI 没有 GTK widget 可用。
  本仓库不为预览背这些依赖，Linux 编译的是 `shell/link_preview_unsupported.rs`
  占位实现，链接回退 `cx.open_url`。
  Windows 侧的 `webview2` 模块只看 target、不看 `os-webview` feature（`wry-0.57.1/src/lib.rs:391`），
  所以 `default-features = false` 可以保留；`set_bounds` 在 Windows 内部用 `hwnd_dpi`
  把逻辑尺寸转物理（`webview2/mod.rs:1639`），和 macOS 一样不要自己乘 scale。
- **终端链接只有一个入口**：`terminal::Event::Open` → `crossh_terminal_view::link_opener::open_link`。
  未安装 `LinkOpener` 全局时行为与之前一致（系统浏览器），所以设置窗口等处的
  外链不受影响。

## 验证

- `cargo check -p crossh --no-default-features --features wayland,x11`。
- `cargo test -p crossh --bin crossh only_http_schemes_are_previewable`、
  `cargo test -p crossh-assets`（新增图标可加载）、`cargo test -p terminal`。
- 手动（macOS）：终端里 `echo https://example.com` 后点链接 → 卡片弹出并渲染；
  后退/在浏览器打开/关闭按钮；地址栏点击可编辑（选区/IME/⌘V 粘贴），
  输 `example.com` 回车补成 `https://example.com`，页面内跳转后地址栏跟随；
  状态栏“临时网页”图标常驻：没预览过时点它打开 cn.bing.com 起始页，有预览后
  点击显示/隐藏且保留页面状态；
  Esc 关闭；拖动缩放窗口时网页跟随；`⌘⇧P` 打开
  命令面板时网页消失、关闭后恢复；关窗不崩溃。
- 手动（Windows，需实机；CI 只证明能编译）：同样步骤 + 以下特有项——WebView2 运行时存在
  时卡片能渲染、缺运行时应回退浏览器而不崩；DPI 100% / 150% 缩放下网页矩形与卡片内容区对齐；
  点进网页后能拿到键盘焦点，网页内 `Esc` 能关闭；打开命令面板/右键菜单时网页消失、关闭后恢复；
  若网页不可见或被 GPUI 盖住，先查是否是 DirectComposition 与子窗口的层级冲突，结论回写到本节。

## 关键词

`webview`, `wry`, `WKWebView`, `build_as_child`, `HasWindowHandle`, `ns_view`, `GPUIView`,
`原生子视图`, `图层顺序`, `盖住浮层`, `overlay`, `set_bounds`, `LogicalPosition`,
`backingScaleFactor`, `y 翻转`, `focus_parent`, `first responder`, `Esc 收不到`,
`ipc`, `Wayland 不支持`, `enable_window_composition`, `WindowCompositionSurface`,
`EntityInputHandler`, `TextEditingState`, `地址栏可输入`, `first responder`, `lease`,
`during another update`, `address.update`, `normalize_address`, `with_on_page_load_handler`,
`WebView2`, `HWND`, `DirectComposition`, `hwnd_dpi`, `gtk::init`, `main_iteration_do`,
`gdk_x11_window_foreign_new_for_display`, `build_gtk`, `Linux 回退浏览器`
