//! 主窗口单实例入口：实现已移入 [`crossh_core::single_instance`]，
//! 此处仅做 crate 内重导出，保持 `app::single_instance::` 调用点不动。
//! git/note 入口只给独立二进制用，主二进制用不到，告警无意义。
#![allow(unused_imports)]

pub(crate) use crossh_core::single_instance::{
    ForwardOutcome, git_port_file_path, note_port_file_path, port_file_path, resolve_open_path,
    serve, serve_git, serve_note, try_forward, try_forward_git, try_forward_note,
};
