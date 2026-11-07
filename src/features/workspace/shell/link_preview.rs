//! 链接预览：把终端里的链接开在临时 WebView 悬浮层里（当前仅 macOS）。
//!
//! 形态照 `scratch_bar` 的抽屉：整窗 scrim + 居中卡片，卡片内容区承载 `wry`
//! 的 WKWebView 原生子视图。原生子视图恒在 GPUI 之上、且无法被 GPUI 裁剪，
//! 因此这里有两条硬约束：
//!
//! - 卡片边框、header、scrim 必须落在 webview 矩形之外（webview 只占内容区）；
//! - 其他 GPUI 浮层（命令面板 / 右键菜单 / 重命名框）打开时隐藏原生视图，
//!   否则它们会被网页盖住；页面状态保留，浮层关闭后直接重新显示。

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Bounds, ClipboardEntry, ClipboardItem, Context, Element,
    ElementId, Entity, EntityInputHandler, FocusHandle, GlobalElementId, Hitbox, HitboxBehavior,
    InteractiveElement as _, IntoElement, KeyDownEvent, LayoutId, MouseButton, ParentElement as _,
    Pixels, Render, SharedString, Size, Style, Styled as _, UTF16Selection, WeakEntity, Window,
    div, px,
};

use super::AppShell;
use crossh_core::i18n;
use crossh_core::input_handler::{
    editing_mark_text, editing_marked_range, editing_replace, editing_selected_range,
    editing_unmark,
};
use crossh_core::text_editing::{
    EditingKeystroke, TextEditingState, byte_index_for_utf16, handle_text_editing_key, utf16_len,
    utf16_slice,
};
use crossh_ui::widgets::ime_caret_bounds;
use crossh_ui::{icons, theme};
use crossh_ui_component::{Button, ButtonSize, ButtonVariant, TextInput};

/// 卡片占视口的比例：与临时终端抽屉同一档"大卡片"观感。
const CARD_WIDTH_RATIO: f32 = 0.78;
const CARD_HEIGHT_RATIO: f32 = 0.72;

/// 还没有任何预览时，状态栏按钮开这个起始页（国内可直连）。
const START_URL: &str = "https://cn.bing.com";

/// 页面内的 Esc 兜底：原生视图抢到键盘焦点时 GPUI 收不到按键，由页面经 IPC 转回来。
const ESCAPE_SCRIPT: &str = r#"
document.addEventListener('keydown', function (event) {
  if (event.key === 'Escape') {
    event.preventDefault();
    window.ipc.postMessage('close');
  }
}, true);
"#;

/// 地址栏：可编辑的网址输入框（选区 / IME / 剪贴板复用 `TextEditingState`）。
///
/// 自己实现 `EntityInputHandler`：IME 文本只进这一张输入框，不进 `AppShell`
/// 的输入字段分发表（那张表服务 shell 级浮层，预览卡片是独立实体）。
struct LinkAddress {
    state: TextEditingState,
    /// 当前页面真实地址（用来判断用户是否已改写输入框）。
    page: SharedString,
    focus: FocusHandle,
    shell: WeakEntity<AppShell>,
}

impl LinkAddress {
    fn new(url: &str, shell: WeakEntity<AppShell>, cx: &mut Context<Self>) -> Self {
        Self {
            state: TextEditingState::new(url),
            page: SharedString::from(url.to_string()),
            focus: cx.focus_handle(),
            shell,
        }
    }

    /// 强制展示给定地址（回车导航 / 点链接复用预览后）。
    fn show_url(&mut self, url: &str, cx: &mut Context<Self>) {
        self.page = SharedString::from(url.to_string());
        self.state = TextEditingState::new(url);
        cx.notify();
    }

    /// 页面自己跳转（重定向 / 页内链接）后同步：用户改写过的输入不被吞掉。
    fn set_page_url(&mut self, url: &str, cx: &mut Context<Self>) {
        if self.page.as_ref() == url {
            return;
        }
        if self.state.value == self.page.as_ref() {
            self.state = TextEditingState::new(url);
            cx.notify();
        }
        self.page = SharedString::from(url.to_string());
    }

