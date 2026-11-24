//! AppShell 分屏动作：递归分屏树的方向导航、拆分与折叠。
//!
//! 分屏树归 daemon；本模块只把按键翻译成 `SplitPane`/`ClosePane`/`ActivatePane`，
//! 纯几何函数（`directional_neighbor`）不依赖 `gpui` 便于测试。

#[cfg(test)]
use std::collections::BTreeMap;

use gpui::Window;

use super::*;
use crate::features::workspace::projection;
use crossh_session::workspace::{Axis, PaneNodeSnapshot, SplitId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// 以父容器为单位的相对矩形（x/y/w/h 均为 0..1 比例）。
#[derive(Clone, Copy, Debug)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn full() -> Self {
        Self {
            x: 0.,
            y: 0.,
            w: 1.,
            h: 1.,
        }
    }
}

fn collect_rects(node: &PaneNodeSnapshot, rect: Rect, out: &mut Vec<(PaneId, Rect)>) {
    match node {
        PaneNodeSnapshot::Leaf(pane) => out.push((pane.id, rect)),
        PaneNodeSnapshot::Split {
            axis,
            ratio,
            first,
            second,
            ..
        } => {
            let ratio = projection::clamp_split_ratio(*ratio);
            match axis {
                Axis::Row => {
                    let first_width = rect.w * ratio;
                    collect_rects(
                        first,
                        Rect {
                            w: first_width,
                            ..rect
                        },
                        out,
                    );
                    collect_rects(
                        second,
                        Rect {
                            x: rect.x + first_width,
                            w: rect.w - first_width,
                            ..rect
                        },
                        out,
                    );
                }
                Axis::Column => {
                    let first_height = rect.h * ratio;
                    collect_rects(
                        first,
                        Rect {
                            h: first_height,
                            ..rect
                        },
                        out,
                    );
                    collect_rects(
                        second,
                        Rect {
                            y: rect.y + first_height,
                            h: rect.h - first_height,
                            ..rect
                        },
                        out,
                    );
                }
            }
        }
    }
}

fn center(rect: Rect) -> (f32, f32) {
    (rect.x + rect.w / 2., rect.y + rect.h / 2.)
}

/// 按几何位置找方向上的相邻 pane；没有相邻 pane 时返回 `None`。
pub(crate) fn directional_neighbor(
    layout: &PaneNodeSnapshot,
    active: PaneId,
    direction: Direction,
) -> Option<PaneId> {
    let mut rects = Vec::new();
    collect_rects(layout, Rect::full(), &mut rects);
    let (_, active_rect) = rects.iter().find(|(id, _)| *id == active)?;
    let active_rect = *active_rect;
    let (active_x, active_y) = center(active_rect);

    let mut best: Option<(PaneId, f32, f32)> = None;
    for (pane_id, rect) in &rects {
        if *pane_id == active {
            continue;
        }
        let (x, y) = center(*rect);
        let usable = match direction {
            Direction::Left => rect.x + rect.w <= active_rect.x + f32::EPSILON,
            Direction::Right => rect.x >= active_rect.x + active_rect.w - f32::EPSILON,
            Direction::Up => rect.y + rect.h <= active_rect.y + f32::EPSILON,
            Direction::Down => rect.y >= active_rect.y + active_rect.h - f32::EPSILON,
        };
        if !usable {
            continue;
        }
        // 主轴距离优先，次轴距离用于同列/同行内择近。
        let (primary, secondary) = match direction {
            Direction::Left => (active_rect.x - (rect.x + rect.w), (active_y - y).abs()),
            Direction::Right => (
                rect.x - (active_rect.x + active_rect.w),
                (active_y - y).abs(),
            ),
            Direction::Up => (active_rect.y - (rect.y + rect.h), (active_x - x).abs()),
            Direction::Down => (
                rect.y - (active_rect.y + active_rect.h),
                (active_x - x).abs(),
            ),
        };
        let candidate = (primary.abs(), secondary);
        let replace = best.is_none_or(|(_, best_primary, best_secondary)| {
            candidate.0 < best_primary - f32::EPSILON
                || ((candidate.0 - best_primary).abs() <= f32::EPSILON
                    && candidate.1 < best_secondary)
        });
        if replace {
            best = Some((*pane_id, candidate.0, candidate.1));
        }
    }
    best.map(|(pane_id, _, _)| pane_id)
}

