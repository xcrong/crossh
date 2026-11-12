//! 单实例与令牌载体：原子占位 + 专用子目录 + 权限收敛。
//!
//! - `daemon.port` 与 `instance.port` / `git.port` / `note.port` 互不影响：
//!   新文件名 + 新解析函数，不动既有三份文件的读取路径（D4，任务 5.3）。
//! - 原子占位：`OpenOptions::create_new` 独占创建，成功者即属主；
//!   失败者读文件连上去；属主正常退出删除文件；启动时连接失败视为陈旧文件，
//!   删除后重试一次（D7，竞态假解见 review F6）。
//! - 令牌与端口同文件；Unix 下目录 `0700`、文件 `0600`；
//!   Windows 依赖每用户目录默认 ACL（`#[cfg(unix)]` 分支）。

use std::fs::OpenOptions;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

/// daemon 端口文件名（与 `instance.port` 区分，避免旧版残留被误读）。
pub const DAEMON_PORT_FILE: &str = "daemon.port";

fn cache_root() -> PathBuf {
    // 测试钩子：集成测试用临时目录隔离，不碰用户真实缓存。
    if let Some(dir) = std::env::var_os("CROSSH_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    let suffix = if cfg!(debug_assertions) { "-dev" } else { "" };
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("crossh{suffix}"))
}

/// daemon 专用子目录（不是共享的 `port_dir`，见 D4）。
pub fn daemon_dir() -> PathBuf {
    cache_root().join("daemon")
}

/// `daemon.port` 完整路径。
pub fn daemon_port_file_path() -> PathBuf {
    daemon_dir().join(DAEMON_PORT_FILE)
}

/// 端口文件内容：`{port}\n{token}\n`。令牌只从文件读取，不落日志。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonAddr {
    pub port: u16,
    pub token: String,
}

pub fn read_daemon_addr() -> Option<DaemonAddr> {
    let text = std::fs::read_to_string(daemon_port_file_path()).ok()?;
    parse_daemon_addr(&text)
}

fn parse_daemon_addr(text: &str) -> Option<DaemonAddr> {
    let mut lines = text.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    let token = lines.next()?.trim().to_string();
    if token.is_empty() {
        return None;
    }
    Some(DaemonAddr { port, token })
}

fn write_daemon_addr(port: u16, token: &str) -> std::io::Result<()> {
    let dir = daemon_dir();
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = daemon_port_file_path();
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    // 建文件即 `0600`：赢家 `write_all` 落盘前的窗口里文件也不对外可见。
    #[cfg(unix)]
    opts.mode(0o600);
    let mut f = opts.open(&path)?;
    f.write_all(format!("{port}\n{token}\n").as_bytes())?;
    f.sync_all()?;
    Ok(())
}

/// 原子占位结果。
#[derive(Debug)]
pub enum Claim {
    /// 本进程成为属主：已绑定 `listener` 并落盘。后到者将读到它。
    Owner { listener: TcpListener },
    /// 已有属主在运行：返回其地址，调用方应接入而不是再占位。
    Existing(DaemonAddr),
}

/// 尝试成为会话属主。`token` 为本进程生成的随机令牌。
/// 陈旧文件（连不上）删除后重试一次；仍失败则返回 IO 错误。
pub fn claim_instance(token: &str) -> std::io::Result<Claim> {
    match try_claim_once(token)? {
        Some(claim) => Ok(claim),
        None => {
            // 可能是陈旧文件：删掉再试一次。
            let _ = std::fs::remove_file(daemon_port_file_path());
            try_claim_once(token)?.ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::AddrInUse, "daemon already running")
            })
        }
    }
}

fn try_claim_once(token: &str) -> std::io::Result<Option<Claim>> {
    let dir = daemon_dir();
    std::fs::create_dir_all(&dir)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    // tokio `from_std` 要求非阻塞（daemon run 在 tokio 内转换）。
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    match write_daemon_addr(port, token) {
        Ok(()) => Ok(Some(Claim::Owner { listener })),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // 已有文件：读地址并探活。赢家可能正在 `write_all`（半截文件）
            // 或刚落盘监听尚未就绪：给一次 50ms 重读机会再判陈旧，
            // 否则并发占位会误删属主文件造出多个 Owner。
            for _ in 0..2 {
                if let Some(addr) = read_daemon_addr() {
                    let probe = format!("127.0.0.1:{}", addr.port);
                    let parsed: std::net::SocketAddr = match probe.parse() {
                        Ok(addr) => addr,
                        Err(_) => {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "bad port",
                            ));
                        }
                    };
                    if TcpStream::connect_timeout(&parsed, CONNECT_TIMEOUT).is_ok() {
                        return Ok(Some(Claim::Existing(addr)));
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            // 两次都连不上 → 陈旧文件，返回 None 让上层删除重试。
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

pub fn remove_port_file() {
    let _ = std::fs::remove_file(daemon_port_file_path());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{LazyLock, Mutex};

    static FILE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
    #[test]
    fn parse_roundtrip() {
        let addr = parse_daemon_addr("1234\nabc\n").unwrap();
        assert_eq!(addr.port, 1234);
        assert_eq!(addr.token, "abc");
        assert!(parse_daemon_addr("1234\n\n").is_none());
        assert!(parse_daemon_addr("notaport\nabc\n").is_none());
    }

    #[test]
    fn concurrent_claim_exactly_one_owner() {
        let _guard = FILE_LOCK.lock().unwrap();
        // 可确定复现：并发 N 次占位，恰好一次胜出。
        let dir = daemon_dir();
        let _ = std::fs::remove_file(daemon_port_file_path());
        let _ = std::fs::create_dir_all(&dir);
        let n = 8;
        let handles: Vec<_> = (0..n)
            .map(|i| {
                std::thread::spawn(move || {
                    // 每个线程用独立 token；只有一个能 create_new 成功。
                    claim_instance(&format!("tok-{i}-{}", std::process::id()))
                })
            })
            .collect();
        // 属主 listener 持有到全部 join 结束：真实 daemon 会持续监听，
        // 败者探活成功才判 Existing；提前 drop 会让端口变“陈旧”导致误删重夺。
        let mut owners = 0;
        let mut existing = 0;
        let claims: Vec<_> = handles
            .into_iter()
            .map(|h| h.join().expect("claim thread"))
            .collect();
        for c in &claims {
            match c {
                Ok(Claim::Owner { .. }) => owners += 1,
                _ => existing += 1,
            }
        }
        assert_eq!(owners, 1, "owners={owners} existing={existing}");
        assert_eq!(owners + existing, n);
        drop(claims);
        // 清理：属主退出删除文件（此处测试进程即“属主环境”，直接删）。
        remove_port_file();
    }

    #[test]
    #[cfg(unix)]
    fn port_file_is_0600_when_created_here() {
        let _guard = FILE_LOCK.lock().unwrap();
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::remove_file(daemon_port_file_path());
        let _ = std::fs::create_dir_all(daemon_dir());
        // 直接走写入路径（单线程独占）。
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // 若文件已存在先清理，保证 create_new 成功。
        let _ = std::fs::remove_file(daemon_port_file_path());
        write_daemon_addr(port, "perm-check-token").unwrap();
        let mode = std::fs::metadata(daemon_port_file_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_file(daemon_port_file_path());
    }
}
