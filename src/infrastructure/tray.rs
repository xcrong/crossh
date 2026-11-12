//! Menu-bar tray (macOS only): daemon 可见性。
//!
//! - 图标常驻菜单栏：绿 = daemon 可达，灰 = 未运行；首项显示存活会话数。
//! - 菜单：状态行（禁用）、打开终端窗口、停止 daemon（停完连 GUI 一起退，
//!   daemon 没了 GUI 就是空壳）。
//! - 纯观察 + 停止：tray 永不自动拉起 daemon（`connect_to` 只连不建，
//!   `ensure_daemon` 的拉起语义这里禁用）；拉起仍走按需建会话与 CLI。
//! - `tray-icon`/`muda` 的句柄是 `!Send`（主线程亲和）：全部状态机跑在
//!   GPUI 主线程任务里（2s timer + `MenuEvent::try_recv`），网络阻塞段
//!   （connect/list/stop）交 `background_executor`，不卡 UI。
//! - Linux 暂无 tray（不引 GTK/AppIndicator 系统依赖），Windows 后续跟进，
//!   见 `Cargo.toml` 的 target 门控与本模块的 `cfg`。

use std::time::Duration;

use gpui::App;
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuId, MenuItem, MenuItemBuilder, PredefinedMenuItem},
};

use crossh_session::{Client, instance::read_daemon_addr};

/// 存活轮询间隔：`ls` 级别的轻请求，2s 一次不 disturb daemon。
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// 主线程调用（`NSStatusItem` 要求主线程建；`app.run` 闭包里调）。
/// 失败只记日志，绝不阻塞应用启动。
pub(crate) fn spawn(cx: &mut App) {
    let status = MenuItemBuilder::new()
        .id(MenuId::new("status"))
        .text(status_stopped())
        .enabled(false)
        .build();
    let open = MenuItemBuilder::new()
        .id(MenuId::new("open"))
        .text(rust_i18n::t!("tray.open_window").to_string())
        .build();
    let stop = MenuItemBuilder::new()
        .id(MenuId::new("stop"))
        .text(rust_i18n::t!("tray.stop_daemon").to_string())
        .enabled(false)
        .build();
    let menu = Menu::new();
    if menu.append(&status).is_err()
        || menu.append(&PredefinedMenuItem::separator()).is_err()
        || menu.append(&open).is_err()
        || menu.append(&stop).is_err()
    {
        log::warn!("tray: menu build failed");
        return;
    }
    let icon = match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(status_stopped())
        .with_icon(status_icon(false))
        .build()
    {
        Ok(icon) => icon,
        Err(error) => {
            log::warn!("tray: icon build failed: {error}");
            return;
        }
    };

    cx.spawn(async move |cx| {
        let mut state = State {
            icon,
            status,
            stop,
            last: None,
        };
        loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            // 菜单事件排空（`try_recv` 不阻塞主线程）。
            {
                let events = MenuEvent::receiver();
                while let Ok(event) = events.try_recv() {
                    state.on_menu_event(event, cx);
                }
            }
            // 网络段放 background 线程，主线程只做状态呈现。
            let snapshot = cx
                .background_executor()
                .spawn(async move { State::snapshot() })
                .await;
            state.refresh(snapshot);
        }
    })
    .detach();
}

fn status_stopped() -> String {
    rust_i18n::t!("tray.stopped").to_string()
}

fn status_running(alive: usize) -> String {
    rust_i18n::t!("tray.running", count = alive).to_string()
}

