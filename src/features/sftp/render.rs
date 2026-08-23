use super::*;
use crossh_core::format::format_bytes;

impl Render for SftpPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_requested {
            self.focus_requested = false;
            if let Some(editor) = &self.editor {
                editor.focus.focus(window, cx);
            } else {
                self.root_focus.focus(window, cx);
            }
        }
        if self.editor.is_some() {
            return self.render_editor(window, cx);
        }

        let cwd = self.cwd.clone();

        // 顶部：上级 / 当前路径 / 刷新。
        let top = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                Button::new("sftp-up")
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Ghost)
                    .icon(icons::icon(icons::IconName::ArrowUp, 14.))
                    .label(i18n::text("sftp.parent"))
                    .on_click(cx.listener(|this, _ev, _w, cx| {
                        let p = parent_of(&this.cwd);
                        this.request_list(p);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(cwd)),
            )
            .child(
                Button::new("sftp-refresh")
                    .size(ButtonSize::Icon(px(28.)))
                    .variant(ButtonVariant::Ghost)
                    .icon(
                        icons::icon(icons::IconName::RefreshCw, 14.)
                            .text_color(theme::muted_text()),
                    )
                    .on_click(cx.listener(|this, _ev, _w, cx| {
                        this.request_list(this.cwd.clone());
                        cx.notify();
                    })),
            );

        // 列表区。
        let mut list = scroll_y(&self.list_scroll)
            .id("sftp-entry-list")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .px_2()
            .py_2();
        let first_entry = ((self.list_scroll.offset().y.as_f32().max(0.) / SFTP_ROW_HEIGHT).floor()
            as usize)
            .min(self.entries.len());
        let visible_entries = (window.viewport_size().height.as_f32() / SFTP_ROW_HEIGHT).ceil()
            as usize
            + VIRTUAL_LIST_OVERSCAN;
        let last_entry = (first_entry + visible_entries).min(self.entries.len());
        list = list.child(
            div()
                .h(px(first_entry as f32 * SFTP_ROW_HEIGHT))
                .flex_shrink_0(),
        );
        for (idx, e) in self
            .entries
            .iter()
            .enumerate()
            .skip(first_entry)
            .take(last_entry.saturating_sub(first_entry))
        {
            let name = e.name.clone();
            let is_dir = e.is_dir;
            let size = if is_dir {
                String::new()
            } else {
                format_bytes(e.size)
            };
            let row = div()
                .id(("entry", idx))
                .flex()
                .flex_row()
                .flex_shrink_0()
                .items_center()
                .gap_2()
                .h(px(SFTP_ROW_HEIGHT))
                .px_2()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .hover(|s| s.bg(theme::surface()))
                .child(
                    icons::icon(
                        if is_dir {
                            icons::IconName::Folder
                        } else {
                            icons::IconName::FileText
                        },
                        15.,
                    )
                    .text_color(if is_dir {
                        theme::warning()
                    } else {
                        theme::muted_text()
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(theme::text())
                        .child(SharedString::from(name.clone())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::faint_text())
                        .child(SharedString::from(size)),
                )
                .on_click({
                    let name_click = name.clone();
                    cx.listener(move |this, _ev, _w, cx| {
                        if is_dir {
                            let p = join(&this.cwd, &name_click);
                            this.request_list(p);
                        } else {
                            this.open_file_or_download(&name_click, cx);
                        }
                        cx.notify();
                    })
                })
                .on_mouse_down(MouseButton::Right, {
                    let name_menu = name.clone();
                    cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                        let mut entries = vec![];
                        if is_dir {
                            entries.push(MenuEntry::Item(MenuItem {
                                id: "navigate".into(),
                                label: i18n::text("context_menu.open_folder"),
                                shortcut_hint: None,
                                disabled: false,
                                danger: false,
                                action: SftpMenuAction::Navigate(name_menu.clone()),
                            }));
                            entries.push(MenuEntry::Item(MenuItem {
                                id: "upload-here".into(),
                                label: i18n::text("context_menu.upload_here"),
                                shortcut_hint: None,
                                disabled: false,
                                danger: false,
                                action: SftpMenuAction::UploadHere(name_menu.clone()),
                            }));
                        } else {
                            entries.push(MenuEntry::Item(MenuItem {
                                id: "download".into(),
                                label: i18n::text("context_menu.download"),
                                shortcut_hint: None,
                                disabled: false,
                                danger: false,
                                action: SftpMenuAction::Download(name_menu.clone()),
                            }));
                        }
                        entries.push(MenuEntry::Separator);
                        entries.push(MenuEntry::Item(MenuItem {
                            id: "rename".into(),
                            label: i18n::text("context_menu.rename"),
                            shortcut_hint: None,
                            disabled: false,
                            danger: false,
                            action: SftpMenuAction::Rename(name_menu.clone()),
                        }));
                        entries.push(MenuEntry::Item(MenuItem {
                            id: "delete".into(),
                            label: i18n::text("context_menu.delete"),
                            shortcut_hint: None,
                            disabled: false,
                            danger: true,
                            action: SftpMenuAction::Delete {
                                name: name_menu.clone(),
                                is_dir,
                            },
                        }));
                        entries.push(MenuEntry::Separator);
                        entries.push(MenuEntry::Item(MenuItem {
                            id: "new-folder".into(),
                            label: i18n::text("context_menu.new_folder"),
                            shortcut_hint: None,
                            disabled: false,
                            danger: false,
                            action: SftpMenuAction::NewDir,
                        }));
                        entries.push(MenuEntry::Item(MenuItem {
                            id: "refresh".into(),
                            label: i18n::text("context_menu.refresh"),
                            shortcut_hint: None,
                            disabled: false,
                            danger: false,
                            action: SftpMenuAction::Refresh,
                        }));
                        this.open_context_menu(ev.position, entries, cx);
                    })
                });
            list = list.child(row);
        }
        list = list.child(
            div()
                .h(px(
                    (self.entries.len() - last_entry) as f32 * SFTP_ROW_HEIGHT
                ))
                .flex_shrink_0(),
        );

        // 底部：上传输入 + 进度/消息。
        let input = TextInput::new("sftp-upload-input", self.focus.clone())
            .flex_1()
            .value(self.upload_input.value.clone())
            .placeholder(i18n::text("sftp.local_path_placeholder"))
            .ime_marked_text(self.upload_input.ime_marked_text.clone())
            .caret_height(px(15.))
            .text_xs()
            .text_color(theme::text())
            .focus_visible_accent()
            .entity(cx.entity())
            .on_key_down(cx.listener(SftpPane::handle_input_key));

        let mut bottom = div()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .bg(theme::surface())
            .border_t_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(input)
                    .child(
                        div()
                            .id("sftp-upload-btn")
                            .h(px(28.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded(px(theme::RADIUS_SM))
                            .cursor_pointer()
                            .bg(theme::accent())
                            .hover(|s| s.bg(theme::accent_hover()))
                            .text_xs()
                            .text_color(theme::canvas())
                            .child(
                                icons::icon(icons::IconName::Upload, 14.)
                                    .text_color(theme::canvas()),
                            )
                            .child(SharedString::from(i18n::text("sftp.upload")))
                            .on_click(cx.listener(|this, _ev, _w, cx| {
                                this.do_upload(cx);
                            })),
                    )
                    .child(
                        div()
                            .id("sftp-choose-file")
                            .h(px(28.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded(px(theme::RADIUS_SM))
                            .cursor_pointer()
                            .bg(theme::raised())
                            .hover(|s| s.bg(theme::border_strong()))
                            .text_xs()
                            .text_color(theme::text())
                            .child(
                                icons::icon(icons::IconName::FolderOpen, 14.)
                                    .text_color(theme::text()),
                            )
                            .child(SharedString::from(i18n::text("sftp.choose_file")))
                            .on_click(cx.listener(|this, _ev, _w, cx| {
                                this.choose_upload_file(cx);
                            })),
                    ),
            );

        if let Some(p) = &self.progress {
            let pct = p
                .total
                .filter(|&t| t > 0)
                .map(|t| ((p.transferred as f64 / t as f64) * 100.0) as u32)
                .unwrap_or(0);
            bottom = bottom.child(div().text_xs().text_color(theme::info()).child(
                SharedString::from(format!(
                    "{}: {} / {} ({}%)",
                    p.label,
                    format_bytes(p.transferred),
                    p.total.map(format_bytes).unwrap_or_else(|| "?".into()),
                    pct
                )),
            ));
        } else if self.loading {
            bottom = bottom.child(
                div()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(i18n::text("sftp.loading"))),
            );
        } else if let Some(msg) = &self.message {
            bottom = bottom.child(
                div()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(msg.clone())),
            );
        }

        // 全尺寸 canvas：在 prepaint 阶段捕获根 div 的窗口坐标 bounds，
        // 并在 paint 阶段注册菜单的窗口级外部点击监听。
        let anchor = self.anchor_bounds.clone();
        let context_menu_open = self.context_menu.is_some();
        let context_menu_weak = cx.entity().downgrade();
        let bounds_canvas = canvas(
            {
                let anchor = anchor.clone();
                move |bounds, _window, _cx| {
                    anchor.set(Some(bounds));
                    bounds
                }
            },
            move |_bounds, _state, window, _cx| {
                if !context_menu_open {
                    return;
                }
                let weak = context_menu_weak.clone();
                let anchor = anchor.clone();
                window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
                    if !matches!(phase, gpui::DispatchPhase::Capture) {
                        return;
                    }
                    let closed = weak
                        .update(cx, |this, _| {
                            let outside = anchor
                                .get()
                                .is_some_and(|bounds| !bounds.contains(&ev.position));
                            if this.context_menu.is_some() && outside {
                                this.context_menu = None;
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false);
                    if closed {
                        // 外部点击只关菜单，不再触发下方控件。
                        cx.stop_propagation();
                        window.refresh();
                    }
                });
            },
        )
        .absolute()
        .left_0()
        .top_0()
        .size_full();

        let mut root = div()
            .id("sftp-pane")
            .relative()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .track_focus(&self.root_focus)
            .tab_stop(true)
            .on_key_down(cx.listener(SftpPane::handle_root_key))
            .child(bounds_canvas)
            .child(top)
            .child(list)
            .child(bottom);

        if let Some(menu) = self.context_menu.clone() {
            let anchor = self
                .anchor_bounds
                .get()
                .map(|bounds| bounds.origin)
                .unwrap_or_else(|| Point::new(px(0.), px(0.)));
            root = root.child(render_context_menu(
                &menu,
                anchor,
                window,
                cx,
                |this, action, window, cx| this.dispatch_menu_action(action, window, cx),
                |this, cx| this.close_context_menu(cx),
            ));
        }
        root = root.child(self.render_path_input_modal(window, cx));
        root = root.child(self.render_delete_confirm(cx));
        root.into_any_element()
    }
}