    /// 回车提交：在自身状态内完成地址归一化与展示，再请 shell 导航。
    ///
    /// 归一化放这里是因为 `navigate_link_preview` 运行在 `AppShell` 的 lease 内，
    /// 回去 `address.update(...)` 会在同一实体二次 lease 而 panic。
    fn commit(&mut self, cx: &mut Context<Self>) {
        let address = self.state.value.trim().to_owned();
        if address.is_empty() {
            return;
        }
        let Some(url) = normalize_address(&address) else {
            // 非 http(s)（`file:` / `mailto:`）：与链接点击一致交系统浏览器。
            cx.open_url(&address);
            return;
        };
        self.show_url(&url, cx);
        let shell = self.shell.clone();
        let _ = shell.update(cx, |shell, cx| shell.navigate_link_preview(&url, cx));
    }

    fn handle_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        // enter 是导航提交 > 通用编辑；escape 不拦，冒泡到 shell 关预览。
        if keystroke.key == "enter" || keystroke.key == "return" {
            self.commit(cx);
            return;
        }
        let primary = keystroke.modifiers.control || keystroke.modifiers.platform;
        let paste_text = if primary && keystroke.key == "v" {
            cx.read_from_clipboard().and_then(|item| {
                item.into_entries().find_map(|entry| match entry {
                    ClipboardEntry::String(value) => Some(value.text),
                    _ => None,
                })
            })
        } else {
            None
        };
        let editing = EditingKeystroke {
            key: keystroke.key.clone(),
            key_char: keystroke.key_char.clone(),
            control: keystroke.modifiers.control,
            platform: keystroke.modifiers.platform,
            shift: keystroke.modifiers.shift,
        };
        let result = handle_text_editing_key(&mut self.state, &editing, paste_text.as_deref());
        if let Some(text) = result.copy_text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        if result.handled {
            cx.notify();
        }
    }
}

