//! Timestamp metadata for terminal rows.
//!
//! This state deliberately lives beside the terminal contracts instead of in
//! the renderer. It tracks terminal rows without changing PTY input/output or
//! the terminal emulator's cell grid.

use chrono::Local;

const MAX_TRACKED_ROWS: usize = 100_000;
const ALIGNMENT_SEARCH_RADIUS: usize = 1_024;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TerminalRow {
    pub text: String,
    pub has_content: bool,
}

impl TerminalRow {
    pub fn new(mut text: String) -> Self {
        while text.ends_with(' ') {
            text.pop();
        }
        let has_content = text.chars().any(|character| character != ' ');
        Self { text, has_content }
    }
}

#[derive(Clone, Debug, Default)]
struct TimestampedRow {
    row: TerminalRow,
    timestamp: Option<String>,
}

#[derive(Default)]
pub struct TerminalTimestampState {
    rows: Vec<TimestampedRow>,
    latest_timestamp: Option<String>,
}

impl TerminalTimestampState {
    /// Update the known main-screen rows and return timestamps for `rows`.
    ///
    /// `rows` is the current viewport in display order. The tracker keeps a
    /// bounded sequence of rows so scrolling can recover timestamps for rows
    /// that were previously visible. `timestamp` is supplied by the terminal
    /// wakeup event and is only applied to changed/content rows or the cursor
    /// row.
    pub fn observe(
        &mut self,
        rows: &[TerminalRow],
        display_offset: usize,
        cursor_row: Option<usize>,
        timestamp: Option<String>,
        alternate_screen: bool,
    ) -> Vec<Option<String>> {
        if alternate_screen {
            return vec![None; rows.len()];
        }

        if let Some(timestamp) = timestamp.as_ref() {
            self.latest_timestamp = Some(timestamp.clone());
        }

        if rows.is_empty() {
            return Vec::new();
        }

        let visible_start = if let Some(start) = self.find_alignment(rows, display_offset) {
            self.merge_rows(start, rows, cursor_row, timestamp.as_ref());
            start
        } else if display_offset == 0 {
            self.rows = rows
                .iter()
                .enumerate()
                .map(|(index, row)| TimestampedRow {
                    row: row.clone(),
                    timestamp: timestamp_for(timestamp.as_ref(), row, cursor_row == Some(index)),
                })
                .collect();
            0
        } else {
            return vec![None; rows.len()];
        };

        let removed = self.trim_rows();
        let visible_start = visible_start.saturating_sub(removed);
        (0..rows.len())
            .map(|index| {
                self.rows
                    .get(visible_start + index)
                    .and_then(|row| row.timestamp.clone())
            })
            .collect()
    }

    fn merge_rows(
        &mut self,
        start: usize,
        rows: &[TerminalRow],
        cursor_row: Option<usize>,
        timestamp: Option<&String>,
    ) {
        let known_len = self.rows.len();
        for (index, row) in rows.iter().enumerate() {
            let known_index = start + index;
            let next_timestamp = timestamp_for(timestamp, row, cursor_row == Some(index));
            let next_timestamp = if known_index >= known_len && next_timestamp.is_none() {
                timestamp_for(
                    self.latest_timestamp.as_ref(),
                    row,
                    cursor_row == Some(index),
                )
            } else {
                next_timestamp
            };

            if known_index >= known_len {
                self.rows.push(TimestampedRow {
                    row: row.clone(),
                    timestamp: next_timestamp,
                });
            } else if self.rows[known_index].row != *row {
                self.rows[known_index].row = row.clone();
                self.rows[known_index].timestamp = next_timestamp;
            } else if cursor_row == Some(index) && next_timestamp.is_some() {
                self.rows[known_index].timestamp = next_timestamp;
            }
        }
    }

