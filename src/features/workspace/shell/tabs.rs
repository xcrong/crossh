//! AppShell terminal tab and session navigation.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossh_terminal::ConnState;
use gpui::{PromptButton, PromptLevel, Window};

use super::*;
use crate::features::workspace::local_paths::{current_local_cwd, normalize_local_cwd};
use crate::features::workspace::modal_editor::{DefaultCommandEditor, RenameEditor};
use crate::features::workspace::pinned::{next_pin_id, pinned_tabs_for_project};
use crate::features::workspace::settings::PinnedLocalTab;

/// 单个标签页关闭时可能被打断的活动；任何一项存在都需要确认。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TabCloseRisk {
    command_running: bool,
    unsaved_editors: usize,
}

/// 分栏右窗格会话退出分栏时的去向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SplitPaneRetirement {
    /// 无活动：直接销毁。分栏是当前 Tab 的临时工作区，退出即清理。
    Close,
    /// 有活动（命令运行中/传输中/未保存等）：保留为普通标签，不打断用户。
    KeepAsTab,
}

/// 决策右窗格会话退出分栏的去向。`None` 表示会话已不存在，统一按关闭
/// 处理（关闭操作对不存在的会话是安全的空操作）。
pub(super) fn split_pane_retirement(risk: Option<&TabCloseRisk>) -> SplitPaneRetirement {
    match risk {
        Some(risk) if risk.needs_confirmation() => SplitPaneRetirement::KeepAsTab,
        _ => SplitPaneRetirement::Close,
    }
}

impl TabCloseRisk {
    pub(crate) fn needs_confirmation(&self) -> bool {
        self.command_running || self.unsaved_editors > 0
    }

    fn detail(&self) -> String {
        let mut lines = Vec::new();
        if self.command_running {
            lines.push(i18n::text("tab_close.running"));
        }
        if self.unsaved_editors > 0 {
            lines.push(
                rust_i18n::t!("tab_close.unsaved_editors", count = self.unsaved_editors)
                    .to_string(),
            );
        }
        lines.push(String::new());
        lines.push(i18n::text("tab_close.consequence"));
        lines.join("\n")
    }
}

impl AppShell {
    pub(super) fn handle_new_terminal(
        &mut self,
        _: &crate::NewTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_tab(window, cx);
    }

    pub(super) fn handle_close_active_tab(
        &mut self,
        _: &crate::CloseActiveTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_close_active_tab(window, cx);
    }

    pub(super) fn switch_tab(&mut self, idx: usize, cx: &mut Context<Self>) {
        match self.workspace.active_view {
            Some(ActiveView::LocalSession(_session_id)) => {
                let ids: Vec<LocalSessionId> = self.workspace.sessions.local_sessions.keys().cloned().collect();
                if idx < ids.len() {
                    self.select_local_session(ids[idx], cx);
                }
            }
            None => {}
        }
    }

