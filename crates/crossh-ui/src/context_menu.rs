//! 轻量右键上下文菜单：类型 + 定位 + 渲染。
//!
//! GPUI 没有窗口内菜单 API（`Menu`/`MenuItem` 只用于 macOS 菜单栏），
//! 这里按语言菜单/模态的既有样式自建：全屏 scrim（点击即关闭）+ 定位菜单。
//! 菜单状态由各拥有者（AppShell / TerminalView / SftpPane）持有，渲染时
//! 作为根 div 的最后一个 child 挂载，保证 z 序最高。

use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels,
    Point, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::{icons, theme};

/// 菜单固定宽度；定位钳制时按此估算。
pub const CONTEXT_MENU_WIDTH: f32 = 216.0;
/// 单个菜单项高度。
const ITEM_HEIGHT: f32 = 28.0;
/// 分隔线行高。
const SEPARATOR_HEIGHT: f32 = 8.0;
/// 菜单上下内边距。
const MENU_PADDING: f32 = 8.0;

/// 外壳级（侧栏/标签条/终端/SFTP 由各自模块自持）菜单动作。
#[derive(Clone, Debug)]
pub enum ShellMenuAction {
    /// 打开远程主机终端。
    OpenHost(usize),
    /// 打开远程主机 SFTP。
    OpenSftp(usize),
    /// 打开远程主机端口转发。
    OpenForward(usize),
    /// 复制任意文本到剪贴板。
    CopyText(String),
    /// 在 Finder 中显示本地目录。
    RevealInFinder(PathBuf),
    /// 从「最近本地目录」移除。
    ForgetLocalDir(PathBuf),
    /// 在本地目录打开终端。
    OpenLocalTerminal(PathBuf),
    /// 切换到远程标签。
    SelectRemoteTab(usize),
    /// Toggle the optional local line editor for a remote terminal tab.
    ToggleLowLatencyShellInput(usize),
    /// 关闭远程标签。
    CloseRemoteTab(usize),
    /// 关闭除指定索引外的远程标签。
    CloseOtherRemoteTabs(usize),
    /// 关闭全部远程标签。
    CloseAllRemoteTabs,
    /// 切换到本地会话。
    SelectLocalSession(u64),
    /// 关闭本地会话。
    CloseLocalSession(u64),
    /// 关闭同目录下的其他本地会话。
    CloseOtherLocalSessions(u64),
    /// Execute a cwd-bound quick command in the active terminal or in a task.
    RunQuickCommand {
        scope: String,
        command: String,
        background: bool,
    },
    /// Open the command editor.
    EditQuickCommand { scope: String, command: String },
    /// Remove a command from the aggregate history.
    DeleteQuickCommand { scope: String, command: String },
    /// Exclude a command from cwd-bound history.
    IgnoreQuickCommand { scope: String, command: String },
    /// Stop one background command task.
    StopBackgroundTask(u64),
}

/// SFTP 面板菜单动作（由 SftpPane 自持分发）。
#[derive(Clone, Debug)]
pub enum SftpMenuAction {
    /// 进入子目录。
    Navigate(String),
    /// 下载文件。
    Download(String),
    /// 上传到该目录（进入目录并聚焦上传框）。
    UploadHere(String),
    /// 重命名条目。
    Rename(String),
    /// 删除条目。
    Delete { name: String, is_dir: bool },
    /// 在当前目录新建目录。
    NewDir,
    /// 刷新当前目录。
    Refresh,
}

/// 单个菜单项。
#[derive(Clone)]
pub struct MenuItem<A> {
    /// 稳定 id（用于 hit-test 定位）。
    pub id: String,
    /// 显示文案（已 i18n）。
    pub label: String,
    /// 右侧快捷键提示文字。
    pub shortcut_hint: Option<String>,
    pub disabled: bool,
    /// 危险操作（删除类）用红色。
    pub danger: bool,
    pub action: A,
}

/// 菜单条目：项或分隔线。
#[derive(Clone)]
pub enum MenuEntry<A> {
    Item(MenuItem<A>),
    /// A menu item with a persistent on/off check mark.
    CheckedItem {
        item: MenuItem<A>,
        checked: bool,
    },
    Separator,
}

/// 打开的上下文菜单快照。
#[derive(Clone)]
pub struct ContextMenuState<A> {
    /// 鼠标按下时的窗口坐标。
    pub position: Point<Pixels>,
    pub entries: Vec<MenuEntry<A>>,
}

/// 按条目数量估算菜单高度（定位钳制用）。
pub fn estimate_menu_height<A>(entries: &[MenuEntry<A>]) -> f32 {
    MENU_PADDING * 2.0
        + entries
            .iter()
            .map(|entry| match entry {
                MenuEntry::Item(_) | MenuEntry::CheckedItem { .. } => ITEM_HEIGHT,
                MenuEntry::Separator => SEPARATOR_HEIGHT,
            })
            .sum::<f32>()
}

