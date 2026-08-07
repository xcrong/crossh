//! Non-blocking terminal input delivery and optional local line editing.

use std::collections::VecDeque;

use async_channel::{Sender, TrySendError};

use crate::shared::terminal::InputCmd;

/// Local line editor used by the optional low-latency shell input mode.
///
/// The remote PTY remains the source of truth. This buffer only owns text
/// that has not been submitted with Enter yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ShellInputBuffer {
    value: String,
    cursor: usize,
}

impl ShellInputBuffer {
    pub(super) fn text(&self) -> &str {
        &self.value
    }

    pub(super) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(super) fn insert(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.value.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    pub(super) fn backspace(&mut self) {
        let start = previous_char_boundary(&self.value, self.cursor);
        if start != self.cursor {
            self.value.replace_range(start..self.cursor, "");
            self.cursor = start;
        }
    }

    pub(super) fn delete(&mut self) {
        let end = next_char_boundary(&self.value, self.cursor);
        if end != self.cursor {
            self.value.replace_range(self.cursor..end, "");
        }
    }

    pub(super) fn move_left(&mut self) {
        self.cursor = previous_char_boundary(&self.value, self.cursor);
    }

    pub(super) fn move_right(&mut self) {
        self.cursor = next_char_boundary(&self.value, self.cursor);
    }

    pub(super) fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub(super) fn move_end(&mut self) {
        self.cursor = self.value.len();
    }

    pub(super) fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.value)
    }

    pub(super) fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map(|ch| cursor + ch.len_utf8())
        .unwrap_or(cursor)
}

pub(super) fn queue_input_nonblocking(
    input_tx: &Sender<InputCmd>,
    pending_input: &mut VecDeque<InputCmd>,
    command: InputCmd,
) {
    flush_pending_commands(input_tx, pending_input);
    match input_tx.try_send(command) {
        Ok(()) => {}
        Err(TrySendError::Full(command)) => pending_input.push_back(command),
        Err(TrySendError::Closed(_)) => {
            log::warn!("input_tx is closed");
            pending_input.clear();
        }
    }
}

pub(super) fn flush_pending_commands(
    input_tx: &Sender<InputCmd>,
    pending_input: &mut VecDeque<InputCmd>,
) {
    while let Some(command) = pending_input.pop_front() {
        match input_tx.try_send(command) {
            Ok(()) => {}
            Err(TrySendError::Full(command)) => {
                pending_input.push_front(command);
                break;
            }
            Err(TrySendError::Closed(_)) => {
                log::warn!("input_tx is closed");
                pending_input.clear();
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ShellInputBuffer;

    #[test]
    fn edits_utf8_text_at_character_boundaries() {
        let mut buffer = ShellInputBuffer::default();
        buffer.insert("a中b");
        buffer.move_left();
        buffer.backspace();
        assert_eq!(buffer.text(), "ab");
        assert_eq!(buffer.cursor(), 1);

        buffer.move_home();
        buffer.delete();
        assert_eq!(buffer.text(), "b");
        buffer.move_end();
        assert_eq!(buffer.take(), "b");
        assert!(buffer.text().is_empty());
    }

    #[test]
    fn inserts_at_the_local_cursor() {
        let mut buffer = ShellInputBuffer::default();
        buffer.insert("ac");
        buffer.move_left();
        buffer.insert("中");
        assert_eq!(buffer.text(), "a中c");
        assert_eq!(buffer.cursor(), "a中".len());
    }
}