    fn find_alignment(&self, rows: &[TerminalRow], display_offset: usize) -> Option<usize> {
        if self.rows.is_empty() {
            return None;
        }

        let viewport_lines = rows.len();
        let expected = self
            .rows
            .len()
            .saturating_sub(viewport_lines.saturating_add(display_offset));
        let lower = expected.saturating_sub(ALIGNMENT_SEARCH_RADIUS);
        let upper = expected
            .saturating_add(ALIGNMENT_SEARCH_RADIUS)
            .min(self.rows.len());

        let mut best: Option<(usize, usize, usize, usize)> = None;
        for start in lower..=upper {
            let overlap = rows.len().min(self.rows.len().saturating_sub(start));
            if overlap == 0 {
                continue;
            }

            let mut matches = 0;
            let mut informative_matches = 0;
            for (index, row) in rows.iter().take(overlap).enumerate() {
                if self.rows[start + index].row == *row {
                    matches += 1;
                    informative_matches += usize::from(row.has_content);
                }
            }
            if matches == 0 {
                continue;
            }

            let score = informative_matches * 4 + matches;
            let distance = start.abs_diff(expected);
            let candidate = (score, informative_matches, distance, start);
            let is_better = best.is_none_or(|current| {
                candidate.0 > current.0
                    || (candidate.0 == current.0 && candidate.1 > current.1)
                    || (candidate.0 == current.0
                        && candidate.1 == current.1
                        && candidate.2 < current.2)
            });
            if is_better {
                best = Some(candidate);
            }
        }

        best.map(|(_, _, _, start)| start)
    }

    fn trim_rows(&mut self) -> usize {
        let removed = self.rows.len().saturating_sub(MAX_TRACKED_ROWS);
        if removed > 0 {
            self.rows.drain(..removed);
        }
        removed
    }
}

fn timestamp_for(
    timestamp: Option<&String>,
    row: &TerminalRow,
    is_cursor_row: bool,
) -> Option<String> {
    timestamp
        .filter(|_| row.has_content || is_cursor_row)
        .cloned()
}

pub fn timestamp_now() -> String {
    Local::now().format("%H:%M:%S%.3f").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str) -> TerminalRow {
        TerminalRow::new(text.to_owned())
    }

    #[test]
    fn timestamp_has_millisecond_precision() {
        let timestamp = timestamp_now();
        assert_eq!(timestamp.len(), 12);
        assert_eq!(timestamp.as_bytes()[8], b'.');
    }

    #[test]
    fn timestamps_follow_rows_that_scroll_into_view() {
        let mut state = TerminalTimestampState::default();
        let initial = vec![row("one"), row("two"), row("three")];
        assert_eq!(
            state.observe(&initial, 0, Some(0), Some("10:00:00.001".to_owned()), false,),
            vec![
                Some("10:00:00.001".to_owned()),
                Some("10:00:00.001".to_owned()),
                Some("10:00:00.001".to_owned()),
            ]
        );

        let after_scroll = vec![row("two"), row("three"), row("four")];
        assert_eq!(
            state.observe(
                &after_scroll,
                0,
                Some(2),
                Some("10:00:00.002".to_owned()),
                false,
            ),
            vec![
                Some("10:00:00.001".to_owned()),
                Some("10:00:00.001".to_owned()),
                Some("10:00:00.002".to_owned()),
            ]
        );
    }

    #[test]
    fn unseen_output_keeps_a_timestamp_until_it_reaches_the_viewport() {
        let mut state = TerminalTimestampState::default();
        let initial = vec![row("one"), row("two"), row("three")];
        state.observe(&initial, 0, Some(0), Some("10:00:00.001".to_owned()), false);

        state.observe(&initial, 1, None, Some("10:00:00.002".to_owned()), false);

        let at_bottom = vec![row("two"), row("three"), row("four")];
        assert_eq!(
            state.observe(&at_bottom, 0, Some(2), None, false),
            vec![
                Some("10:00:00.001".to_owned()),
                Some("10:00:00.001".to_owned()),
                Some("10:00:00.002".to_owned()),
            ]
        );
    }

    #[test]
    fn alternate_screen_does_not_replace_main_screen_timestamps() {
        let mut state = TerminalTimestampState::default();
        let main = vec![row("prompt"), row("")];
        state.observe(&main, 0, Some(0), Some("10:00:00.001".to_owned()), false);

        let alternate = vec![row("vim"), row("~")];
        assert_eq!(
            state.observe(
                &alternate,
                0,
                Some(0),
                Some("10:00:00.002".to_owned()),
                true,
            ),
            vec![None, None]
        );
        assert_eq!(
            state.observe(&main, 0, None, None, false),
            vec![Some("10:00:00.001".to_owned()), None]
        );
    }
}
