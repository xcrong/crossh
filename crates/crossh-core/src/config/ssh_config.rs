//! 解析 `~/.ssh/config`（常用子集）。
//!
//! 支持的关键字：`Host`、`HostName`、`User`、`Port`、`IdentityFile`、
//! `ProxyJump`、`LocalForward`、`RemoteForward`、`DynamicForward`、`Include`。
//! 不支持：`Match exec`、`ProxyCommand`（计划明确排除）。
//!
//! 主机匹配遵循 OpenSSH「首匹配胜出」语义（标量键取首个命中块，IdentityFile
//! 与端口转发规则跨块累加）。

use std::fs;
use std::path::{Path, PathBuf};

/// 一条端口转发规则。`-L`/`-R`: `[bind:]listen:host:port`；`-D`: `[bind:]listen`。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForwardSpec {
    /// 监听端，例如 `8080`、`127.0.0.1:8080`、`localhost:8080`。
    pub listen: String,
    /// 目标端，例如 `example.com:80`。仅 -L/-R 有意义；-D 为空。
    pub remote: String,
}

/// 单个 `Host` 块解析出的（部分）配置。
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// 该块 `Host` 行的全部模式（用于匹配与展示）。
    pub aliases: Vec<String>,
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_files: Vec<String>,
    /// `IdentitiesOnly yes` 时只使用显式声明的 IdentityFile，不自动发现默认密钥。
    pub identities_only: Option<bool>,
    pub proxy_jump: Option<String>,
    pub local_forwards: Vec<ForwardSpec>,
    pub remote_forwards: Vec<ForwardSpec>,
    pub dynamic_forwards: Vec<ForwardSpec>,
}

impl HostConfig {
    /// 块的展示别名（首个模式）。
    pub fn alias(&self) -> &str {
        self.aliases.first().map(|s| s.as_str()).unwrap_or("?")
    }

    /// 是否匹配给定目标（任一模式命中即视为该块适用）。
    pub fn matches(&self, target: &str) -> bool {
        self.aliases.iter().any(|pat| pattern_matches(pat, target))
    }

    /// 用于列表展示的「有效地址」（HostName 优先，否则用别名）。
    pub fn effective_host(&self) -> &str {
        self.host_name.as_deref().unwrap_or_else(|| self.alias())
    }

    pub fn effective_port(&self) -> u16 {
        self.port.unwrap_or(22)
    }
}

/// 完整解析结果：所有 Host 块（保持源文件顺序）。
#[derive(Debug, Default, Clone)]
pub struct SshConfig {
    pub hosts: Vec<HostConfig>,
}

impl SshConfig {
    /// 解析 `~/.ssh/config`（展开 `~`）。
    pub fn from_default_location() -> Result<Self, ConfigError> {
        let path = default_config_path()?;
        Self::from_path(&path)
    }

    /// 解析指定文件（递归处理 `Include`）。
    pub fn from_path(path: &Path) -> Result<Self, ConfigError> {
        let mut seen: Vec<PathBuf> = Vec::new();
        let mut cfg = SshConfig::default();
        parse_file(path, &mut cfg, &mut seen)?;
        Ok(cfg)
    }

    /// 列出所有「具名」主机（排除纯通配模式如 `*`、`*.example.com` 的展示）。
    /// 这里返回所有 Host 块，让 UI 自行过滤/展示。
    pub fn hosts(&self) -> &[HostConfig] {
        &self.hosts
    }