/// 拆分目标：优先活跃 pane；没有活跃 pane 时回退到首个叶子。
///
/// 分屏动作在已有布局里继续拆分，不折叠其它 pane。
pub(crate) fn split_target(layout: &PaneNodeSnapshot, active: Option<PaneId>) -> Option<PaneId> {
    active.or_else(|| layout.leaves().next().map(|pane| pane.id))
}

impl AppShell {
    pub(crate) fn handle_split_terminal_horizontal(
        &mut self,
        _: &crate::features::workspace::shell::SplitTerminalHorizontal,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_terminal_horizontal(cx);
    }

    pub(crate) fn handle_split_terminal_vertical(
        &mut self,
        _: &crate::features::workspace::shell::SplitTerminalVertical,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_terminal_vertical(cx);
    }

    pub(crate) fn handle_focus_split_left(
        &mut self,
        _: &crate::features::workspace::shell::FocusSplitLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_split_focus(Direction::Left, cx);
    }

    pub(crate) fn handle_focus_split_right(
        &mut self,
        _: &crate::features::workspace::shell::FocusSplitRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_split_focus(Direction::Right, cx);
    }

    pub(crate) fn handle_focus_split_up(
        &mut self,
        _: &crate::features::workspace::shell::FocusSplitUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_split_focus(Direction::Up, cx);
    }

    pub(crate) fn handle_focus_split_down(
        &mut self,
        _: &crate::features::workspace::shell::FocusSplitDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_split_focus(Direction::Down, cx);
    }

    /// 快捷键与命令面板：始终在活跃 pane 上继续拆分（已有分屏时形成嵌套）。
    pub(crate) fn split_terminal_horizontal(&mut self, cx: &mut Context<Self>) {
        self.split_active_pane(Axis::Row, cx);
    }

    pub(crate) fn split_terminal_vertical(&mut self, cx: &mut Context<Self>) {
        self.split_active_pane(Axis::Column, cx);
    }

    fn split_active_pane(&mut self, axis: Axis, cx: &mut Context<Self>) {
        // Scratch 悬浮时不拆背后终端：焦点在临时终端内，cmd-d 冒泡到 #app-shell
        // 也应直接丢弃，避免误拆工作区分屏。
        if self.scratch_visible {
            return;
        }
        let Some(tab) = self.workspace.active_tab() else {
            return;
        };
        let Some(pane_id) = split_target(&tab.layout, tab.active_pane) else {
            return;
        };
        self.send_workspace_op(
            WorkspaceOp::SplitPane {
                pane_id,
                axis,
                cwd: None,
            },
            cx,
        );
    }

    /// 左右分屏按钮：已有分屏时折叠为一个 pane，否则把活跃 pane 一分为二。
    pub(crate) fn toggle_terminal_split(&mut self, cx: &mut Context<Self>) {
        self.toggle_split(Axis::Row, cx);
    }

    pub(crate) fn toggle_vertical_split(&mut self, cx: &mut Context<Self>) {
        self.toggle_split(Axis::Column, cx);
    }

    fn toggle_split(&mut self, axis: Axis, cx: &mut Context<Self>) {
        if self.scratch_visible {
            return;
        }
        let Some(tab) = self.workspace.active_tab() else {
            return;
        };
        let pane_ids = tab.layout.leaves().map(|pane| pane.id).collect::<Vec<_>>();
        if pane_ids.len() > 1 {
            let Some(active) = tab.active_pane else {
                return;
            };
            for pane_id in pane_ids.into_iter().filter(|pane_id| *pane_id != active) {
                self.send_workspace_op(WorkspaceOp::ClosePane { pane_id }, cx);
            }
            return;
        }
        if let Some(pane_id) = tab.active_pane.or_else(|| pane_ids.first().copied()) {
            self.send_workspace_op(
                WorkspaceOp::SplitPane {
                    pane_id,
                    axis,
                    cwd: None,
                },
                cx,
            );
        }
    }

    pub(crate) fn move_split_focus(&mut self, direction: Direction, cx: &mut Context<Self>) {
        if self.scratch_visible {
            return;
        }
        let Some(tab) = self.workspace.active_tab() else {
            return;
        };
        let Some(active) = tab.active_pane else {
            return;
        };
        let target = directional_neighbor(&tab.layout, active, direction);
        if let Some(pane_id) = target {
            self.focus_pane(pane_id, cx);
        }
    }