impl EntityInputHandler for LinkAddress {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        Some(utf16_slice(&self.state.value, range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let selection = editing_selected_range(&self.state);
        Some(UTF16Selection {
            range: selection.range,
            reversed: selection.reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        editing_marked_range(&self.state)
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        editing_unmark(&mut self.state);
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        editing_replace(&mut self.state, replacement_range, text);
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        editing_mark_text(&mut self.state, new_text);
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let cursor = byte_index_for_utf16(&self.state.value, range.start);
        Some(ime_caret_bounds(
            window,
            element_bounds,
            &self.state.value[..cursor],
            ADDRESS_TEXT_SIZE,
            ADDRESS_PADDING_X,
            px(0.),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(utf16_len(&self.state.value))
    }
}

impl Render for LinkAddress {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.state.value.is_empty();
        TextInput::new("link-preview-address", self.focus.clone())
            .value(self.state.value.clone())
            .placeholder(i18n::text("link_preview.address_placeholder"))
            .ime_marked_text(self.state.ime_marked_text.clone())
            .selection(self.state.selection())
            .cursor(self.state.cursor)
            .text_color(if empty {
                theme::faint_text()
            } else {
                theme::text()
            })
            .bg(theme::canvas())
            .height(px(26.))
            .padding_x(ADDRESS_PADDING_X)
            .text_size(ADDRESS_TEXT_SIZE)
            .flex_1()
            .entity(cx.entity())
            .on_key_down(cx.listener(Self::handle_key))
    }
}

/// 单例链接预览：只保留一个原生 WebView，换链接时复用（保留前进/后退历史）。
pub(crate) struct LinkPreview {
    url: SharedString,
    webview: Rc<wry::WebView>,
    address: Entity<LinkAddress>,
    /// 上次同步给原生视图的窗口坐标，避免每帧重复 `set_bounds`。
    last_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl LinkPreview {
    /// 在 `window` 里创建原生子视图。失败（没有可用 WebKit）时返回 `None`。
    fn build(url: &SharedString, window: &mut Window, cx: &mut Context<AppShell>) -> Option<Self> {
        let shell = cx.entity().downgrade();
        let async_cx = cx.to_async();
        let address = cx.new(|cx| LinkAddress::new(url.as_ref(), shell.clone(), cx));
        let page_shell = shell.clone();
        let page_cx = async_cx.clone();
        let webview = wry::WebViewBuilder::new()
            .with_url(url.as_ref())
            // 页面自己跳转（重定向 / 页内链接）后把 shell 记录与地址栏同步回真实地址。
            .with_on_page_load_handler(move |_event, url| {
                let shell = page_shell.clone();
                page_cx.update(move |cx| {
                    let _ = shell.update(cx, |shell, cx| shell.sync_link_preview_url(&url, cx));
                });
            })
            // 只放行 http(s)：页面里的其他 scheme（file:/自定义）交回系统处理。
            .with_navigation_handler(|target| is_web_url(&target))
            .with_initialization_script(ESCAPE_SCRIPT)
            .with_ipc_handler(move |request| {
                if request.body().as_str() != "close" {
                    return;
                }
                let shell = shell.clone();
                async_cx.update(move |cx| {
                    let _ = shell.update(cx, |shell, cx| shell.hide_link_preview(cx));
                });
            })
            .build_as_child(&*window)
            .inspect_err(|error| log::warn!("link preview webview unavailable: {error}"))
            .ok()?;
        // 焦点留在 GPUI（Esc / 快捷键才收得到）；点进网页后由页面接管焦点。
        let _ = webview.focus_parent();

        Some(Self {
            url: url.clone(),
            webview: Rc::new(webview),
            address,
            last_bounds: Rc::new(Cell::new(None)),
        })
    }

    /// 显示并交还键盘焦点给 GPUI。
    fn show(&self) {
        let _ = self.webview.focus_parent();
        let _ = self.webview.set_visible(true);
    }

    fn hide(&self) {
        let _ = self.webview.focus_parent();
        let _ = self.webview.set_visible(false);
    }
}

impl AppShell {
    /// 打开（或复用）链接预览。返回 `false` 表示不接管，调用方回退系统浏览器。
    pub(crate) fn open_link_preview(
        &mut self,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !is_web_url(url) {
            return false;
        }
        let url = SharedString::from(url.to_string());

        match self.link_preview.as_mut() {
            Some(preview) if preview.url == url => preview.show(),
            Some(preview) => {
                preview.url = url.clone();
                let _ = preview.webview.load_url(url.as_ref());
                preview.show();
            }
            None => match LinkPreview::build(&url, window, cx) {
                Some(preview) => self.link_preview = Some(preview),
                None => return false,
            },
        }
        // 每次打开都把地址栏校正到当前页（丢掉上次没回车提交的输入）。
        if let Some(preview) = self.link_preview.as_ref() {
            let address = preview.address.clone();
            address.update(cx, |address, cx| address.show_url(url.as_ref(), cx));
        }

        self.link_preview_visible = true;
        cx.notify();
        true
    }

    /// 页面自身导航后的同步：`url` 作为新的“当前页”（后退/前进、外部打开、状态栏都看它）。
    fn sync_link_preview_url(&mut self, url: &str, cx: &mut Context<Self>) {
        let Some(preview) = self.link_preview.as_mut() else {
            return;
        };
        preview.url = SharedString::from(url.to_string());
        let address = preview.address.clone();
        address.update(cx, |address, cx| address.set_page_url(url, cx));
        cx.notify();
    }

    /// 地址栏回车后的导航（`url` 已归一化为 http(s)，地址栏由 `LinkAddress` 自己更新）。
    pub(crate) fn navigate_link_preview(&mut self, url: &str, cx: &mut Context<Self>) {
        let Some(preview) = self.link_preview.as_mut() else {
            return;
        };
        if preview.url.as_ref() != url {
            preview.url = SharedString::from(url.to_string());
            let _ = preview.webview.load_url(url);
        }
        preview.show();
        self.link_preview_visible = true;
        cx.notify();
    }

    pub(crate) fn hide_link_preview(&mut self, cx: &mut Context<Self>) {
        if !self.link_preview_visible {
            return;
        }
        self.link_preview_visible = false;
        if let Some(preview) = &self.link_preview {
            preview.hide();
        }
        self.refocus_active_terminal(cx);
        cx.notify();
    }

    /// 状态栏入口：显示/隐藏上一次的链接预览；还没有预览时开一个起始页。
    pub(crate) fn toggle_link_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.link_preview_visible {
            self.hide_link_preview(cx);
        } else if let Some(preview) = &self.link_preview {
            preview.show();
            self.link_preview_visible = true;
            cx.notify();
        } else if !self.open_link_preview(START_URL, window, cx) {
            // 拿不到原生 WebKit 时退回系统浏览器，不让按钮变成死键。
            cx.open_url(START_URL);
        }
    }
}

/// 状态栏入口：与“临时终端”并列的常驻图标；没有预览过的网页时点它开起始页。
pub(crate) fn render_status_toggle(
    shell: &AppShell,
    cx: &mut Context<AppShell>,
) -> Option<AnyElement> {
    let active = shell.link_preview_visible;
    let available = shell.link_preview.is_some();
    Some(
        Button::new("status-link-preview")
            .size(ButtonSize::Icon(px(22.)))
            .variant(ButtonVariant::Ghost)
            .selected(active)
            .icon(
                icons::icon(icons::IconName::Link, 13.).text_color(if active {
                    theme::accent()
                } else {
                    theme::muted_text()
                }),
            )
            .tooltip(i18n::text(if available {
                "tooltip.link_preview"
            } else {
                "tooltip.link_preview_empty"
            }))
            .on_click(cx.listener(|this, _ev, window, cx| {
                this.toggle_link_preview(window, cx);
            }))
            .into_any_element(),
    )
}

/// 渲染悬浮层。与 `render_scratch_panel` 同构：scrim 拦截点击，卡片承载内容。
pub(crate) fn render_link_preview(
    shell: &mut AppShell,
    window: &Window,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let Some(preview) = shell.link_preview.as_ref() else {
        return div().into_any_element();
    };
    let url = preview.url.clone();
    let address = preview.address.clone();
    let webview = preview.webview.clone();
    let last_bounds = preview.last_bounds.clone();
    // 原生视图会盖住与它矩形重叠的 GPUI 内容，这些浮层打开时先把它藏起来。
    let covered = shell.command_palette.is_some()
        || shell.context_menu.is_some()
        || shell.rename_editor.is_some()
        || shell.default_command_editor.is_some();

    let viewport = window.viewport_size();
    let card_width = viewport.width.as_f32() * CARD_WIDTH_RATIO;
    let card_height = viewport.height.as_f32() * CARD_HEIGHT_RATIO;

    let back = webview.clone();
    let forward = webview.clone();
    let focus_view = webview.clone();
    let external = url.clone();

    let header = div()
        .h(px(36.))
        .flex_shrink_0()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .px_3()
        .bg(theme::surface())
        .border_b_1()
        .border_color(theme::border())
        .rounded_t(px(theme::RADIUS_MD))
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap_2()
                .child(icons::icon(icons::IconName::Link, 14.).text_color(theme::accent()))
                .child(address),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    icon_button(
                        "link-preview-back",
                        icons::IconName::ArrowLeft,
                        "link_preview.back",
                    )
                    .on_click(move |_ev, _window, _cx| {
                        let _ = back.evaluate_script("history.back()");
                    }),
                )
                .child(
                    icon_button(
                        "link-preview-forward",
                        icons::IconName::ArrowRight,
                        "link_preview.forward",
                    )
                    .on_click(move |_ev, _window, _cx| {
                        let _ = forward.evaluate_script("history.forward()");
                    }),
                )
                .child(
                    icon_button(
                        "link-preview-external",
                        icons::IconName::ExternalLink,
                        "link_preview.open_in_browser",
                    )
                    .on_click(move |_ev, _window, cx| cx.open_url(external.as_ref())),
                )
                .child(
                    icon_button(
                        "link-preview-close",
                        icons::IconName::X,
                        "link_preview.close",
                    )
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.hide_link_preview(cx);
                    })),
                ),
        );

    let card = div()
        .id("link-preview-card")
        .w(px(card_width))
        .h(px(card_height))
        .flex()
        .flex_col()
        .bg(theme::surface())
        .border_1()
        .border_color(theme::border_strong())
        .rounded(px(theme::RADIUS_MD))
        .shadow_lg()
        .overflow_hidden()
        .child(header)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .bg(theme::canvas())
                .child(WebViewSurface {
                    webview,
                    last_bounds,
                    hidden: covered,
                }),
        );

    div()
        .id("link-preview-scrim")
        .absolute()
        .inset_0()
        .flex()
        .items_start()
        .justify_center()
        .pt(px(72.))
        .bg(theme::scrim())
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _ev, _window, cx| this.hide_link_preview(cx)),
        )
        .child(
            div()
                .id("link-preview-wrapper")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_, _ev, _window, cx| {
                        // 点回卡片内任意处（地址栏 / 按钮）先把 First Responder 从 WKWebView
                        // 收回到 GPUIView，否则键盘输入仍会进网页。
                        let _ = focus_view.focus_parent();
                        cx.stop_propagation();
                    }),
                )
                .child(card),
        )
        .into_any_element()
}

