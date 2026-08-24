//! Git 历史提交图的纯布局算法。
//!
//! 该模块只处理提交父子关系到 lane/edge 的映射，渲染层可以用任意绘制后端
//! 展示结果。这样提交图的行为可以脱离 GPUI 做稳定测试。

use std::collections::BTreeSet;

use crate::git_history::CommitSummary;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryGraphEdge {
    pub from_lane: usize,
    pub to_lane: usize,
    pub color: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryGraphRow {
    pub node_lane: usize,
    pub node_color: usize,
    pub node_has_incoming: bool,
    pub incoming_edges: Vec<HistoryGraphEdge>,
    pub edges: Vec<HistoryGraphEdge>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveLane {
    commit_id: String,
    color: usize,
}

/// 为按拓扑顺序排列的提交生成稳定的图布局。
pub fn layout_history(entries: &[CommitSummary]) -> Vec<HistoryGraphRow> {
    let mut lanes = Vec::<ActiveLane>::new();
    let mut next_color = 0;
    let mut rows = Vec::with_capacity(entries.len());

    for entry in entries {
        let previous = lanes.clone();
        let existing_lane = lanes.iter().position(|lane| lane.commit_id == entry.id);
        let node_has_incoming = existing_lane.is_some();
        let node_lane = existing_lane.unwrap_or_else(|| {
            let color = next_color;
            next_color += 1;
            lanes.insert(
                0,
                ActiveLane {
                    commit_id: entry.id.clone(),
                    color,
                },
            );
            0
        });
        let before = lanes.clone();
        let node_color = before[node_lane].color;
        let incoming_edges = if node_has_incoming {
            Vec::new()
        } else {
            previous
                .iter()
                .enumerate()
                .filter_map(|(from_lane, lane)| {
                    before
                        .iter()
                        .position(|candidate| candidate.commit_id == lane.commit_id)
                        .map(|to_lane| HistoryGraphEdge {
                            from_lane,
                            to_lane,
                            color: lane.color,
                        })
                })
                .collect()
        };

        let mut next = before.clone();
        next.remove(node_lane);
        let insert_at = node_lane.min(next.len());
        let mut inserted = 0;
        let mut seen_parents = BTreeSet::new();
        for (parent_index, parent) in entry.parents.iter().enumerate() {
            if !seen_parents.insert(parent) || next.iter().any(|lane| lane.commit_id == *parent) {
                continue;
            }
            let color = if parent_index == 0 {
                node_color
            } else {
                let color = next_color;
                next_color += 1;
                color
            };
            let index = (insert_at + inserted).min(next.len());
            next.insert(
                index,
                ActiveLane {
                    commit_id: parent.clone(),
                    color,
                },
            );
            inserted += 1;
        }

        let mut edges = Vec::new();
        for (from_lane, lane) in before.iter().enumerate() {
            if from_lane == node_lane {
                for parent in &entry.parents {
                    let Some(to_lane) = next
                        .iter()
                        .position(|candidate| candidate.commit_id == *parent)
                    else {
                        continue;
                    };
                    edges.push(HistoryGraphEdge {
                        from_lane,
                        to_lane,
                        color: lane.color,
                    });
                }
            } else if let Some(to_lane) = next
                .iter()
                .position(|candidate| candidate.commit_id == lane.commit_id)
            {
                edges.push(HistoryGraphEdge {
                    from_lane,
                    to_lane,
                    color: lane.color,
                });
            }
        }

        rows.push(HistoryGraphRow {
            node_lane,
            node_color,
            node_has_incoming,
            incoming_edges,
            edges,
        });
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(id: &str, parents: &[&str]) -> CommitSummary {
        CommitSummary {
            id: id.to_string(),
            short_id: id.to_string(),
            author: "Author".to_string(),
            date: "2026-08-16T12:00:00+08:00".to_string(),
            subject: id.to_string(),
            parents: parents.iter().map(|parent| parent.to_string()).collect(),
        }
    }

    #[test]
    fn keeps_first_parent_in_the_current_lane_and_fans_out_merges() {
        let entries = vec![
            commit("merge", &["left", "right"]),
            commit("right", &["root"]),
            commit("left", &["root"]),
            commit("root", &[]),
        ];

        let rows = layout_history(&entries);

        assert_eq!(rows[0].node_lane, 0);
        assert!(!rows[0].node_has_incoming);
        assert_eq!(
            rows[0].edges,
            vec![
                HistoryGraphEdge {
                    from_lane: 0,
                    to_lane: 0,
                    color: 0,
                },
                HistoryGraphEdge {
                    from_lane: 0,
                    to_lane: 1,
                    color: 0,
                },
            ]
        );
        assert_eq!(rows[1].node_lane, 1);
        assert!(rows[1].node_has_incoming);
        assert_eq!(rows[2].node_lane, 0);
    }

    #[test]
    fn bridges_existing_lanes_when_a_new_history_head_is_inserted() {
        let rows = layout_history(&[
            commit("first-head", &["base"]),
            commit("second-head", &["side", "base"]),
            commit("side", &[]),
            commit("base", &[]),
        ]);

        assert_eq!(
            rows[1].incoming_edges,
            vec![HistoryGraphEdge {
                from_lane: 0,
                to_lane: 1,
                color: 0,
            }]
        );
    }

    #[test]
    fn starts_a_new_lane_for_disconnected_history_roots() {
        let rows = layout_history(&[commit("second-root", &[]), commit("first-root", &[])]);

        assert_eq!(rows[0].node_lane, 0);
        assert_eq!(rows[1].node_lane, 0);
        assert_ne!(rows[0].node_color, rows[1].node_color);
    }

    #[test]
    fn keeps_a_linear_history_on_one_color() {
        let rows = layout_history(&[
            commit("tip", &["middle"]),
            commit("middle", &["root"]),
            commit("root", &[]),
        ]);

        assert_eq!(
            rows.iter().map(|row| row.node_color).collect::<Vec<_>>(),
            vec![0, 0, 0]
        );
        assert!(
            rows.iter()
                .flat_map(|row| row.edges.iter())
                .all(|edge| edge.color == 0)
        );
    }
}
