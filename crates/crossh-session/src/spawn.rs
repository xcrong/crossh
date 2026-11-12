//! 真实 PTY 会话：`portable_pty` 子进程 + 阻塞读线程 + 广播。
//!
//! - 不用 `alacritty_terminal::tty::new`：它把 master fd 设成非阻塞并与
//!   `EventLoop` + poller 绑定（`tty/unix.rs:339`），脱离那条路径用要重写
//!   poller 循环；`portable_pty` 是阻塞读写，无事件循环耦合（D11）。
//! - 读线程把原始字节**直写**共享 `FeedState`（环 + 扫描，不丢）；
//!   广播只给实时客户端做带序号的尽力投递，消费慢会 lag（D9 可接受）。

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::sync::broadcast;

use crate::feed::FeedState;
use crossh_core::terminal::{LocalShellEnvironment, insert_zed_terminal_env};

/// 实时输出块：该块末尾在会话总字节流中的偏移 + 原始字节。
///
/// 推送与序号在 `FeedState` 锁内先后发生，广播保序；
/// 接入流程"先订阅、再快照、再过滤序号"做到不丢不重。
pub type OutputChunk = (u64, Vec<u8>);

/// 建会话参数。
pub struct SpawnOptions {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub cols: u16,
    pub rows: u16,
    pub env: Vec<(String, String)>,
    /// 交互 shell 集成脚本的守卫（拥有 rcfile 临时目录，`None` = 非集成 shell）。
    /// 随会话持有，`Drop` 时删除临时文件。
    pub shell_env: Option<LocalShellEnvironment>,
}

impl SpawnOptions {
    /// 交互式 shell 会话：按给定程序装配 `LocalShellEnvironment`（bash/zsh/fish
    /// 写 `--rcfile`/`ZDOTDIR` 集成脚本，不支持的 shell 降级为裸启动），
    /// 并注入终端标识环境变量（D11：不装集成等于什么都没做）。
    pub fn interactive_shell(
        program: &str,
        cwd: Option<PathBuf>,
        cols: u16,
        rows: u16,
    ) -> std::io::Result<Self> {
        let shell_env = LocalShellEnvironment::create(program).map_err(io_err)?;
        let (program, args, mut env): (String, Vec<String>, HashMap<String, String>) =
            match &shell_env {
                Some(env) => (
                    env.program().to_string(),
                    env.args().to_vec(),
                    env.env().iter().cloned().collect(),
                ),
                None => (program.to_string(), Vec::new(), HashMap::new()),
            };
        if !env.contains_key("TERM") {
            env.insert("TERM".to_string(), "xterm-256color".to_string());
        }
        insert_zed_terminal_env(&mut env, &env!("CARGO_PKG_VERSION"));
        Ok(Self {
            program,
            args,
            cwd,
            cols,
            rows,
            env: env.into_iter().collect(),
            shell_env,
        })
    }

    /// 一次性命令：直接执行 `argv[0]` + 参数，不装配交互 shell 集成脚本。
    /// 仍继承 daemon 环境并注入终端标识变量。
    pub fn raw(
        argv: &[String],
        cwd: Option<PathBuf>,
        cols: u16,
        rows: u16,
    ) -> std::io::Result<Self> {
        let program = argv
            .first()
            .cloned()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty command")
            })?;
        let mut env: HashMap<String, String> = HashMap::new();
        if std::env::var_os("TERM").is_none() {
            env.insert("TERM".to_string(), "xterm-256color".to_string());
        }
        insert_zed_terminal_env(&mut env, &env!("CARGO_PKG_VERSION"));
        Ok(Self {
            program,
            args: argv[1..].to_vec(),
            cwd,
            cols,
            rows,
            env: env.into_iter().collect(),
            shell_env: None,
        })
    }
}

/// 运行中的 PTY 会话。`Drop` 时杀子进程并回收读线程。
///
/// 字节归属：读线程把原始输出**直写**共享回放环（不丢）；广播只给实时
/// 客户端做尽力投递，消费慢的客户端会 lag（这是可接受的，见 D9）。
pub struct ChildSession {
    pub output_tx: broadcast::Sender<OutputChunk>,
    pub feed: Arc<Mutex<FeedState>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    reader: Option<thread::JoinHandle<()>>,
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    /// 交互 shell 集成脚本守卫（见 `SpawnOptions::shell_env`）。
    _shell_env: Option<LocalShellEnvironment>,
}

