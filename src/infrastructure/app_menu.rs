//! 原生应用菜单装配与应用级 action 路由。

use gpui::{App, Entity, KeyBinding, Menu, MenuItem, OsAction, Window};
use terminal as zed_terminal;

#[cfg(target_os = "macos")]
use gpui::SystemMenuType;

use crate::features::settings::{self, SettingsSection};
use crate::features::workspace::AppShell;
use crate::shared::i18n;
use crate::{
    About, CheckForUpdates, CloseActiveTab, CloseWindow, MinimizeWindow, NewTerminal, OpenProject,
    OpenSettings, Quit, ToggleCommandPalette, ToggleFullScreen, ToggleHostSidebar,
    ToggleTimestamps, ZoomWindow,
};

#[cfg(target_os = "macos")]
use crate::{Hide, HideOthers, ShowAll};

/// 注册应用级快捷键与处理器，并发布原生菜单。
pub(crate) fn install(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("shift-cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-m", MinimizeWindow, None),
        KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
        KeyBinding::new("cmd-o", OpenProject, Some("AppShell")),
        KeyBinding::new("cmd-t", NewTerminal, Some("AppShell")),
        KeyBinding::new("cmd-w", CloseActiveTab, Some("AppShell")),
        KeyBinding::new("cmd-k", ToggleCommandPalette, None),
        KeyBinding::new("ctrl-k", ToggleCommandPalette, None),
    ]);

    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
    ]);

    cx.on_action(|_: &About, cx| {
        defer_open_settings(SettingsSection::About, false, cx);
    });
    cx.on_action(|_: &CheckForUpdates, cx| {
        defer_open_settings(SettingsSection::Updates, true, cx);
    });
    cx.on_action(|_: &OpenSettings, cx| {
        defer_open_settings(SettingsSection::General, false, cx);
    });
    cx.on_action(|_: &CloseWindow, cx| defer_close_active_window(cx));
    cx.on_action(|_: &MinimizeWindow, cx| defer_active_window_action(cx, Window::minimize_window));
    cx.on_action(|_: &ZoomWindow, cx| defer_active_window_action(cx, Window::zoom_window));
    cx.on_action(|_: &ToggleFullScreen, cx| {
        defer_active_window_action(cx, Window::toggle_fullscreen)
    });

    #[cfg(target_os = "macos")]
    cx.on_action(|_: &Hide, cx| cx.hide());
    #[cfg(target_os = "macos")]
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    #[cfg(target_os = "macos")]
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());

    // 即使设置等辅助窗口处于前台，Quit 也统一交给 AppShell，避免绕过
    // 风险确认与资源清理。菜单 action 分发时当前窗口仍在 GPUI 更新栈中，
    cx.on_action(|_: &Quit, cx| {
        cx.defer(|cx| {
            if let Some(window) = find_main_window(cx) {
                let _ = window.update(cx, |shell, window, cx| shell.request_app_quit(window, cx));
            } else {
                cx.quit();
            }
        });
    });
    cx.on_action(|_: &ToggleCommandPalette, cx| {
        cx.defer(|cx| {
            if let Some(window) = find_main_window(cx) {
                let _ = window.update(cx, |shell, window, cx| {
                    shell.toggle_command_palette(window, cx)
                });
            }
        });
    });

    refresh(cx);
}