    /// 关闭活动标签；有命令运行等风险时先弹确认框。
    pub(super) fn request_close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ActiveView::LocalSession(session_id)) = self.workspace.active_view {
            self.request_close_local_session(session_id, window, cx);
        }
    }

    /// 关闭单个本地会话；存在活动风险时先请求确认。
    pub(crate) fn request_close_local_session(
        &mut self,
        session_id: LocalSessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_close_local_session_with_keep_pinned(session_id, window, cx, false);
    }

    pub(crate) fn request_close_local_session_with_keep_pinned(
        &mut self,
        session_id: LocalSessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
        keep_pinned: bool,
    ) {
        let Some(risk) = self.local_session_close_risk(session_id, cx) else {
            return;
        };
        if !risk.needs_confirmation() {
            self.close_local_session_internal(session_id, keep_pinned, cx);
            return;
        }
        self.prompt_close_tab(risk, window, cx, move |this, cx| {
            this.close_local_session_internal(session_id, keep_pinned, cx);
        });
    }

    pub(crate) fn local_session_close_risk(
        &self,
        session_id: LocalSessionId,
        cx: &Context<Self>,
    ) -> Option<TabCloseRisk> {
        let session = self.workspace.sessions.local_sessions.get(&session_id)?;
        Some(TabCloseRisk {
            command_running: session.terminal.read(cx).is_command_running(cx),
            ..TabCloseRisk::default()
        })
    }

    /// 一键停止项目：关闭该项目下全部本地会话，但保留 recent/pinned（契约 2）。
    /// 批量快照 `Vec<LocalSessionId>` 后 `detach_splits_for` 再逐个经
    /// `request_close_local_session` 风险确认（契约 5），有风险的会话取消后保留。
    pub(crate) fn stop_local_project(
        &mut self,
        project_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project_dir) = normalize_local_cwd(project_dir) else {
            return;
        };
        let ids: Vec<LocalSessionId> = match self.workspace.sessions.local_dirs.get(&project_dir) {
            Some(dir) => dir.sessions.clone(),
            None => return,
        };
        if ids.is_empty() {
            return;
        }
        let views: Vec<ActiveView> = ids.iter().copied().map(ActiveView::LocalSession).collect();
        self.detach_splits_for(&views, cx);
        for session_id in ids {
            if !self
                .workspace
                .sessions
                .local_sessions
                .contains_key(&session_id)
            {
                continue;
            }
            self.request_close_local_session_with_keep_pinned(session_id, window, cx, true);
        }
    }

    fn prompt_close_tab(
        &mut self,
        risk: TabCloseRisk,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_confirm: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        if self.tab_close_confirmation_open
            || self.quit_confirmation_open
            || self.shutdown_in_progress
        {
            return;
        }
        self.tab_close_confirmation_open = true;
        let answers = [
            PromptButton::ok(i18n::text("tab_close.confirm")),
            PromptButton::cancel(i18n::text("tab_close.cancel")),
        ];
        let answer = window.prompt(
            PromptLevel::Warning,
            &i18n::text("tab_close.title"),
            Some(&risk.detail()),
            &answers,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let confirmed = answer.await == Ok(0);
            let _ = this.update(cx, |this, cx| {
                this.tab_close_confirmation_open = false;
                if confirmed {
                    on_confirm(this, cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn cycle_tab(&mut self, direction: isize, cx: &mut Context<Self>) {
        let Some(active) = self.workspace.active_view else {
            return;
        };
        let ActiveView::LocalSession(active_id) = active;
        let ids: Vec<LocalSessionId> = self.workspace.sessions.local_sessions.keys().cloned().collect();
        if ids.is_empty() {
            return;
        }
        let current_idx = ids.iter().position(|id| *id == active_id).unwrap_or(0);
        let next_idx = (current_idx as isize + direction).rem_euclid(ids.len() as isize) as usize;
        self.select_local_session(ids[next_idx], cx);
    }

    /// 从当前标签复制一个终端标签；没有活动标签时把焦点放到快速连接框。
    pub(crate) fn new_tab(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ActiveView::LocalSession(session_id)) = self.workspace.active_view
            && let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) {
                let project_dir = session.project_dir.clone();
                let cwd = session.cwd.clone();
                let _ = self.open_local_session(project_dir, cwd, cx);
                return;
            }
        let cwd = current_local_cwd();
        let _ = self.open_local_session(cwd.clone(), cwd, cx);
    }

    pub(super) fn local_session_cwd(
        &self,
        session_id: LocalSessionId,
        cx: &Context<Self>,
    ) -> PathBuf {
        self.workspace
            .sessions
            .local_sessions
            .get(&session_id)
            .map(|session| {
                session
                    .terminal
                    .read(cx)
                    .cwd
                    .as_deref()
                    .and_then(|cwd| normalize_local_cwd(PathBuf::from(cwd)))
                    .unwrap_or_else(|| session.cwd.clone())
            })
            .unwrap_or_else(current_local_cwd)
    }

    pub(super) fn local_session_project_dir(&self, session_id: LocalSessionId) -> PathBuf {
        self.workspace
            .sessions
            .local_sessions
            .get(&session_id)
            .map(|session| session.project_dir.clone())
            .unwrap_or_else(current_local_cwd)
    }

    /// 关闭同一目录下的其他本地会话（保留 `keep`）。
    pub(super) fn close_other_local_sessions(
        &mut self,
        keep: LocalSessionId,
        cx: &mut Context<Self>,
    ) {
        let Some(others) = self.local_dir_for_session(keep).map(|dir| {
            dir.sessions
                .iter()
                .copied()
                .filter(|id| *id != keep)
                .collect::<Vec<_>>()
        }) else {
            return;
        };
        let swept = others
            .iter()
            .copied()
            .map(ActiveView::LocalSession)
            .collect::<Vec<_>>();
        self.detach_splits_for(&swept, cx);
        for session_id in others {
            self.close_local_session(session_id, cx);
        }
        self.select_local_session(keep, cx);
    }

    pub(crate) fn pin_local_session(&mut self, session_id: LocalSessionId, cx: &mut Context<Self>) {
        let Some(session) = self.workspace.sessions.local_sessions.get_mut(&session_id) else {
            return;
        };
        if session.pin_id.is_some() {
            return;
        }
        let pin_id = next_pin_id(&self.workspace_settings.pinned_local_tabs);
        session.pin_id = Some(pin_id);
        self.workspace_settings
            .pinned_local_tabs
            .push(PinnedLocalTab {
                pin_id,
                project_dir: session.project_dir.clone(),
                cwd: self.local_session_cwd(session_id, cx),
                custom_name: None,
                default_command: None,
            });
        self.persist_settings();
        cx.notify();
    }

    /// 取消固定：会话保持打开但回到普通标签行为，持久化记录移除。
    pub(crate) fn unpin_local_session(
        &mut self,
        session_id: LocalSessionId,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.workspace.sessions.local_sessions.get_mut(&session_id) else {
            return;
        };
        let Some(pin_id) = session.pin_id.take() else {
            return;
        };
        self.workspace_settings
            .pinned_local_tabs
            .retain(|tab| tab.pin_id != pin_id);
        self.persist_settings();
        cx.notify();
    }

    /// 打开固定标签的重命名弹窗（初始值取当前自定义名称；空白表示回退默认标题）。
    /// 与 Quick Command 编辑器互斥，打开时静默关闭另一个模态。
    pub(crate) fn open_rename_local_session(
        &mut self,
        session_id: LocalSessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) else {
            return;
        };
        let focus = cx.focus_handle();
        self.quick_command_editor = None;
        self.rename_editor = Some(RenameEditor::new(
            session_id,
            session.custom_name.clone().unwrap_or_default(),
            focus.clone(),
        ));
        window.focus(&focus, cx);
        cx.notify();
    }

    /// 提交重命名：空白清除名称（契约 4），否则覆盖并持久化到固定记录。
    pub(crate) fn submit_rename_local_session(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.rename_editor.take() else {
            return;
        };
        let name = editor.state.value.clone();
        let Some(session) = self
            .workspace
            .sessions
            .local_sessions
            .get_mut(&editor.session_id)
        else {
            // 弹窗期间会话已关闭：静默忽略，不写持久化。
            cx.notify();
            return;
        };
        let custom_name = (!name.trim().is_empty()).then(|| name.trim().to_string());
        session.custom_name = custom_name.clone();
        if let Some(pin_id) = session.pin_id
            && let Some(tab) = self
                .workspace_settings
                .pinned_local_tabs
                .iter_mut()
                .find(|tab| tab.pin_id == pin_id)
        {
            tab.custom_name = custom_name;
            self.persist_settings();
        }
        cx.notify();
    }

    pub(crate) fn cancel_rename_local_session(&mut self, cx: &mut Context<Self>) {
        if self.rename_editor.take().is_some() {
            cx.notify();
        }
    }

    /// 打开默认命令编辑弹窗（初始值取当前 default_command；空白表示清除）。
    /// 与其他模态互斥。
    pub(crate) fn open_default_command_editor(
        &mut self,
        session_id: LocalSessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) else {
            return;
        };
        if session.pin_id.is_none() {
            return;
        }
        let focus = cx.focus_handle();
        self.quick_command_editor = None;
        self.rename_editor = None;
        self.default_command_editor = Some(DefaultCommandEditor::new(
            session_id,
            session.default_command.clone().unwrap_or_default(),
            focus.clone(),
        ));
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn submit_default_command(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.default_command_editor.take() else {
            return;
        };
        let raw = editor.state.value.clone();
        let trimmed = raw.trim().to_string();
        let new_command = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        };
        let Some(session) = self
            .workspace
            .sessions
            .local_sessions
            .get_mut(&editor.session_id)
        else {
            cx.notify();
            return;
        };
        session.default_command = new_command.clone();
        if let Some(pin_id) = session.pin_id
            && let Some(tab) = self
                .workspace_settings
                .pinned_local_tabs
                .iter_mut()
                .find(|tab| tab.pin_id == pin_id)
        {
            tab.default_command = new_command;
            // normalized 会在 persist 前 trim+空白归一，但此处已处理
            self.persist_settings();
        }
        cx.notify();
    }

    pub(crate) fn cancel_default_command(&mut self, cx: &mut Context<Self>) {
        if self.default_command_editor.take().is_some() {
            cx.notify();
        }
    }

    /// 重载默认命令到终端；空闲时才执行（契约 5/6）。
    pub(crate) fn reload_default_command(
        &mut self,
        session_id: LocalSessionId,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) else {
            return;
        };
        let Some(cmd) = session.default_command.clone() else {
            return;
        };
        if cmd.trim().is_empty() {
            return;
        }
        if session.pin_id.is_none() {
            return;
        }
        if session.terminal.read(cx).is_command_running(cx) {
            return;
        }
        session.terminal.update(cx, |terminal, terminal_cx| {
            terminal.run_command(&cmd, terminal_cx)
        });
        cx.notify();
    }

    pub(crate) fn clear_default_command(
        &mut self,
        session_id: LocalSessionId,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.workspace.sessions.local_sessions.get_mut(&session_id) else {
            return;
        };
        if session.pin_id.is_none() {
            return;
        }
        if session.default_command.is_none() {
            return;
        }
        session.default_command = None;
        if let Some(pin_id) = session.pin_id
            && let Some(tab) = self
                .workspace_settings
                .pinned_local_tabs
                .iter_mut()
                .find(|tab| tab.pin_id == pin_id)
        {
            tab.default_command = None;
            self.persist_settings();
        }
        cx.notify();
    }

    /// 按固定记录顺序重开会话（契约 11 恢复路径）：把 `pin_id` 与自定义
    /// 名称应用到**本次恢复创建的新会话**（显式会话 id），目录已失效的
    /// 记录跳过并即时从持久化设置清理（契约 11 Rev-4）。
    pub(super) fn restore_pinned_local_tabs(
        &mut self,
        tabs: Vec<PinnedLocalTab>,
        cx: &mut Context<Self>,
    ) {
        let mut removed_stale = false;
        for tab in tabs {
            let pin_id = tab.pin_id;
            let custom_name = tab.custom_name.clone();
            let default_command = tab.default_command.clone();
            let Some(session_id) =
                self.open_local_session(tab.project_dir.clone(), tab.cwd.clone(), cx)
            else {
                // 记录目录已失效（删除/改名/不可访问）：跳过恢复并即时
                // 清理，等价契约 8 的关闭清理，不等待下次启动。
                self.workspace_settings
                    .pinned_local_tabs
                    .retain(|entry| entry.pin_id != pin_id);
                removed_stale = true;
                continue;
            };
            self.apply_pin_to_session(session_id, pin_id, custom_name, default_command.clone(), cx);
            // 自动执行默认命令（契约 4）：恢复后若配置了 default_command，延迟到终端 Connected 再执行
            // `open_local_session` 创建的 TerminalView 初始为 Connecting（display-only），立即 send_input 会丢失；
            // 需等待 Zed TerminalBuilder 完成 attach 后再投递。
            if let Some(cmd) = default_command
                && !cmd.trim().is_empty()
            {
                let cmd = cmd.trim().to_string();
                if let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) {
                    let terminal = session.terminal.clone();
                    cx.spawn(async move |weak, cx| {
                        // 最多等待 4s，轮询终端是否已 Connected 且空闲
                        for _ in 0..40 {
                            cx.background_executor()
                                .timer(Duration::from_millis(100))
                                .await;
                            let ready = weak
                                .update(cx, |this, cx| {
                                    this.workspace
                                        .sessions
                                        .local_sessions
                                        .get(&session_id)
                                        .is_some_and(|s| {
                                            s.terminal.entity_id() == terminal.entity_id()
                                                && s.terminal.read(cx).state == ConnState::Connected
                                                && !s.terminal.read(cx).is_command_running(cx)
                                        })
                                })
                                .unwrap_or(false);
                            if ready {
                                let _ = weak.update(cx, |this, cx| {
                                    if let Some(s) =
                                        this.workspace.sessions.local_sessions.get(&session_id)
                                    {
                                        s.terminal
                                            .update(cx, |t, term_cx| t.run_command(&cmd, term_cx));
                                    }
                                });
                                break;
                            }
                        }
                    })
                    .detach();
                }
            }
        }
        if removed_stale {
            self.persist_settings();
        }
    }

    /// 激活项目时恢复该项目尚无会话的固定记录（契约 11）。
    /// 幂等：已有对应 `pin_id` 会话的记录（用户手动固定或先前已恢复）
    /// 不重复打开，也不改写既有会话内容。
    pub(super) fn restore_pinned_tabs_for_project(
        &mut self,
        project_dir: &Path,
        cx: &mut Context<Self>,
    ) {
        let open_pin_ids = self
            .workspace
            .sessions
            .local_sessions
            .values()
            .filter_map(|session| session.pin_id)
            .collect::<BTreeSet<_>>();
        let pending =
            pinned_tabs_for_project(&self.workspace_settings.pinned_local_tabs, project_dir)
                .into_iter()
                .filter(|tab| !open_pin_ids.contains(&tab.pin_id))
                .cloned()
                .collect::<Vec<_>>();
        self.restore_pinned_local_tabs(pending, cx);
    }

    /// 把固定记录的状态应用到指定的本地会话（恢复路径专用；会话创建由
    /// `open_local_session` 完成）。以显式 `session_id` 为目标，不依赖
    /// 「当前活动会话」，避免失效记录污染其他会话（契约 11 Rev-4）。
    pub(super) fn apply_pin_to_session(
        &mut self,
        session_id: LocalSessionId,
        pin_id: u64,
        custom_name: Option<String>,
        default_command: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.workspace.sessions.local_sessions.get_mut(&session_id) {
            session.pin_id = Some(pin_id);
            session.custom_name = custom_name;
            // default_command 已做 trim/空白归一
            session.default_command = default_command
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty());
            cx.notify();
        }
    }

    pub(super) fn first_local_view(&self) -> Option<ActiveView> {
        self.workspace
            .sessions
            .local_dirs
            .values()
            .find_map(|dir| dir.active_session.map(ActiveView::LocalSession))
    }

    /// 把焦点交还给当前活动终端 tab（切换 tab / 关闭模态后调用）。
    pub(crate) fn refocus_active_terminal(&self, cx: &mut Context<Self>) {
        if let Some(ActiveView::LocalSession(session_id)) = self.workspace.active_view {
            if let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) {
                session.terminal.update(cx, |terminal, _| {
                    terminal.request_focus();
                });
            }
        } else if let Some(session_id) = self.workspace.sessions.local_sessions.keys().next().cloned()
            && let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) {
                session.terminal.update(cx, |terminal, _| {
                    terminal.request_focus();
                });
            }
    }
}