    /// 解析用户输入的目标名：跨块「首匹配胜出」合并有效配置。
    /// - 标量键（HostName/User/Port/ProxyJump）：首个命中块的值生效。
    /// - IdentityFile / 三类转发：所有命中块累加。
    pub fn resolve(&self, target: &str) -> HostConfig {
        let mut merged = HostConfig {
            aliases: vec![target.to_string()],
            host_name: None,
            user: None,
            port: None,
            identity_files: Vec::new(),
            identities_only: None,
            proxy_jump: None,
            local_forwards: Vec::new(),
            remote_forwards: Vec::new(),
            dynamic_forwards: Vec::new(),
        };

        for h in &self.hosts {
            if !h.matches(target) {
                continue;
            }
            if merged.host_name.is_none() {
                merged.host_name = h.host_name.clone();
            }
            if merged.user.is_none() {
                merged.user = h.user.clone();
            }
            if merged.port.is_none() {
                merged.port = h.port;
            }
            if merged.identities_only.is_none() {
                merged.identities_only = h.identities_only;
            }
            if merged.proxy_jump.is_none() {
                merged.proxy_jump = h.proxy_jump.clone();
            }
            for f in &h.identity_files {
                if !merged.identity_files.iter().any(|x| x == f) {
                    merged.identity_files.push(f.clone());
                }
            }
            merged
                .local_forwards
                .extend(h.local_forwards.iter().cloned());
            merged
                .remote_forwards
                .extend(h.remote_forwards.iter().cloned());
            merged
                .dynamic_forwards
                .extend(h.dynamic_forwards.iter().cloned());
        }

        // 用户直接输入 "host:port" 或 "user@host" 形式时也支持一下。
        if merged.host_name.is_none() && merged.user.is_none() {
            let (user, host, port) = split_target(target);
            merged.host_name = Some(host.to_string());
            merged.user = user.map(|u| u.to_string());
            if let Some(p) = port {
                merged.port = Some(p);
            }
        }

        merged
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("home directory not found (set $HOME)")]
    NoHome,
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn default_config_path() -> Result<PathBuf, ConfigError> {
    let home = std::env::var_os("HOME").ok_or(ConfigError::NoHome)?;
    Ok(PathBuf::from(home).join(".ssh").join("config"))
}

/// 解析单个文件。`seen` 用于防止 Include 循环。
fn parse_file(
    path: &Path,
    cfg: &mut SshConfig,
    seen: &mut Vec<PathBuf>,
) -> Result<(), ConfigError> {
    let canonical = match fs::canonicalize(path) {
        Ok(c) => c,
        Err(_) => {
            // 文件不存在（常见：用户还没有 ~/.ssh/config）→ 当作空配置。
            return Ok(());
        }
    };
    if seen.iter().any(|p| p == &canonical) {
        return Ok(()); // 防循环
    }
    seen.push(canonical.clone());

    let content = fs::read_to_string(&canonical).map_err(|source| ConfigError::Read {
        path: canonical.clone(),
        source,
    })?;

    // 行内 KEY VALUE 分词；忽略注释/空行。
    for raw in content.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = match split_kv(line) {
            Some(kv) => kv,
            None => continue,
        };
        // 关键字大小写不敏感。
        match key.to_ascii_lowercase().as_str() {
            "include" => {
                for inc in glob_includes(&canonical, value) {
                    parse_file(&inc, cfg, seen)?;
                }
            }
            "host" => {
                let aliases: Vec<String> =
                    value.split_whitespace().map(|s| s.to_string()).collect();
                if !aliases.is_empty() {
                    cfg.hosts.push(HostConfig {
                        aliases,
                        host_name: None,
                        user: None,
                        port: None,
                        identity_files: Vec::new(),
                        identities_only: None,
                        proxy_jump: None,
                        local_forwards: Vec::new(),
                        remote_forwards: Vec::new(),
                        dynamic_forwards: Vec::new(),
                    });
                }
            }
            _ => {
                if let Some(last) = cfg.hosts.last_mut() {
                    apply_key(last, key, value);
                }
            }
        }
    }
    Ok(())
}

/// 把一个键值应用到当前 Host 块。
fn apply_key(host: &mut HostConfig, key: &str, value: &str) {
    match key.to_ascii_lowercase().as_str() {
        "hostname" => host.host_name = Some(value.trim().to_string()),
        "user" => host.user = Some(value.trim().to_string()),
        "port" => {
            if let Ok(p) = value.trim().parse::<u16>() {
                host.port = Some(p);
            }
        }
        "identityfile" => host.identity_files.push(value.trim().to_string()),
        "identitiesonly" => {
            let v = value.trim().to_ascii_lowercase();
            host.identities_only = Some(v == "yes");
        }
        "proxyjump" => host.proxy_jump = Some(value.trim().to_string()),
        "localforward" => {
            if let Some(spec) = parse_forward(value, false) {
                host.local_forwards.push(spec);
            }
        }
        "remoteforward" => {
            if let Some(spec) = parse_forward(value, false) {
                host.remote_forwards.push(spec);
            }
        }
        "dynamicforward" => {
            if let Some(spec) = parse_forward(value, true) {
                host.dynamic_forwards.push(spec);
            }
        }
        _ => {} // 其它关键字第一期忽略
    }
}

