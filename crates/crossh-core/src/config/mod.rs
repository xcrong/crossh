//! OpenSSH configuration parsing and host resolution.

pub mod ssh_config;

pub use ssh_config::{ConfigError, ForwardSpec, HostConfig, SshConfig, expand_tilde};