impl ChildSession {
    pub fn spawn(id: &str, opt: SpawnOptions) -> std::io::Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: opt.rows,
                cols: opt.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io_err)?;
        let mut cmd = CommandBuilder::new(&opt.program);
        cmd.args(&opt.args);
        if let Some(cwd) = &opt.cwd {
            cmd.cwd(cwd);
        }
        for (k, v) in &opt.env {
            cmd.env(k, v);
        }
        let master = pair.master;
        let mut reader = master.try_clone_reader().map_err(io_err)?;
        let writer = master.take_writer().map_err(io_err)?;
        let child = pair.slave.spawn_command(cmd).map_err(io_err)?;
        drop(pair.slave);

        let (output_tx, _) = broadcast::channel::<OutputChunk>(1024);
        let tx = output_tx.clone();
        let feed = Arc::new(Mutex::new(FeedState::new()));
        let feed_in = feed.clone();
        let owned_id = id.to_string();
        let reader_handle = thread::Builder::new()
            .name(format!("pty-read-{owned_id}"))
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let seq = match feed_in.lock() {
                                Ok(mut f) => {
                                    f.push(&buf[..n]);
                                    f.seq()
                                }
                                Err(_) => break,
                            };
                            // 实时广播尽力而为：无订阅/消费慢都不影响回放环。
                            let _ = tx.send((seq, buf[..n].to_vec()));
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(io_err)?;

        Ok(Self {
            output_tx,
            feed,
            child: Some(child),
            reader: Some(reader_handle),
            master: Some(master),
            writer: Some(writer),
            _shell_env: opt.shell_env,
        })
    }

    /// 向 PTY 写输入（controller 门控由调用方做，这里只写字节）。
    pub fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer
            .as_mut()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pty closed"))?
            .write_all(bytes)
    }

    /// 缩放 PTY（尺寸权威在服务端，见 D8）。
    pub fn resize(&mut self, cols: u16, rows: u16) -> std::io::Result<()> {
        self.master
            .as_mut()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pty closed"))?
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io_err)
    }

    /// 非阻塞收割退出码（未退出返回 `None`；句柄已移交返回 `None`）。
    pub fn try_wait(&mut self) -> std::io::Result<Option<i32>> {
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        match child.try_wait().map_err(io_err)? {
            Some(status) => Ok(Some(status.exit_code() as i32)),
            None => Ok(None),
        }
    }

    /// 发信号终止子进程（不回收）。退出码由持有句柄者的 `try_wait`/`wait` 收割。
    pub fn kill(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
    }

    /// 阻塞等到退出，返回退出码。
    pub fn wait(&mut self) -> std::io::Result<i32> {
        let Some(mut child) = self.child.take() else {
            return Ok(-1);
        };
        let status = child.wait().map_err(io_err)?;
        Ok(status.exit_code() as i32)
    }

    /// 等读线程把剩余输出吐完（子进程退出后调用）。
    pub fn join_reader(&mut self) {
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
    }
}

impl Drop for ChildSession {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // 先关 master/writer，让阻塞的 read 感知到 EOF 再 join，避免悬挂。
        self.writer.take();
        self.master.take();
        self.join_reader();
    }
}

fn io_err(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh_opts(cmd: &str) -> SpawnOptions {
        SpawnOptions {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), cmd.to_string()],
            cwd: None,
            cols: 80,
            rows: 24,
            env: vec![("TERM".to_string(), "xterm-256color".to_string())],
            shell_env: None,
        }
    }

    /// 收集会话从启动到退出的全部输出（读线程直写 feed，测试读快照）。
    fn run_to_exit(id: &str, opt: SpawnOptions) -> (i32, Vec<u8>) {
        let mut sess = ChildSession::spawn(id, opt).expect("spawn pty");
        // 轮询退出（测试用，不追求优雅）。
        let code = loop {
            if let Some(code) = sess.try_wait().expect("try_wait") {
                break code;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        sess.join_reader();
        let (_, out) = sess.feed.lock().expect("feed").snapshot();
        (code, out)
    }

    #[test]
    fn real_pty_output_enters_ring() {
        let (code, out) = run_to_exit("spawn-smoke", sh_opts("printf hello-pty"));
        assert_eq!(code, 0);
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("hello-pty"), "got {text:?}");
    }

    #[test]
    fn ring_truncation_keeps_tail_only() {
        // 600 KiB 输出 → 环只保留最近 512 KiB。
        let (code, out) = run_to_exit("spawn-truncate", sh_opts("head -c 614400 /dev/zero"));
        assert_eq!(code, 0);
        assert_eq!(out.len(), crate::session::REPLAY_LIMIT_BYTES);
        assert!(out.iter().all(|&b| b == 0));
    }

    #[test]
    fn exit_code_propagates() {
        let (code, _) = run_to_exit("spawn-exit3", sh_opts("exit 3"));
        assert_eq!(code, 3);
    }

    #[test]
    fn interactive_shell_installs_markers() {
        // 交互 shell 必须装上集成脚本：输出里出现 `crossh-command=` 标记（2.3）。
        let shell = ["/bin/bash", "/bin/zsh", "/bin/sh"]
            .into_iter()
            .find_map(|p| SpawnOptions::interactive_shell(p, None, 80, 24).ok())
            .filter(|o| o.shell_env.is_some())
            .expect("an integrated shell");
        assert!(
            shell.env.iter().any(|(k, _)| k == "TERM_PROGRAM"),
            "insert_zed_terminal_env applied"
        );
        let mut sess = ChildSession::spawn("spawn-interactive", shell).expect("spawn pty");
        // 进一次输入让 shell 跑一次 prompt 命令（`true` 无输出，只触发标记）。
        sess.write_all(b"true\n").expect("write");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let found = loop {
            {
                let feed = sess.feed.lock().expect("feed");
                let (_, out) = feed.snapshot();
                if out.windows(15).any(|w| w == b"crossh-command=") {
                    break true;
                }
            }
            if std::time::Instant::now() > deadline {
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        sess.write_all(b"exit\n").expect("exit");
        let _ = sess.wait();
        sess.join_reader();
        assert!(found, "integration markers never appeared");
    }
}