/// 品牌 glyph（`assets/appicon/icon-master.svg` 的准星本体，透明底）：
/// 薄荷绿 = daemon 可达，灰 = 未运行。几何按 master 等比缩到 32px
///（圆环 r=210/1024、线宽 64/1024、中心圆点 r=44/1024）。
fn status_icon(running: bool) -> Icon {
    const SIZE: usize = 32;
    const S: f32 = SIZE as f32 / 1024.0;
    const CENTER: f32 = 512.0 * S;
    const RING_R: f32 = 210.0 * S;
    const HALF_STROKE: f32 = 64.0 * S / 2.0;
    const DOT_R: f32 = 44.0 * S;
    // 准星四 tick 起止（master 坐标），顺序 上/下/左/右。
    const TICKS: [((f32, f32), (f32, f32)); 4] = [
        ((512.0, 170.0), (512.0, 262.0)),
        ((512.0, 762.0), (512.0, 854.0)),
        ((170.0, 512.0), (262.0, 512.0)),
        ((762.0, 512.0), (854.0, 512.0)),
    ];
    let (r, g, b) = if running {
        (0x69, 0xD7, 0xB0)
    } else {
        (0x8E, 0x8E, 0x93)
    };
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let on_ring = ((px - CENTER).hypot(py - CENTER) - RING_R).abs() <= HALF_STROKE;
            let on_dot = (px - CENTER).hypot(py - CENTER) <= DOT_R;
            let on_tick = TICKS.iter().any(|&(a, b)| {
                dist_to_segment(px, py, (a.0 * S, a.1 * S), (b.0 * S, b.1 * S)) <= HALF_STROKE
            });
            if on_ring || on_dot || on_tick {
                let i = (y * SIZE + x) * 4;
                rgba[i] = r;
                rgba[i + 1] = g;
                rgba[i + 2] = b;
                rgba[i + 3] = 0xFF;
            }
        }
    }
    Icon::from_rgba(rgba, SIZE as u32, SIZE as u32).expect("tray icon rgba")
}

/// 点到线段距离（tray glyph tick 光栅化用）。
fn dist_to_segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let t = ((px - a.0) * dx + (py - a.1) * dy) / (dx * dx + dy * dy);
    let t = t.clamp(0.0, 1.0);
    (px - (a.0 + t * dx)).hypot(py - (a.1 + t * dy))
}

struct State {
    icon: TrayIcon,
    status: MenuItem,
    stop: MenuItem,
    last: Option<usize>,
}

impl State {
    /// 只连不建：无端口文件 / 连不上 = 未运行，不触发拉起。
    /// 跑在 background 线程，可阻塞。
    fn snapshot() -> Option<usize> {
        let addr = read_daemon_addr()?;
        crate::app::bootstrap::runtime().block_on(async {
            let mut client = Client::connect_to(&addr).await.ok()?;
            let sessions = client.list().await.ok()?;
            Some(sessions.iter().filter(|s| s.alive).count())
        })
    }

    fn refresh(&mut self, current: Option<usize>) {
        if current == self.last {
            return;
        }
        self.last = current;
        match current {
            Some(alive) => {
                let text = status_running(alive);
                self.status.set_text(text.clone());
                let _ = self.icon.set_tooltip(Some(text));
                let _ = self.icon.set_icon(Some(status_icon(true)));
                self.stop.set_enabled(true);
            }
            None => {
                self.status.set_text(status_stopped());
                let _ = self.icon.set_tooltip(Some(status_stopped()));
                let _ = self.icon.set_icon(Some(status_icon(false)));
                self.stop.set_enabled(false);
            }
        }
    }

    fn on_menu_event(&mut self, event: MenuEvent, cx: &gpui::AsyncApp) {
        match event.id.as_ref() {
            "open" => {
                // 主实例已运行（菜单只在主实例存在时才有）：新进程的
                // `try_forward` 会把它变成"给运行实例开窗"后退出。
                let exe =
                    std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("crossh"));
                let _ = std::process::Command::new(exe)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
            "stop" => {
                // daemon 没了 GUI 就是空壳：停完连 GUI 一起退。
                // `daemon stop` 含 10s 等待确认：阻塞段扔 background主线程不卡；
                // quit 挂前台任务（AsyncApp 非 Send进不了 background）。
                let stopping = cx.background_executor().spawn(async move {
                    if read_daemon_addr().is_some() {
                        let _ = crate::app::bootstrap::runtime()
                            .block_on(crossh_session::daemon_stop());
                    }
                });
                let foreground = cx.foreground_executor().clone();
                let handle = cx.clone();
                foreground
                    .spawn(async move {
                        stopping.await;
                        handle.update(|cx| cx.quit());
                    })
                    .detach();
            }
            _ => {}
        }
    }
}
