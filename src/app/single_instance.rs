//! 单实例复用：回环 TCP + 端口文件。
//!
//! 第二个 `crossh [PATH]` 进程把目录发给已在运行的首个实例后退出；
//! 无运行实例时返回 [`ForwardOutcome::NoInstance`]，调用方继续以主实例启动。
//!
//! 设计取舍（保持无聊、可测）：
//! - 只用 `std::net` 阻塞 IO + 一个 accept 线程，不引入新依赖；
//!   跨平台统一走 `127.0.0.1` 回环，避免 Unix socket / Windows 命名管道两套实现。
//! - 端口文件放在每用户缓存目录，避免 `/tmp` 多用户串扰；
//!   崩溃残留的过期文件无需清理——连接失败即视为无实例，新主实例直接覆盖。
//! - 本模块零 `gpui` 依赖。线程到前台的投递桥（channel → `cx.spawn`
//!   前台循环）由 `main` 组装，`serve` 只接受 `Fn` 回调。
//! - 同时启动的两个主实例后写者胜出，先写者成为收不到转发的孤儿窗口；
//!   窗口级去重不在本轮范围。
//! - 回环端口无认证：同机任意进程可请求打开目录。影响面限于打开目录与恢复
//!   用户自己的固定标签（含其自配 `default_command`），接受该风险。
//! - 非 UTF-8 路径：CLI 取参已是 `String`（`env::args` 有损），全链路按
//!   UTF-8 处理；根治需 `args_os` 重排 `parse_cli` 签名，暂不做。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 连接超时：主实例必然同机回环，超限即视为无实例。
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
/// 单连接读写超时：载荷仅一行路径，超限即丢弃该连接。
const IO_TIMEOUT: Duration = Duration::from_secs(2);
/// 请求行上限（含换行）：绝对路径远小于此值，超限按拒绝处理。
const MAX_REQUEST_BYTES: usize = 8192;

/// 转发结果。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ForwardOutcome {
    /// 已投递给运行中实例。
    Forwarded,
    /// 无运行中实例，调用方应以主实例启动。
    NoInstance,
    /// 运行中实例拒绝了请求（目录在其侧已失效等），不应再启动新实例。
    Rejected(String),
}

/// 端口文件路径：每用户缓存目录下的固定位置。
pub(crate) fn port_file_path() -> PathBuf {
    port_dir().join("instance.port")
}

fn port_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("crossh")
}

/// 把 CLI 原始路径解析为可打开的项目目录：相对路径相对 `cwd`，
/// 必须已存在且为目录，并经 `dunce::canonicalize` 规范化
/// （去掉 Windows `\\?\` 前缀，与 `local_paths` 保持一致）。
pub(crate) fn resolve_open_path(raw: &Path, cwd: &Path) -> Result<PathBuf, String> {
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        cwd.join(raw)
    };
    if !joined.is_dir() {
        return Err(format!("not a directory: {}", joined.display()));
    }
    dunce::canonicalize(&joined)
        .map_err(|error| format!("cannot resolve {}: {error}", joined.display()))
}

/// 尝试把打开请求转发给运行中实例（`None` 表示裸 `crossh` 的仅聚焦）。
pub(crate) fn try_forward(request: Option<&Path>) -> ForwardOutcome {
    try_forward_to(&port_file_path(), request)
}

fn try_forward_to(port_file: &Path, request: Option<&Path>) -> ForwardOutcome {
    let Some(port) = read_port(port_file) else {
        return ForwardOutcome::NoInstance;
    };
    let address: SocketAddr = match format!("127.0.0.1:{port}").parse() {
        Ok(address) => address,
        Err(_) => return ForwardOutcome::NoInstance,
    };
    let Ok(stream) = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) else {
        // 端口文件过期（主实例已崩溃）：视为无实例，调用方接管。
        return ForwardOutcome::NoInstance;
    };
    if stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .and(stream.set_write_timeout(Some(IO_TIMEOUT)))
        .is_err()
    {
        return ForwardOutcome::NoInstance;
    }
    let stream = match write_request(stream, request) {
        Ok(stream) => stream,
        Err(_) => return ForwardOutcome::NoInstance,
    };
    match read_reply(&stream) {
        // 连接已建立且发送成功后应答丢失：主实例大概率已收到请求，
        // 按已转发处理，避免再起一个窗口造成重复打开。
        None => ForwardOutcome::Forwarded,
        Some(reply) if reply == "ok" => ForwardOutcome::Forwarded,
        Some(reply) => ForwardOutcome::Rejected(
            reply
                .strip_prefix("err ")
                .unwrap_or("request rejected")
                .to_string(),
        ),
    }
}

