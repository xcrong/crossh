//! 模态弹窗：主机密钥确认 / 凭据（口令、密码）输入。

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::features::workspace::AppShell;
use crate::shared::i18n;
use crossh_ssh::{CredentialKind, HostKeyDecision};
use crossh_ui::widgets::{ime_input_canvas, printable_char, text_caret};
use crossh_ui::{icons, theme};
use crossh_ui_component::{Button, ButtonSize, ButtonVariant, ModalDialog};

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
    window: &Window,
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
            // 变更密钥时只提供「本次接受」：引擎侧 AcceptOnce 不写 known_hosts，
            // AcceptAlways 变更路径等同拒绝，因此不再展示该按钮。
            let mut accept_once = div().child(host_key_button(
                cx,
                i18n::text("prompt.accept_once"),
                HostKeyDecision::AcceptOnce,
            ));
            if !changed {
                accept_once = accept_once.child(host_key_button(
                    cx,
                    i18n::text("prompt.accept_always"),
                    HostKeyDecision::AcceptAlways,
                ));
            }
            buttons = accept_once.child(host_key_button(
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

    let mut modal = ModalDialog::new(
        title,
        icons::icon(modal_icon, 17.).text_color(if is_credential {
            theme::info()
        } else {
            theme::warning()
        }),
    )
    .body(body);

    if is_credential {
        let masked = "•".repeat(shell.prompt_input.chars().count());
        let ime_marked_text = shell.prompt_ime_marked_text.clone();
        let input_focused = modal_focus.is_focused(window);
        let mut input = div()
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
            .tab_stop(true)
            .relative()
            .on_click({
                let modal_focus = modal_focus.clone();
                move |_ev, window, cx| window.focus(&modal_focus, cx)
            })
            .on_key_down(cx.listener(handle_credential_key));
        if !masked.is_empty() {
            input = input.child(SharedString::from(masked));
        }
        if input_focused {
            input = input.child(text_caret(px(16.)));
        }
        if !ime_marked_text.is_empty() {
            input = input.child(
                div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .underline()
                    .text_decoration_color(theme::accent())
                    .child(SharedString::from(
                        "•".repeat(ime_marked_text.chars().count()),
                    )),
            );
        }
        input = input.child(ime_input_canvas(modal_focus, cx.entity()));
        modal = modal.child(input);
    }
    modal = modal.child(buttons);

    modal.into_any_element()
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
            this.prompt_ime_marked_text.clear();
            cx.notify();
        }
        _ => {
            if let Some(ch) = printable_char(ks) {
                this.prompt_input.push(ch);
                this.prompt_ime_marked_text.clear();
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
    let reject = matches!(decision, HostKeyDecision::Reject);
    Button::new(id)
        .size(ButtonSize::Medium)
        .variant(if reject {
            ButtonVariant::Default
        } else {
            ButtonVariant::Primary
        })
        .icon(icons::icon(icon, 14.).text_color(if reject {
            theme::text()
        } else {
            theme::canvas()
        }))
        .label(label)
        .on_click(cx.listener(move |this, _ev, _w, cx| {
            this.resolve_host_key(decision, cx);
        }))
}

fn cred_button(cx: &mut Context<AppShell>, label: String, submit: bool) -> impl IntoElement {
    let id = SharedString::from(label.clone());
    Button::new(id)
        .size(ButtonSize::Medium)
        .variant(if submit {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Default
        })
        .icon(
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
        .label(label)
        .on_click(cx.listener(move |this, _ev, _w, cx| {
            if submit {
                let val = std::mem::take(&mut this.prompt_input);
                this.resolve_credential(Some(val), cx);
            } else {
                this.resolve_credential(None, cx);
            }
        }))
}