/// 解析转发规则。`is_dynamic=true` 时只有 listen 端。
fn parse_forward(value: &str, is_dynamic: bool) -> Option<ForwardSpec> {
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }
    // LocalForward / RemoteForward 形如: [bind:]lport host:hport
    if is_dynamic {
        return Some(ForwardSpec {
            listen: parts[0].to_string(),
            remote: String::new(),
        });
    }
    match parts.len() {
        1 => {
            // "lport:host:hport" 或 "bind:lport:host:hport" 紧凑写法
            let segs: Vec<&str> = parts[0].splitn(2, ':').collect();
            if segs.len() == 2 {
                Some(ForwardSpec {
                    listen: segs[0].to_string(),
                    remote: segs[1].to_string(),
                })
            } else {
                None
            }
        }
        _ => Some(ForwardSpec {
            listen: parts[0].to_string(),
            remote: parts[1].to_string(),
        }),
    }
}

/// `Include` 的 glob 展开（相对 ~/.ssh/，支持 `*`/`?`）。失败则忽略。
fn glob_includes(referring_file: &Path, pattern: &str) -> Vec<PathBuf> {
    let expanded = expand_tilde(pattern.trim());
    let p = PathBuf::from(&expanded);
    let base = if p.is_absolute() {
        PathBuf::new()
    } else {
        referring_file
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf()
    };
    let full = base.join(&p);

    // 简单 glob：含通配符才扫描目录，否则直接返回路径。
    let s = full.to_string_lossy();
    if !s.contains('*') && !s.contains('?') {
        return vec![full];
    }
    let parent = match full.parent() {
        Some(d) => d,
        None => return vec![full],
    };
    let pat = match full.file_name().and_then(|n| n.to_str()) {
        Some(p) => p,
        None => return vec![full],
    };
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(parent) {
        for ent in entries.flatten() {
            if let Some(name) = ent.file_name().to_str()
                && pattern_matches(pat, name)
            {
                out.push(ent.path());
            }
        }
    }
    out.sort();
    out
}

pub fn expand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return Path::new(&home).join(rest).to_string_lossy().into_owned();
        }
    } else if s == "~"
        && let Some(home) = std::env::var_os("HOME")
    {
        return home.to_string_lossy().into_owned();
    }
    s.to_string()
}

/// OpenSSH Host 模式匹配：`*` 匹配任意、`?` 匹配单字符、`!` 取反（仅多段列表）。
fn pattern_matches(pattern: &str, target: &str) -> bool {
    // 单个 `!` 前缀取反（OpenSSH 语义里 ! 是列表级别的取反，这里近似处理）。
    if let Some(rest) = pattern.strip_prefix('!') {
        return !glob(rest, target);
    }
    glob(pattern, target)
}

/// 极简通配匹配（`*` 任意、`?` 单字符），大小写敏感。
fn glob(pattern: &str, target: &str) -> bool {
    fn helper(p: &[u8], t: &[u8]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some(b'*'), _) => helper(&p[1..], t) || (!t.is_empty() && helper(p, &t[1..])),
            (Some(b'?'), Some(_)) => helper(&p[1..], &t[1..]),
            (Some(&a), Some(&b)) => a.eq_ignore_ascii_case(&b) && helper(&p[1..], &t[1..]),
            _ => false,
        }
    }
    helper(pattern.as_bytes(), target.as_bytes())
}

/// `KEY VALUE` 拆分（首段为 key，其余整体为 value；允许 `=` 分隔）。
fn split_kv(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
        i += 1;
    }
    if i == 0 || i == bytes.len() {
        return None;
    }
    let key = &line[..i];
    let mut rest = &line[i..];
    rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '=');
    Some((key, rest.trim_end()))
}

/// 去除行尾注释（`#`），但不破坏引号内的 `#`。
fn strip_comment(line: &str) -> &str {
    let mut in_quote = false;
    for (i, ch) in line.char_indices() {
        match ch {
            '"' => in_quote = !in_quote,
            '#' if !in_quote => return &line[..i],
            _ => {}
        }
    }
    line
}