fn write_request(mut stream: TcpStream, request: Option<&Path>) -> std::io::Result<TcpStream> {
    let mut payload = request
        .map(|path| path.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default();
    payload.push('\n');
    stream.write_all(payload.as_bytes())?;
    stream.flush()?;
    Ok(stream)
}

fn read_reply(stream: &TcpStream) -> Option<String> {
    read_line(stream, MAX_REQUEST_BYTES)
}

/// 以主实例身份启动监听：绑定回环随机端口、落盘端口文件、派生 accept 线程。
/// `on_request` 在监听线程被调用（`Some` 为目录，`None` 为仅聚焦），
/// 调用方负责转交到 UI 线程；返回前监听已就绪，可立即接受转发。
pub(crate) fn serve(on_request: impl Fn(Option<PathBuf>) + Send + 'static) -> std::io::Result<()> {
    serve_to(port_file_path(), on_request)
}

fn serve_to(
    port_file: PathBuf,
    on_request: impl Fn(Option<PathBuf>) + Send + 'static,
) -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    if let Some(parent) = port_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&port_file, port.to_string())?;
    std::thread::spawn(move || accept_loop(listener, &on_request));
    Ok(())
}

fn accept_loop(listener: TcpListener, on_request: &impl Fn(Option<PathBuf>)) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        handle_connection(stream, on_request);
    }
}

fn handle_connection(mut stream: TcpStream, on_request: &impl Fn(Option<PathBuf>)) {
    if stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .and(stream.set_write_timeout(Some(IO_TIMEOUT)))
        .is_err()
    {
        return;
    }
    let reply = match read_and_validate(&stream) {
        Ok(request) => {
            on_request(request);
            "ok\n".to_string()
        }
        Err(message) => format!("err {message}\n"),
    };
    let _ = stream.write_all(reply.as_bytes());
}

/// 读取一行请求并校验：空行表示仅聚焦；非空必须是绝对路径且仍为目录。
fn read_and_validate(stream: &TcpStream) -> Result<Option<PathBuf>, String> {
    let line = read_line(stream, MAX_REQUEST_BYTES).ok_or("request read failed")?;
    if line.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(line);
    if !path.is_absolute() {
        return Err("path must be absolute".to_string());
    }
    if !path.is_dir() {
        return Err(format!("not a directory: {}", path.display()));
    }
    dunce::canonicalize(&path)
        .map_err(|_| format!("cannot resolve {}", path.display()).to_string())
        .map(Some)
}

/// 读取到换行符为止的一行（去尾部换行，容忍 CRLF 的 `\r`），超限或失败返回 `None`。
fn read_line(mut stream: &TcpStream, cap: usize) -> Option<String> {
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if bytes.len() >= cap {
            return None;
        }
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                if byte[0] == b'\n' {
                    break;
                }
                bytes.push(byte[0]);
            }
            Err(_) => return None,
        }
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8(bytes).ok()
}