fn icon_button(id: &'static str, icon: icons::IconName, tooltip: &'static str) -> Button {
    Button::new(id)
        .size(ButtonSize::Icon(px(22.)))
        .variant(ButtonVariant::Ghost)
        .icon(icons::icon(icon, 12.).text_color(theme::muted_text()))
        .tooltip(i18n::text(tooltip))
}

/// 地址栏字号/内边距：渲染与 IME caret 定位共用同一组值。
const ADDRESS_TEXT_SIZE: Pixels = px(12.);
const ADDRESS_PADDING_X: Pixels = px(8.);

/// 只有 http(s) 才在预览里打开；其余 scheme（如 `file://`）交给系统浏览器。
fn is_web_url(url: &str) -> bool {
    let url = url.trim().to_ascii_lowercase();
    url.starts_with("http://") || url.starts_with("https://")
}

/// 地址栏输入 → 可预览的 URL：裸域名/路径补 `https://`；其他 scheme 返回 `None`。
fn normalize_address(input: &str) -> Option<String> {
    let text = input.trim();
    if text.is_empty() {
        return None;
    }
    if is_web_url(text) {
        return Some(text.to_string());
    }
    // `file:` / `mailto:` / `javascript:` 这类显式 scheme 不接管（端口号 port 不以数字开头的不算）。
    if let Some((scheme, rest)) = text.split_once(':')
        && !scheme.is_empty()
        && !scheme.contains(['/', '.'])
        && !rest.starts_with(|c: char| c.is_ascii_digit())
    {
        return None;
    }
    Some(format!("https://{text}"))
}

