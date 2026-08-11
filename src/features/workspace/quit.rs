//! AppShell shutdown confirmation and cleanup.

use gpui::{PromptButton, PromptLevel};

use super::*;

#[derive(Clone, Copy)]
enum ExitIntent {
    QuitApp,
    CloseWindow,
}

#[derive(Default)]
struct QuitRiskSummary {
    running_commands: usize,
    sftp_writes: usize,
    unsaved_editors: usize,
    active_forwards: usize,
}

impl QuitRiskSummary {
    fn needs_confirmation(&self) -> bool {
        self.running_commands > 0
            || self.sftp_writes > 0
            || self.unsaved_editors > 0
            || self.active_forwards > 0
    }

    fn detail(&self) -> String {
        let mut lines = vec![i18n::text("quit.warning")];
        if self.running_commands > 0 {
            lines.push(rust_i18n::t!("quit.commands", count = self.running_commands).to_string());
        }
        if self.sftp_writes > 0 {
            lines.push(rust_i18n::t!("quit.transfers", count = self.sftp_writes).to_string());
        }
        if self.unsaved_editors > 0 {
            lines.push(
                rust_i18n::t!("quit.unsaved_editors", count = self.unsaved_editors).to_string(),
            );
        }
        if self.active_forwards > 0 {
            lines.push(rust_i18n::t!("quit.forwards", count = self.active_forwards).to_string());
        }
        lines.push(String::new());
        lines.push(i18n::text("quit.cleanup"));
        lines.join("\n")
    }
}

impl AppShell {
    pub(super) fn handle_quit(
        &mut self,
        _: &crate::Quit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_app_quit(window, cx);
    }

    pub(crate) fn request_app_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_exit(ExitIntent::QuitApp, window, cx);
    }

    pub(crate) fn request_close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.should_close_window(window, cx) {
            window.remove_window();
        }
    }

    pub(super) fn should_close_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.shutdown_in_progress {
            return true;
        }
        if !self.quit_risks(cx).needs_confirmation() {
            self.begin_shutdown(cx);
            return true;
        }
        self.request_exit(ExitIntent::CloseWindow, window, cx);
        false
    }

    fn request_exit(&mut self, intent: ExitIntent, window: &mut Window, cx: &mut Context<Self>) {
        if self.shutdown_in_progress || self.quit_confirmation_open {
            return;
        }

        let risks = self.quit_risks(cx);
        if !risks.needs_confirmation() {
            self.begin_shutdown(cx);
            match intent {
                ExitIntent::QuitApp => cx.quit(),
                ExitIntent::CloseWindow => window.remove_window(),
            }
            return;
        }

        self.quit_confirmation_open = true;
        let answers = [
            PromptButton::ok(i18n::text("quit.confirm")),
            PromptButton::cancel(i18n::text("quit.cancel")),
        ];
        let answer = window.prompt(
            PromptLevel::Warning,
            &i18n::text("quit.title"),
            Some(&risks.detail()),
            &answers,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let confirmed = answer.await == Ok(0);
            let _ = this.update(cx, |this, cx| {
                this.quit_confirmation_open = false;
                if confirmed {
                    this.begin_shutdown(cx);
                }
            });
            if !confirmed {
                return;
            }

            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let _ = cx.update(|window, cx| match intent {
                ExitIntent::QuitApp => cx.quit(),
                ExitIntent::CloseWindow => window.remove_window(),
            });
        })
        .detach();
    }

    fn quit_risks(&self, cx: &Context<Self>) -> QuitRiskSummary {
        let mut risks = QuitRiskSummary {
            running_commands: self.background_tasks.running_count(),
            ..QuitRiskSummary::default()
        };
        for session in self.workspace.sessions.local_sessions.values() {
            if session.terminal.read(cx).is_command_running(cx) {
                risks.running_commands += 1;
            }
        }
        for tab in &self.workspace.sessions.remote_tabs {
            if tab.pane.is_command_running(cx) {
                risks.running_commands += 1;
            }
            let pane_risk = tab.pane.risk(cx);
            risks.sftp_writes += pane_risk.sftp_writes;
            risks.unsaved_editors += pane_risk.unsaved_editors;
            risks.active_forwards += pane_risk.active_forwards;
        }
        risks
    }

    pub(super) fn begin_shutdown(&mut self, cx: &mut Context<Self>) {
        if self.shutdown_in_progress {
            return;
        }
        self.shutdown_in_progress = true;
        self.status = Some(i18n::text("quit.closing"));
        let running_background = self
            .background_tasks
            .tasks
            .values()
            .filter(|task| {
                matches!(
                    task.status,
                    crossh_core::commands::BackgroundTaskStatus::Running
                        | crossh_core::commands::BackgroundTaskStatus::Stopping
                )
            })
            .map(|task| task.id)
            .collect::<Vec<_>>();
        for id in running_background {
            self.stop_background_task(id, cx);
        }

        let terminals = self
            .workspace
            .sessions
            .local_sessions
            .values()
            .map(|session| session.terminal.clone())
            .collect::<Vec<_>>();

        for tab in &self.workspace.sessions.remote_tabs {
            tab.pane.request_close(cx);
        }
        for terminal in terminals {
            terminal.update(cx, |terminal, terminal_cx| {
                terminal.request_close(terminal_cx)
            });
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::QuitRiskSummary;

    #[test]
    fn quit_confirmation_is_only_required_for_material_activity() {
        assert!(!QuitRiskSummary::default().needs_confirmation());

        for risks in [
            QuitRiskSummary {
                running_commands: 1,
                ..Default::default()
            },
            QuitRiskSummary {
                sftp_writes: 1,
                ..Default::default()
            },
            QuitRiskSummary {
                unsaved_editors: 1,
                ..Default::default()
            },
            QuitRiskSummary {
                active_forwards: 1,
                ..Default::default()
            },
        ] {
            assert!(risks.needs_confirmation());
        }
    }
}