    /// 分屏比例提交：拖拽结束时调用。
    pub(crate) fn commit_split_ratio(
        &mut self,
        split_id: SplitId,
        ratio: f32,
        cx: &mut Context<Self>,
    ) {
        let ratio = projection::clamp_split_ratio(ratio);
        let unchanged = self
            .workspace
            .snapshot
            .projects
            .iter()
            .flat_map(|project| &project.tabs)
            .any(|tab| match &tab.layout {
                PaneNodeSnapshot::Split {
                    id, ratio: current, ..
                } if *id == split_id => (*current - ratio).abs() < f32::EPSILON,
                _ => false,
            });
        if unchanged {
            return;
        }
        self.workspace.split_drag_cell.set(None);
        self.send_workspace_op(WorkspaceOp::SetSplitRatio { split_id, ratio }, cx);
    }
}

/// 每个叶子 pane 的比例矩形，供几何测试使用。
#[cfg(test)]
pub(crate) fn pane_rects(layout: &PaneNodeSnapshot) -> BTreeMap<PaneId, (f32, f32, f32, f32)> {
    let mut rects = Vec::new();
    collect_rects(layout, Rect::full(), &mut rects);
    rects
        .into_iter()
        .map(|(pane_id, rect)| (pane_id, (rect.x, rect.y, rect.w, rect.h)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: u64) -> PaneNodeSnapshot {
        PaneNodeSnapshot::Leaf(crossh_session::workspace::PaneSnapshot {
            id: PaneId(id),
            cwd: "/tmp".into(),
            size: crossh_session::workspace::TermSize::default(),
            session: None,
            cwd_available: true,
            status: crossh_session::workspace::PaneStatus::Dormant {
                reason: crossh_session::workspace::PaneStatusReason::NotStarted,
            },
        })
    }

    fn split(
        id: u64,
        axis: Axis,
        ratio: f32,
        first: PaneNodeSnapshot,
        second: PaneNodeSnapshot,
    ) -> PaneNodeSnapshot {
        PaneNodeSnapshot::Split {
            id: SplitId(id),
            axis,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    #[test]
    fn directional_neighbor_follows_split_geometry() {
        let layout = split(1, Axis::Row, 0.5, leaf(1), leaf(2));
        assert_eq!(
            directional_neighbor(&layout, PaneId(1), Direction::Right),
            Some(PaneId(2))
        );
        assert_eq!(
            directional_neighbor(&layout, PaneId(2), Direction::Left),
            Some(PaneId(1))
        );
        assert_eq!(
            directional_neighbor(&layout, PaneId(1), Direction::Left),
            None
        );
        assert_eq!(
            directional_neighbor(&layout, PaneId(1), Direction::Down),
            None
        );

        let layout = split(
            2,
            Axis::Column,
            0.5,
            split(3, Axis::Row, 0.5, leaf(1), leaf(2)),
            split(4, Axis::Row, 0.5, leaf(3), leaf(4)),
        );
        assert_eq!(
            directional_neighbor(&layout, PaneId(1), Direction::Down),
            Some(PaneId(3))
        );
        assert_eq!(
            directional_neighbor(&layout, PaneId(4), Direction::Up),
            Some(PaneId(2))
        );
        assert_eq!(
            directional_neighbor(&layout, PaneId(3), Direction::Right),
            Some(PaneId(4))
        );
    }

    #[test]
    fn split_target_continues_inside_existing_layout() {
        let layout = split(
            2,
            Axis::Row,
            0.5,
            leaf(1),
            split(3, Axis::Column, 0.5, leaf(2), leaf(3)),
        );
        assert_eq!(split_target(&layout, Some(PaneId(2))), Some(PaneId(2)));
        assert_eq!(split_target(&layout, Some(PaneId(3))), Some(PaneId(3)));
        assert_eq!(split_target(&layout, None), Some(PaneId(1)));
        assert_eq!(split_target(&leaf(9), None), Some(PaneId(9)));
    }

    #[test]
    fn pane_rects_cover_the_container_without_overlap() {
        let layout = split(
            2,
            Axis::Row,
            0.25,
            leaf(1),
            split(3, Axis::Column, 0.5, leaf(2), leaf(3)),
        );
        let rects = pane_rects(&layout);
        assert_eq!(rects[&PaneId(1)], (0., 0., 0.25, 1.));
        assert!((rects[&PaneId(2)].0 - 0.25).abs() < 1e-5);
        assert!((rects[&PaneId(2)].3 - 0.5).abs() < 1e-5);
        assert!((rects[&PaneId(3)].1 - 0.5).abs() < 1e-5);
    }
}