/// 应用语言切换后重建本地化菜单标签。
pub(crate) fn refresh(cx: &App) {
    let mut app_items = vec![
        MenuItem::action(i18n::text("app_menu.about"), About),
        MenuItem::action(i18n::text("app_menu.check_for_updates"), CheckForUpdates),
        MenuItem::separator(),
        MenuItem::action(i18n::text("app_menu.settings"), OpenSettings),
        MenuItem::separator(),
    ];

    #[cfg(target_os = "macos")]
    {
        app_items.push(MenuItem::os_submenu(
            i18n::text("app_menu.services"),
            SystemMenuType::Services,
        ));
        app_items.push(MenuItem::separator());
        app_items.push(MenuItem::action(i18n::text("app_menu.hide"), Hide));
        app_items.push(MenuItem::action(
            i18n::text("app_menu.hide_others"),
            HideOthers,
        ));
        app_items.push(MenuItem::action(i18n::text("app_menu.show_all"), ShowAll));
        app_items.push(MenuItem::separator());
    }

    app_items.push(MenuItem::action(i18n::text("quit.menu"), Quit));
    cx.set_menus([
        Menu::new("Crossh").items(app_items),
        Menu::new(i18n::text("app_menu.file")).items([
            MenuItem::action(i18n::text("app_menu.new_terminal"), NewTerminal),
            MenuItem::action(i18n::text("app_menu.open_project"), OpenProject),
            MenuItem::separator(),
            MenuItem::action(i18n::text("app_menu.close_tab"), CloseActiveTab),
            MenuItem::action(i18n::text("app_menu.close_window"), CloseWindow),
        ]),
        Menu::new(i18n::text("app_menu.edit")).items([
            MenuItem::os_action(
                i18n::text("app_menu.copy"),
                zed_terminal::Copy,
                OsAction::Copy,
            ),
            MenuItem::os_action(
                i18n::text("app_menu.paste"),
                zed_terminal::Paste,
                OsAction::Paste,
            ),
            MenuItem::os_action(
                i18n::text("app_menu.select_all"),
                zed_terminal::SelectAll,
                OsAction::SelectAll,
            ),
            MenuItem::separator(),
            MenuItem::action(i18n::text("app_menu.clear_terminal"), zed_terminal::Clear),
        ]),
        Menu::new(i18n::text("app_menu.view")).items([
            MenuItem::action(
                i18n::text("app_menu.toggle_host_sidebar"),
                ToggleHostSidebar,
            ),
            MenuItem::action(i18n::text("app_menu.toggle_timestamps"), ToggleTimestamps),
        ]),
        Menu::new(i18n::text("app_menu.window")).items([
            MenuItem::action(i18n::text("app_menu.minimize"), MinimizeWindow),
            MenuItem::action(i18n::text("app_menu.zoom"), ZoomWindow),
            MenuItem::separator(),
            MenuItem::action(i18n::text("app_menu.toggle_full_screen"), ToggleFullScreen),
        ]),
    ]);
}

fn defer_open_settings(section: SettingsSection, check_for_updates: bool, cx: &mut App) {
    cx.defer(move |cx| {
        let shell = ensure_main_shell(cx);
        if check_for_updates {
            let updates = shell.read(cx).updates.clone();
            updates.update(cx, |updates, cx| updates.check(cx));
        }
        settings::open_settings_section(shell.downgrade(), section, cx);
    });
}

fn defer_close_active_window(cx: &mut App) {
    let Some(active_window) = cx.active_window() else {
        return;
    };
    cx.defer(move |cx| {
        if let Some(main_window) = active_window.downcast::<AppShell>() {
            let _ = main_window.update(cx, |shell, window, cx| {
                shell.request_close_window(window, cx)
            });
        } else {
            let _ = active_window.update(cx, |_, window, _| window.remove_window());
            if let Some(shell) = find_main_shell(cx) {
                shell.update(cx, |_, cx| cx.notify());
            }
        }
    });
}

fn defer_active_window_action(cx: &mut App, action: fn(&Window)) {
    let Some(active_window) = cx.active_window() else {
        return;
    };
    cx.defer(move |cx| {
        let _ = active_window.update(cx, |_, window, _| action(window));
    });
}

fn ensure_main_shell(cx: &mut App) -> Entity<AppShell> {
    if let Some(shell) = find_main_shell(cx) {
        return shell;
    }

    crate::app::open_main_window(cx);
    find_main_shell(cx).expect("main window should contain an AppShell")
}

fn find_main_shell(cx: &App) -> Option<Entity<AppShell>> {
    find_main_window(cx).and_then(|handle| handle.entity(cx).ok())
}

fn find_main_window(cx: &App) -> Option<gpui::WindowHandle<AppShell>> {
    cx.windows()
        .iter()
        .find_map(|handle| handle.downcast::<AppShell>())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{
        AppContext, Context, IntoElement, Render, TestAppContext, Window, WindowOptions, actions,
        div,
    };

    actions!(app_menu_test, [ReadDispatchingWindow]);

    struct TestRoot;

    impl Render for TestRoot {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn deferred_menu_work_can_read_the_dispatching_window(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| TestRoot))
                .unwrap()
        });
        let did_read = Rc::new(Cell::new(false));
        let did_read_in_handler = did_read.clone();

        cx.update(|cx| {
            cx.on_action(move |_: &ReadDispatchingWindow, cx| {
                let did_read = did_read_in_handler.clone();
                cx.defer(move |cx| {
                    window
                        .entity(cx)
                        .expect("dispatching window should be readable after defer");
                    did_read.set(true);
                });
            });
        });

        cx.dispatch_action(window.into(), ReadDispatchingWindow);
        assert!(did_read.get());
    }
}
