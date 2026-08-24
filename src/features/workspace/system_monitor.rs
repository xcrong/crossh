use gpui::{
    AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, Window, div, px,
};

use crossh_core::system_stats::SystemSnapshot;
use crossh_ui::theme;

use crate::features::workspace::shell::AppShell;

const CARD_WIDTH: f32 = 280.0;
const BAR_HEIGHT: f32 = 6.0;

pub(crate) fn render_system_monitor_card(
    shell: &AppShell,
    _window: &Window,
    _cx: &mut Context<AppShell>,
) -> Option<AnyElement> {
    if !shell.system_monitor.visible {
        return None;
    }
    let snapshot = shell.system_monitor.snapshot.clone();

    let card = div()
        .absolute()
        .bottom(px(theme::STATUS_BAR_HEIGHT + 8.))
        .right(px(10.))
        .w(px(CARD_WIDTH))
        .max_w(px(CARD_WIDTH))
        .flex()
        .flex_col()
        .gap_3()
        .p_3()
        .rounded(px(theme::RADIUS_MD))
        .bg(theme::raised())
        .border_1()
        .border_color(theme::border())
        .child(render_header())
        .child(render_cpu(&snapshot))
        .child(render_memory(&snapshot))
        .child(render_disk(&snapshot))
        .child(render_network(&snapshot));

    Some(card.into_any_element())
}

fn render_header() -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::muted_text())
                .child(SharedString::from("System Monitor")),
        )
        .child(
            div()
                .text_size(px(10.))
                .text_color(theme::faint_text())
                .child(SharedString::from("local")),
        )
        .into_any_element()
}

fn section_label(title: &str) -> AnyElement {
    div()
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme::text())
        .child(SharedString::from(title.to_string()))
        .into_any_element()
}

fn bar(percent: Option<f32>) -> AnyElement {
    let pct = percent.unwrap_or(0.0).clamp(0.0, 100.0);
    div()
        .w_full()
        .h(px(BAR_HEIGHT))
        .rounded(px(3.))
        .bg(theme::surface())
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .rounded(px(3.))
                .bg(theme::accent())
                .w(px(CARD_WIDTH * pct / 100.0 * 0.92)), // 0.92 留边距
        )
        .into_any_element()
}

fn value_text(text: String) -> AnyElement {
    div()
        .text_size(px(11.))
        .text_color(theme::muted_text())
        .child(SharedString::from(text))
        .into_any_element()
}

fn placeholder() -> String {
    "--".to_string()
}

fn format_percent(v: Option<f32>) -> String {
    v.map(|p| format!("{:.1}%", p)).unwrap_or_else(placeholder)
}

fn format_gb(bytes: u64) -> String {
    let gb = bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    format!("{:.1} GB", gb)
}

fn format_bytes(bytes: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.0} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

fn format_rate(rate: Option<u64>) -> String {
    match rate {
        None => placeholder(),
        Some(b) => {
            let s = format_bytes(b);
            format!("{}/s", s)
        }
    }
}

fn render_cpu(snapshot: &Option<SystemSnapshot>) -> AnyElement {
    let (usage_str, load_str, pct) = match snapshot {
        Some(s) => {
            let u = s
                .cpu_usage
                .map(|v| format!("{:.1}%", v))
                .unwrap_or_else(placeholder);
            let l = s
                .load_avg
                .as_ref()
                .map(|la| format!("{:.2} / {:.2} / {:.2}", la.one, la.five, la.fifteen))
                .unwrap_or_else(placeholder);
            (u, l, s.cpu_usage)
        }
        None => (placeholder(), placeholder(), None),
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(section_label("CPU"))
                .child(value_text(usage_str)),
        )
        .child(bar(pct))
        .child(
            div().flex().items_center().justify_between().child(
                div()
                    .text_size(px(10.))
                    .text_color(theme::faint_text())
                    .child(SharedString::from(format!("Load {}", load_str))),
            ),
        )
        .into_any_element()
}

fn render_memory(snapshot: &Option<SystemSnapshot>) -> AnyElement {
    let (used_str, pct) = match snapshot {
        Some(s) => {
            let used = format!(
                "{} / {}",
                format_gb(s.memory_used),
                format_gb(s.memory_total)
            );
            (used, s.memory_usage_percent)
        }
        None => (placeholder(), None),
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(section_label("Memory"))
                .child(value_text(format_percent(pct))),
        )
        .child(bar(pct))
        .child(value_text(used_str))
        .into_any_element()
}

fn render_disk(snapshot: &Option<SystemSnapshot>) -> AnyElement {
    let (used_str, pct) = match snapshot {
        Some(s) => match (s.disk_used, s.disk_total) {
            (Some(used), Some(total)) => {
                let txt = format!(
                    "{} used, {} free",
                    format_bytes(used),
                    format_bytes(total - used)
                );
                (txt, s.disk_usage_percent)
            }
            _ => (placeholder(), None),
        },
        None => (placeholder(), None),
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(section_label("Disk"))
                .child(value_text(format_percent(pct))),
        )
        .child(bar(pct))
        .child(value_text(used_str))
        .into_any_element()
}

fn render_network(snapshot: &Option<SystemSnapshot>) -> AnyElement {
    let (down_str, up_str) = match snapshot {
        Some(s) => (
            format_rate(s.network_rx_rate),
            format_rate(s.network_tx_rate),
        ),
        None => (placeholder(), placeholder()),
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(section_label("Network"))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(value_text("Down".to_string()))
                        .child(value_text(down_str)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(value_text("Up".to_string()))
                        .child(value_text(up_str)),
                ),
        )
        .into_any_element()
}
