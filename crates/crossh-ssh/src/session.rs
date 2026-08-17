//! SSH 共享类型：认证方式候选、终端通道的输入命令与事件。
//!
//! 连接/认证/relay 逻辑见 `super::connection`。本模块只保留跨模块复用的数据类型
//! 与「从 HostConfig 推导认证候选」的纯函数。

use std::path::PathBuf;

use crossh_core::config::HostConfig;
/// 用户选择的认证方式。
#[derive(Clone, Debug)]
pub enum AuthChoice {
    /// ssh-agent（读取 SSH_AUTH_SOCK）。
    Agent { user: String },
    /// 私钥文件（可选口令）。
    Key {
        user: String,
        path: PathBuf,
        passphrase: Option<String>,
    },
}

/// 从 HostConfig 推导认证方式候选列表（依次尝试，首个成功即用）。
///
/// 复刻 OpenSSH 默认行为：
///  - 显式 `IdentityFile`（config 中声明的，按顺序）。
///  - 当未设 `IdentitiesOnly yes` 时，还会自动发现标准默认密钥
///    （`~/.ssh/id_rsa` / `id_ed25519` / `id_ecdsa` / `id_dsa` / `identity`）。
///  - ssh-agent（`SSH_AUTH_SOCK` 设置时）。
///
/// 顺序：显式密钥 → 默认密钥 → agent。
pub fn default_auth_for(host: &HostConfig) -> Vec<AuthChoice> {
    let user = default_user_for(host);
    let mut out: Vec<AuthChoice> = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    let push_key = |out: &mut Vec<AuthChoice>,
                    seen: &mut std::collections::HashSet<PathBuf>,
                    path: PathBuf| {
        if path.is_file() && seen.insert(path.clone()) {
            out.push(AuthChoice::Key {
                user: user.clone(),
                path,
                passphrase: None,
            });
        }
    };

    // 1) config 显式声明的 IdentityFile（展开 ~）。
    for raw in &host.identity_files {
        let path = PathBuf::from(crossh_core::config::expand_tilde(raw));
        push_key(&mut out, &mut seen, path);
    }

    // 2) 默认密钥发现（除非 IdentitiesOnly yes）。
    let only = host.identities_only == Some(true);
    if !only {
        let home = std::env::var_os("HOME");
        if let Some(home) = home {
            let ssh_dir = PathBuf::from(&home).join(".ssh");
            for name in ["id_ed25519", "id_ecdsa", "id_rsa", "id_dsa", "identity"] {
                push_key(&mut out, &mut seen, ssh_dir.join(name));
            }
        }
    }

    // 3) ssh-agent。
    if std::env::var_os("SSH_AUTH_SOCK").is_some() {
        out.push(AuthChoice::Agent { user: user.clone() });
    }

    out
}

/// 返回 SSH 配置没有显式 User 时应使用的本地用户名。
pub(crate) fn default_user_for(host: &HostConfig) -> String {
    host.user.clone().unwrap_or_else(whoami)
}

fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "root".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn host_with_key(dir: &std::path::Path) -> HostConfig {
        HostConfig {
            aliases: vec!["t".to_string()],
            host_name: None,
            user: Some("alice".to_string()),
            port: None,
            identity_files: vec![dir.join("id_test").to_string_lossy().into_owned()],
            identities_only: Some(true),
            proxy_jump: None,
            local_forwards: Vec::new(),
            remote_forwards: Vec::new(),
            dynamic_forwards: Vec::new(),
        }
    }

    #[test]
    fn spec_20260817_remove_auth_choice_password_default_auth_never_yields_password() {
        let old_sock = std::env::var_os("SSH_AUTH_SOCK");
        unsafe { std::env::remove_var("SSH_AUTH_SOCK") };

        let dir = std::env::temp_dir().join(format!("crossh-auth-choice-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("id_test"), b"unused").unwrap();

        let host = host_with_key(&dir);
        let mut methods = default_auth_for(&host);
        // 穷尽匹配：任何不在 Key/Agent 内的变体（含未来的 Password）都会编译或断言失败。
        assert!(
            methods
                .iter()
                .all(|m| matches!(m, AuthChoice::Key { .. } | AuthChoice::Agent { .. }))
        );
        assert_eq!(methods.len(), 1);
        assert!(matches!(&methods[0], AuthChoice::Key { user, .. } if user == "alice"));

        // agent 可用时追加 Agent，顺序在显式密钥之后。
        unsafe { std::env::set_var("SSH_AUTH_SOCK", "/dev/null") };
        methods = default_auth_for(&host);
        assert!(matches!(
            methods.as_slice(),
            [AuthChoice::Key { .. }, AuthChoice::Agent { .. }]
        ));
        match old_sock {
            Some(v) => unsafe { std::env::set_var("SSH_AUTH_SOCK", v) },
            None => unsafe { std::env::remove_var("SSH_AUTH_SOCK") },
        }

        fs::remove_dir_all(&dir).unwrap();
    }
}