#[cfg(test)]
mod tests {
    use crate::shared::i18n;

    use super::{SplitPaneRetirement, TabCloseRisk, split_pane_retirement};

    #[test]
    fn close_confirmation_is_only_required_for_material_activity() {
        assert!(!TabCloseRisk::default().needs_confirmation());

        for risk in [
            TabCloseRisk {
                command_running: true,
                ..Default::default()
            },
            TabCloseRisk {
                unsaved_editors: 1,
                ..Default::default()
            },
        ] {
            assert!(risk.needs_confirmation());
        }
    }

    #[test]
    fn close_risk_detail_describes_what_will_be_interrupted() {
        let detail = TabCloseRisk {
            command_running: true,
            unsaved_editors: 1,
        }
        .detail();
        assert!(detail.contains(&i18n::text("tab_close.consequence")));
        assert!(detail.contains(&i18n::text("tab_close.running")));
    }

    #[test]
    fn split_pane_retirement_destroys_idle_panes_and_keeps_busy_ones() {
        assert_eq!(split_pane_retirement(None), SplitPaneRetirement::Close);
        assert_eq!(
            split_pane_retirement(Some(&TabCloseRisk::default())),
            SplitPaneRetirement::Close
        );
        for risky in [
            TabCloseRisk {
                command_running: true,
                ..Default::default()
            },
            TabCloseRisk {
                unsaved_editors: 1,
                ..Default::default()
            },
        ] {
            assert_eq!(
                split_pane_retirement(Some(&risky)),
                SplitPaneRetirement::KeepAsTab
            );
        }
    }
}