fn read_port(port_file: &Path) -> Option<u16> {
    std::fs::read_to_string(port_file).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "crossh-single-instance-{name}-{}",
            std::process::id()
        ))
    }

    fn port_file(name: &str) -> PathBuf {
        test_dir(name).join("instance.port")
    }

    #[test]
    fn resolve_open_path_accepts_existing_directories() {
        let root = test_dir("resolve");
        let existing = root.join("proj");
        std::fs::create_dir_all(&existing).expect("test directory should be created");

        assert_eq!(
            resolve_open_path(&existing, &root),
            Ok(dunce::canonicalize(&existing).unwrap())
        );
        assert_eq!(
            resolve_open_path(Path::new("proj"), &root),
            Ok(dunce::canonicalize(&existing).unwrap())
        );

        std::fs::remove_dir_all(&root).expect("test directory should be removed");
    }

    #[test]
    fn resolve_open_path_rejects_files_and_missing_paths() {
        let root = test_dir("reject");
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let file = root.join("file");
        std::fs::write(&file, b"not a directory").expect("test file should be created");

        assert!(resolve_open_path(&file, &root).is_err());
        assert!(resolve_open_path(&root.join("missing"), &root).is_err());
        assert!(resolve_open_path(Path::new("missing"), &root).is_err());

        std::fs::remove_dir_all(&root).expect("test directory should be removed");
    }

    #[test]
    fn forward_without_port_file_reports_no_instance() {
        let file = port_file("absent");
        let _ = std::fs::remove_file(&file);
        assert_eq!(try_forward_to(&file, None), ForwardOutcome::NoInstance);
    }

    #[test]
    fn forward_with_stale_port_file_reports_no_instance() {
        let file = port_file("stale");
        std::fs::create_dir_all(file.parent().unwrap()).expect("test dir should be created");
        // 特权端口同机必然连接失败，模拟崩溃残留的过期端口文件。
        std::fs::write(&file, "1").expect("stale port file should be written");
        assert_eq!(try_forward_to(&file, None), ForwardOutcome::NoInstance);
        std::fs::remove_dir_all(test_dir("stale")).expect("test directory should be removed");
    }

    #[test]
    fn serve_and_forward_round_trip_delivers_directory() {
        let root = test_dir("roundtrip");
        let project = root.join("proj");
        std::fs::create_dir_all(&project).expect("test directory should be created");
        let (sender, receiver) = mpsc::channel();

        serve_to(port_file("roundtrip"), move |request| {
            sender.send(request).expect("test receiver should be alive");
        })
        .expect("test server should start");

        let canonical = dunce::canonicalize(&project).unwrap();
        assert_eq!(
            try_forward_to(&port_file("roundtrip"), Some(&canonical)),
            ForwardOutcome::Forwarded
        );
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("request should arrive"),
            Some(canonical)
        );

        std::fs::remove_dir_all(&root).expect("test directory should be removed");
    }

    #[test]
    fn serve_and_forward_round_trip_supports_focus_only() {
        let (sender, receiver) = mpsc::channel();
        serve_to(port_file("focus"), move |request| {
            sender.send(request).expect("test receiver should be alive");
        })
        .expect("test server should start");

        assert_eq!(
            try_forward_to(&port_file("focus"), None),
            ForwardOutcome::Forwarded
        );
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("request should arrive"),
            None
        );

        std::fs::remove_dir_all(test_dir("focus")).expect("test directory should be removed");
    }

    #[test]
    fn serve_rejects_non_directories() {
        serve_to(port_file("reject-conn"), |_| {
            panic!("invalid request must not reach the callback");
        })
        .expect("test server should start");

        match try_forward_to(
            &port_file("reject-conn"),
            Some(Path::new("/definitely/not/a/dir")),
        ) {
            ForwardOutcome::Rejected(_) => {}
            outcome => panic!("expected rejection, got {outcome:?}"),
        }

        std::fs::remove_dir_all(test_dir("reject-conn")).expect("test directory should be removed");
    }

    #[test]
    fn serve_tolerates_crlf_line_endings() {
        let root = test_dir("crlf");
        let project = root.join("proj");
        std::fs::create_dir_all(&project).expect("test directory should be created");
        let (sender, receiver) = mpsc::channel();
        serve_to(port_file("crlf"), move |request| {
            sender.send(request).expect("test receiver should be alive");
        })
        .expect("test server should start");

        // 绕过 `try_forward_to` 的规范写入，直接发 CRLF 行尾。
        let port: u16 = std::fs::read_to_string(port_file("crlf"))
            .expect("port file should exist")
            .trim()
            .parse()
            .expect("port file should hold a port");
        let canonical = dunce::canonicalize(&project).unwrap();
        let mut stream =
            TcpStream::connect(format!("127.0.0.1:{port}")).expect("test server should accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout should apply");
        stream
            .write_all(format!("{}\r\n", canonical.display()).as_bytes())
            .expect("request should send");
        assert_eq!(
            read_line(&stream, MAX_REQUEST_BYTES),
            Some("ok".to_string())
        );
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("request should arrive"),
            Some(canonical)
        );

        std::fs::remove_dir_all(&root).expect("test directory should be removed");
    }
}
