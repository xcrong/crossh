//! Non-blocking terminal input delivery.

use std::collections::VecDeque;

use async_channel::{Sender, TrySendError};

use crate::shared::terminal::InputCmd;

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
