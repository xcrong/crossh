//! Connection-facing UI such as authentication and host-key prompts.

pub(crate) mod entity;
pub(crate) mod host;
pub(crate) mod manager;
pub(crate) mod prompt;

pub(crate) use entity::{Connection, PendingPrompt};
pub(crate) use manager::ConnectionManager;
pub(crate) use prompt::{PromptDisplay, render_prompt_modal};