/// 把菜单位置钳制到窗口内：默认向右下展开，越界则向左上翻转。
pub fn clamp_menu_position<A>(
    position: Point<Pixels>,
    window: &Window,
    entries: &[MenuEntry<A>],
) -> Point<Pixels> {
    let bounds = window.bounds();
    let mut x = position.x.as_f32();
    let mut y = position.y.as_f32();
    if x + CONTEXT_MENU_WIDTH > bounds.right().as_f32() {
        x = (x - CONTEXT_MENU_WIDTH).max(0.0);
    }
    let height = estimate_menu_height(entries);
    if y + height > bounds.bottom().as_f32() {
        y = (y - height).max(0.0);
    }
    Point::new(px(x), px(y))
}

/// 渲染 scrim + 菜单。scrim 覆盖拥有者根 div，点击任意处（左/右键）关闭。
///
/// `position` 为窗口坐标（即 `MouseDownEvent.position`）；`anchor` 是拥有者
/// 根 div 在窗口坐标系中的原点（AppShell 传 (0,0)），用于换算菜单的相对定位。
pub fn render_context_menu<A: Clone + 'static, T: 'static>(
    state: &ContextMenuState<A>,
    anchor: Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<T>,
    on_action: impl Fn(&mut T, A, &mut Window, &mut Context<T>) + 'static,
    on_dismiss: impl Fn(&mut T, &mut Context<T>) + 'static,
) -> AnyElement {
    let position = clamp_menu_position(state.position, window, &state.entries);
    let relative = Point::new(position.x - anchor.x, position.y - anchor.y);
    let on_action = Rc::new(on_action);
    let on_dismiss = Rc::new(on_dismiss);

    let scrim = div()
        .id("ctx-scrim")
        .absolute()
        .left_0()
        .top_0()
        .size_full()
        .occlude()
        .on_click({
            let on_dismiss = on_dismiss.clone();
            cx.listener(move |this, _ev, _window, cx| {
                on_dismiss(this, cx);
                cx.stop_propagation();
            })
        })
        .on_mouse_down(MouseButton::Right, {
            let on_dismiss = on_dismiss.clone();
            cx.listener(move |this, _ev, _window, cx| {
                on_dismiss(this, cx);
                cx.stop_propagation();
            })
        });

    let mut menu = div()
        .absolute()
        .left(px(relative.x.as_f32()))
        .top(px(relative.y.as_f32()))
        .w(px(CONTEXT_MENU_WIDTH))
        .p_1()
        .flex()
        .flex_col()
        .gap_1()
        .bg(theme::raised())
        .border_1()
        .border_color(theme::border_strong())
        .rounded(px(theme::RADIUS_SM))
        .shadow_md();

    for entry in &state.entries {
        let (item, checked, checkable) = match entry {
            MenuEntry::Separator => {
                menu = menu.child(div().h(px(1.)).mx_2().my_1().bg(theme::border()));
                continue;
            }
            MenuEntry::Item(item) => (item, false, false),
            MenuEntry::CheckedItem { item, checked } => (item, *checked, true),
        };

        let id = item.id.clone();
        let action = item.action.clone();
        let label = item.label.clone();
        let hint = item.shortcut_hint.clone();
        let disabled = item.disabled;
        let danger = item.danger;
        let mut row = div()
            .id(SharedString::from(format!("ctx-item-{id}")))
            .h(px(ITEM_HEIGHT))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(theme::RADIUS_SM))
            .text_xs()
            .text_color(if danger {
                theme::danger()
            } else if disabled {
                theme::faint_text()
            } else {
                theme::text()
            });
        if disabled {
            row = row.cursor_default();
        } else {
            row = row
                .cursor_pointer()
                .hover(|s| s.bg(theme::surface()))
                .on_click({
                    let on_action = on_action.clone();
                    cx.listener(move |this, _ev, window, cx| {
                        on_action(this, action.clone(), window, cx);
                    })
                });
        }
        if checkable {
            if checked {
                row = row.child(
                    icons::icon(icons::IconName::Check, 13.).text_color(if disabled {
                        theme::faint_text()
                    } else {
                        theme::accent()
                    }),
                );
            } else {
                row = row.child(div().w(px(13.)).h(px(13.)));
            }
        }
        row = row.child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(SharedString::from(label)),
        );
        if let Some(hint) = hint {
            row = row.child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(theme::faint_text())
                    .child(SharedString::from(hint)),
            );
        }
        menu = menu.child(row);
    }

    div()
        .absolute()
        .left_0()
        .top_0()
        .size_full()
        .child(scrim)
        .child(menu)
        .into_any_element()
}