/// 拆解 `user@host:port` 三种组合。
fn split_target(target: &str) -> (Option<&str>, &str, Option<u16>) {
    let (user, rest) = match target.split_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, target),
    };

    // tty7 风格的 QuickConnect 也接受 `[::1]:2222`；去掉方括号后交给
    // russh，避免把 IPv6 地址的内部冒号误判成端口分隔符。
    if let Some(bracketed) = rest.strip_prefix('[')
        && let Some((host, suffix)) = bracketed.split_once(']')
    {
        let port = suffix
            .strip_prefix(':')
            .and_then(|value| value.parse::<u16>().ok());
        if suffix.is_empty() || port.is_some() {
            return (user, host, port);
        }
    }

    // 只有单冒号且尾部确实是数字时才解析为端口；裸 IPv6 和非法端口
    // 保持完整主机名，避免丢失地址的一部分。
    if let Some((host, port)) = rest.rsplit_once(':')
        && !host.contains(':')
        && let Ok(port) = port.parse::<u16>()
    {
        return (user, host, Some(port));
    }
    (user, rest, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn cfg(src: &str) -> SshConfig {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let id = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("crossh-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join(format!("config-{id}"));
        fs::write(&path, src).unwrap();
        SshConfig::from_path(&path).unwrap()
    }

    #[test]
    fn parses_basic_host() {
        let c = cfg(
            "Host web\n  HostName 10.0.0.5\n  User deploy\n  Port 2222\n  IdentityFile ~/.ssh/id_web\n",
        );
        assert_eq!(c.hosts.len(), 1);
        let h = &c.hosts[0];
        assert_eq!(h.alias(), "web");
        assert_eq!(h.host_name.as_deref(), Some("10.0.0.5"));
        assert_eq!(h.user.as_deref(), Some("deploy"));
        assert_eq!(h.port, Some(2222));
        assert_eq!(h.identity_files, vec!["~/.ssh/id_web".to_string()]);
    }

    #[test]
    fn first_match_wins_for_scalar_keys() {
        // OpenSSH 语义：首个命中块的标量键生效，因此具体 host 必须在通配 host 之前。
        let c = cfg("Host web\n  User deploy\n\nHost *\n  User fallback\n");
        let r = c.resolve("web");
        assert_eq!(r.user.as_deref(), Some("deploy"));
        let r2 = c.resolve("other");
        assert_eq!(r2.user.as_deref(), Some("fallback"));
    }

    #[test]
    fn identity_files_accumulate() {
        let c =
            cfg("Host *\n  IdentityFile ~/.ssh/default\n\nHost web\n  IdentityFile ~/.ssh/web\n");
        let r = c.resolve("web");
        assert_eq!(r.identity_files.len(), 2);
    }

    #[test]
    fn wildcard_pattern_matches() {
        assert!(pattern_matches("*.example.com", "a.example.com"));
        assert!(!pattern_matches("*.example.com", "a.example.org"));
        assert!(pattern_matches("192.168.0.?", "192.168.0.5"));
        assert!(pattern_matches("*", "anything"));
    }

    #[test]
    fn parses_local_forward_two_tokens() {
        let c = cfg("Host gw\n  LocalForward 8080 localhost:80\n");
        let h = &c.hosts[0];
        assert_eq!(
            h.local_forwards,
            vec![ForwardSpec {
                listen: "8080".into(),
                remote: "localhost:80".into()
            }]
        );
    }

    #[test]
    fn parses_dynamic_forward() {
        let c = cfg("Host gw\n  DynamicForward 1080\n");
        let h = &c.hosts[0];
        assert_eq!(
            h.dynamic_forwards,
            vec![ForwardSpec {
                listen: "1080".into(),
                remote: "".into()
            }]
        );
    }

    #[test]
    fn resolves_inline_user_host_port() {
        let c = SshConfig::default();
        let r = c.resolve("root@example.com:2222");
        assert_eq!(r.user.as_deref(), Some("root"));
        assert_eq!(r.host_name.as_deref(), Some("example.com"));
        assert_eq!(r.port, Some(2222));
    }

    #[test]
    fn resolves_bracketed_ipv6_target() {
        let c = SshConfig::default();
        let r = c.resolve("root@[2001:db8::7]:2200");
        assert_eq!(r.user.as_deref(), Some("root"));
        assert_eq!(r.host_name.as_deref(), Some("2001:db8::7"));
        assert_eq!(r.port, Some(2200));
    }

    #[test]
    fn preserves_unbracketed_ipv6_target() {
        let c = SshConfig::default();
        let r = c.resolve("2001:db8::7");
        assert_eq!(r.host_name.as_deref(), Some("2001:db8::7"));
        assert_eq!(r.port, None);
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let c = cfg("# a comment\n\nHost web\n  # inline\n  User deploy\n");
        assert_eq!(c.hosts.len(), 1);
        assert_eq!(c.hosts[0].user.as_deref(), Some("deploy"));
    }

    #[test]
    fn default_port_when_unset() {
        let c = cfg("Host web\n  HostName 1.2.3.4\n");
        assert_eq!(c.resolve("web").effective_port(), 22);
    }

    #[test]
    fn proxyjump_parsed() {
        let c = cfg("Host target\n  ProxyJump jump.example.com\n");
        assert_eq!(c.hosts[0].proxy_jump.as_deref(), Some("jump.example.com"));
    }
}
