//! 模态弹窗：主机密钥确认 / 凭据（口令、密码）输入。

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::features::workspace::AppShell;
use crate::infrastructure::ssh::{CredentialKind, HostKeyDecision};
use crate::shared::i18n;
use crate::shared::ui::widgets::printable_char;
use crate::shared::ui::{icons, theme};

/// 当前活动模态的显示快照。
pub enum PromptDisplay {
    None,
    HostKey {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
        changed: bool,
    },
    Credential {
        kind: CredentialKind,
        prompt: String,
    },
}

/// 渲染模态覆盖层。
pub fn render_prompt_modal(
    shell: &mut AppShell,
    prompt: PromptDisplay,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let modal_focus = shell.modal_focus.clone();
    let modal_icon = match &prompt {
        PromptDisplay::HostKey { .. } => icons::IconName::ShieldAlert,
        PromptDisplay::Credential { .. } => icons::IconName::KeyRound,
        PromptDisplay::None => return div().into_any_element(),
    };

    let (title, body, is_credential): (String, String, bool) = match &prompt {
        PromptDisplay::HostKey {
            host,
            port,
            key_type,
            fingerprint,
            changed,
        } => {
            let warning = if *changed {
                i18n::text("prompt.changed_host_key_warning")
            } else {
                i18n::text("prompt.unknown_host_warning")
            };
            (
                i18n::text("prompt.host_key_title"),
                rust_i18n::t!(
                    "prompt.host_key_body",
                    warning = warning,
                    host = host,
                    port = port,
                    key_type = key_type,
                    fingerprint = fingerprint
                )
                .to_string(),
                false,
            )
        }
        PromptDisplay::Credential { kind, prompt } => {
            let title = match kind {
                CredentialKind::Passphrase => i18n::text("prompt.passphrase_title"),
                CredentialKind::Password => i18n::text("prompt.password_title"),
            };
            (title, prompt.clone(), true)
        }
        PromptDisplay::None => unreachable!(),
    };

    let mut buttons = div().flex().flex_row().gap_2().mt_4();
    match prompt {
        PromptDisplay::HostKey { changed, .. } => {
            if !changed {
                buttons = buttons
                    .child(host_key_button(
                        cx,
                        i18n::text("prompt.accept_once"),
                        HostKeyDecision::AcceptOnce,
                    ))
                    .child(host_key_button(
                        cx,
                        i18n::text("prompt.accept_always"),
                        HostKeyDecision::AcceptAlways,
                    ));
            }
            buttons = buttons.child(host_key_button(
                cx,
                i18n::text("prompt.reject"),
                HostKeyDecision::Reject,
            ));
        }
        PromptDisplay::Credential { .. } => {
            buttons = buttons
                .child(cred_button(cx, i18n::text("prompt.confirm"), true))
                .child(cred_button(cx, i18n::text("prompt.cancel"), false));
        }
        PromptDisplay::None => {}
    }

    let mut card = div()
        .w(px(440.))
        .p_5()
        .bg(theme::surface())
        .border_1()
        .border_color(theme::border_strong())
        .rounded(px(theme::RADIUS_MD))
        .shadow_md()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::text())
                .child(icons::icon(modal_icon, 17.).text_color(if is_credential {
                    theme::info()
                } else {
                    theme::warning()
                }))
                .child(SharedString::from(title)),
        )
        .child(
            div()
                .mt_2()
                .text_xs()
                .text_color(theme::muted_text())
                .child(SharedString::from(body)),
        );

    if is_credential {
        let masked = "•".repeat(shell.prompt_input.chars().count());
        let input = div()
            .id("prompt-input")
            .w_full()
            .h(px(34.))
            .px_3()
            .flex()
            .items_center()
            .mt_2()
            .bg(theme::canvas())
            .border_1()
            .border_color(theme::border_strong())
            .rounded(px(theme::RADIUS_SM))
            .text_sm()
            .text_color(theme::text())
            .track_focus(&modal_focus)
            .on_key_down(cx.listener(handle_credential_key))
            .child(SharedString::from(masked));
        card = card.child(input);
    }
    card = card.child(buttons);

    div()
        .absolute()
        .size_full()
        .top_0()
        .left_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme::scrim())
        .child(card)
        .into_any_element()
}

fn handle_credential_key(
    this: &mut AppShell,
    ev: &KeyDownEvent,
    _: &mut Window,
    cx: &mut Context<AppShell>,
) {
    let ks = &ev.keystroke;
    match ks.key.as_str() {
        "enter" | "return" => {
            let val = std::mem::take(&mut this.prompt_input);
            this.resolve_credential(Some(val), cx);
        }
        "escape" => {
            this.resolve_credential(None, cx);
        }
        "backspace" => {
            this.prompt_input.pop();
            cx.notify();
        }
        _ => {
            if let Some(ch) = printable_char(ks) {
                this.prompt_input.push(ch);
                cx.notify();
            }
        }
    }
}

fn host_key_button(
    cx: &mut Context<AppShell>,
    label: String,
    decision: HostKeyDecision,
) -> impl IntoElement {
    let id = SharedString::from(label.clone());
    let icon = match decision {
        HostKeyDecision::Reject => icons::IconName::CircleX,
        HostKeyDecision::AcceptOnce | HostKeyDecision::AcceptAlways => icons::IconName::Check,
    };
    div()
        .id(id)
        .h(px(30.))
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(theme::RADIUS_SM))
        .text_xs()
        .cursor_pointer()
        .bg(if matches!(decision, HostKeyDecision::Reject) {
            theme::raised()
        } else {
            theme::accent()
        })
        .hover(|s| s.bg(theme::border_strong()))
        .text_color(if matches!(decision, HostKeyDecision::Reject) {
            theme::text()
        } else {
            theme::canvas()
        })
        .child(
            icons::icon(icon, 14.).text_color(if matches!(decision, HostKeyDecision::Reject) {
                theme::text()
            } else {
                theme::canvas()
            }),
        )
        .child(SharedString::from(label))
        .on_click(cx.listener(move |this, _ev, _w, cx| {
            this.resolve_host_key(decision, cx);
        }))
}

fn cred_button(cx: &mut Context<AppShell>, label: String, submit: bool) -> impl IntoElement {
    let id = SharedString::from(label.clone());
    div()
        .id(id)
        .h(px(30.))
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(theme::RADIUS_SM))
        .text_xs()
        .cursor_pointer()
        .bg(if submit {
            theme::accent()
        } else {
            theme::raised()
        })
        .hover(|s| s.bg(theme::border_strong()))
        .text_color(if submit {
            theme::canvas()
        } else {
            theme::text()
        })
        .child(
            icons::icon(
                if submit {
                    icons::IconName::Check
                } else {
                    icons::IconName::X
                },
                14.,
            )
            .text_color(if submit {
                theme::canvas()
            } else {
                theme::text()
            }),
        )
        .child(SharedString::from(label))
        .on_click(cx.listener(move |this, _ev, _w, cx| {
            if submit {
                let val = std::mem::take(&mut this.prompt_input);
                this.resolve_credential(Some(val), cx);
            } else {
                this.resolve_credential(None, cx);
            }
        }))
}
