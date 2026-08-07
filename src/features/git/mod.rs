//! Git 可视化功能：独立的 Git 窗口（VS Code 源码管理风格）。

pub(crate) mod logic;
mod window;

pub(crate) use window::open_git_window;