/// wry 在 macOS 内部自行处理 y 轴翻转与 Retina 换算，这里只转成逻辑坐标。
fn logical_rect(bounds: Bounds<Pixels>) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(
            f64::from(bounds.origin.x),
            f64::from(bounds.origin.y),
        )
        .into(),
        size: wry::dpi::LogicalSize::new(
            f64::from(bounds.size.width),
            f64::from(bounds.size.height),
        )
        .into(),
    }
}

/// 把原生子视图对齐到 GPUI layout 给出的矩形，并拦截落在其上的鼠标事件。
struct WebViewSurface {
    webview: Rc<wry::WebView>,
    last_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    hidden: bool,
}

impl IntoElement for WebViewSurface {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for WebViewSurface {
    type RequestLayoutState = ();
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = Style {
            size: Size::full(),
            flex_shrink: 1.,
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        if self.hidden {
            let _ = self.webview.set_visible(false);
            return None;
        }

        let _ = self.webview.set_visible(true);
        if self.last_bounds.get() != Some(bounds) {
            if let Err(error) = self.webview.set_bounds(logical_rect(bounds)) {
                log::warn!("link preview bounds update failed: {error}");
            }
            self.last_bounds.set(Some(bounds));
        }

        Some(window.insert_hitbox(bounds, HitboxBehavior::BlockMouse))
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        _: &mut Window,
        _: &mut App,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::{is_web_url, normalize_address};

    #[test]
    fn only_http_schemes_are_previewable() {
        assert!(is_web_url("https://example.com/docs"));
        assert!(is_web_url("HTTP://example.com"));
        assert!(is_web_url("  http://127.0.0.1:3000  "));
        assert!(!is_web_url("file:///tmp/a.html"));
        assert!(!is_web_url("mailto:a@b.c"));
        assert!(!is_web_url("javascript:alert(1)"));
        assert!(!is_web_url("example.com"));
    }

    #[test]
    fn bare_hosts_get_https_prefix() {
        assert_eq!(
            normalize_address("example.com").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            normalize_address("  example.com/a?b=1  ").as_deref(),
            Some("https://example.com/a?b=1")
        );
        assert_eq!(
            normalize_address("127.0.0.1:3000").as_deref(),
            Some("https://127.0.0.1:3000")
        );
        assert_eq!(
            normalize_address("localhost:8080").as_deref(),
            Some("https://localhost:8080")
        );
        assert_eq!(
            normalize_address("http://a.b").as_deref(),
            Some("http://a.b")
        );
        assert_eq!(normalize_address("   "), None);
    }

    #[test]
    fn explicit_foreign_schemes_are_not_taken_over() {
        assert_eq!(normalize_address("file:///tmp/a.html"), None);
        assert_eq!(normalize_address("mailto:a@b.c"), None);
        assert_eq!(normalize_address("javascript:alert(1)"), None);
    }
}
