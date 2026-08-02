//! SFTP 后台工作器：在一条已认证会话上开 sftp 子系统 channel，提供
//! 浏览 / 下载 / 上传 / 建目录，并按 chunk 回报进度。
//!
//! `SftpSession` 自包含（持有 channel stream），由连接层开好后移入本 worker。
//! UI 通过 `SftpCmd`/`SftpEvent` 与之交互（同终端桥接模式）。

use std::path::PathBuf;

use async_channel::{Receiver, Sender};
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 编辑器允许读取和保存的最大文件大小。
pub const MAX_EDITOR_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// 远程目录条目（UI 友好快照）。
#[derive(Clone, Debug)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// UI → worker 命令。
#[allow(dead_code)]
pub enum SftpCmd {
    /// 列目录。
    List { path: String },
    /// 下载远端文件到本地路径。
    Download { remote: String, local: PathBuf },
    /// 上传本地文件到远端路径。
    Upload { local: PathBuf, remote: String },
    /// 读取远端文本文件。
    ReadFile { remote: String },
    /// 保存远端文本文件。
    WriteFile { remote: String, contents: Vec<u8> },
    /// 建目录。
    Mkdir { path: String },
}

/// worker → UI 事件。
#[derive(Debug)]
pub enum SftpEvent {
    /// 列目录结果。
    Listed {
        path: String,
        entries: Vec<RemoteEntry>,
    },
    /// 远端文件内容。
    FileRead { remote: String, contents: Vec<u8> },
    /// 传输进度。
    Progress {
        label: String,
        transferred: u64,
        total: Option<u64>,
    },
    /// 单个操作完成（ok=false 时 message 为错误）。
    Done {
        label: String,
        ok: bool,
        message: String,
    },
    /// 保存文件完成。
    Saved {
        remote: String,
        ok: bool,
        message: String,
    },
    /// worker 致命错误。
    Error(String),
    /// worker 结束。
    Closed,
}

/// SFTP worker 主循环：消费命令、回报事件；命令通道关闭即退出。
pub async fn run_sftp_worker(
    sftp: SftpSession,
    cmd_rx: Receiver<SftpCmd>,
    event_tx: Sender<SftpEvent>,
) {
    while let Ok(cmd) = cmd_rx.recv().await {
        let result: Result<(), String> = match cmd {
            SftpCmd::List { path } => match list_dir(&sftp, &path).await {
                Ok((abs, entries)) => {
                    let _ = event_tx
                        .send(SftpEvent::Listed { path: abs, entries })
                        .await;
                    Ok(())
                }
                Err(e) => Err(format!("list: {e}")),
            },
            SftpCmd::Mkdir { path } => sftp
                .create_dir(&path)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
            SftpCmd::Download { remote, local } => {
                let label = format!("↓ {remote}");
                let res = download(&sftp, &remote, &local, &event_tx, &label).await;
                report_done(&event_tx, label, res).await;
                continue;
            }
            SftpCmd::Upload { local, remote } => {
                let label = format!("↑ {remote}");
                let res = upload(&sftp, &local, &remote, &event_tx, &label).await;
                report_done(&event_tx, label, res).await;
                continue;
            }
            SftpCmd::ReadFile { remote } => match read_file(&sftp, &remote).await {
                Ok(contents) => {
                    let _ = event_tx
                        .send(SftpEvent::FileRead { remote, contents })
                        .await;
                    Ok(())
                }
                Err(e) => Err(format!("read {remote}: {e}")),
            },
            SftpCmd::WriteFile { remote, contents } => {
                let res = write_file(&sftp, &remote, &contents).await;
                report_saved(&event_tx, remote, res).await;
                continue;
            }
        };
        if let Err(e) = result {
            let _ = event_tx.send(SftpEvent::Error(e)).await;
        }
    }
    let _ = sftp.close().await;
    let _ = event_tx.send(SftpEvent::Closed).await;
}

/// 读取远端文件，编辑器只接收小于上限的内容。
async fn read_file(sftp: &SftpSession, remote: &str) -> Result<Vec<u8>, String> {
    let metadata = sftp.metadata(remote).await.map_err(|e| e.to_string())?;
    if metadata.len() > MAX_EDITOR_FILE_BYTES {
        return Err(rust_i18n::t!(
            "sftp.file_too_large",
            size = format_bytes(metadata.len()),
            limit = format_bytes(MAX_EDITOR_FILE_BYTES)
        )
        .to_string());
    }

    let mut remote_file = sftp.open(remote).await.map_err(|e| e.to_string())?;
    let mut contents = Vec::with_capacity(metadata.len() as usize);
    remote_file
        .read_to_end(&mut contents)
        .await
        .map_err(|e| e.to_string())?;
    remote_file.shutdown().await.ok();
    Ok(contents)
}

/// 保存编辑器内容，远端 `create` 会覆盖原文件。
async fn write_file(sftp: &SftpSession, remote: &str, contents: &[u8]) -> Result<String, String> {
    if contents.len() as u64 > MAX_EDITOR_FILE_BYTES {
        return Err(rust_i18n::t!(
            "sftp.file_too_large",
            size = format_bytes(contents.len() as u64),
            limit = format_bytes(MAX_EDITOR_FILE_BYTES)
        )
        .to_string());
    }

    let mut remote_file = sftp.create(remote).await.map_err(|e| e.to_string())?;
    remote_file
        .write_all(contents)
        .await
        .map_err(|e| e.to_string())?;
    remote_file.flush().await.map_err(|e| e.to_string())?;
    remote_file.shutdown().await.ok();
    Ok(format!("{} bytes", contents.len()))
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

async fn list_dir(sftp: &SftpSession, path: &str) -> Result<(String, Vec<RemoteEntry>), String> {
    // 规范化为绝对路径（首次 "." → 用户家目录），失败则保留原值。
    let abs = sftp
        .canonicalize(path)
        .await
        .unwrap_or_else(|_| path.to_string());
    let rd = sftp.read_dir(&abs).await.map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for e in rd {
        let md = e.metadata();
        out.push(RemoteEntry {
            name: e.file_name(),
            is_dir: md.is_dir(),
            size: md.len(),
        });
    }
    // 目录在前，名字字母序。
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    Ok((abs, out))
}

/// 下载：远端读 → 本地写，分块回报进度。
async fn download(
    sftp: &SftpSession,
    remote: &str,
    local: &PathBuf,
    event_tx: &Sender<SftpEvent>,
    label: &str,
) -> Result<String, String> {
    let total = sftp.metadata(remote).await.map(|m| m.len()).ok();
    let mut remote_file = sftp.open(remote).await.map_err(|e| e.to_string())?;
    if let Some(parent) = local.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    let mut local_file = tokio::fs::File::create(local)
        .await
        .map_err(|e| e.to_string())?;
    let mut transferred = 0u64;
    let mut buf = vec![0u8; 32 * 1024];
    loop {
        let n = remote_file
            .read(&mut buf)
            .await
            .map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        local_file
            .write_all(&buf[..n])
            .await
            .map_err(|e| e.to_string())?;
        transferred += n as u64;
        let _ = event_tx
            .send(SftpEvent::Progress {
                label: label.to_string(),
                transferred,
                total,
            })
            .await;
    }
    remote_file.shutdown().await.ok();
    Ok(format!("{transferred} bytes"))
}

/// 上传：本地读 → 远端写，分块回报进度。
async fn upload(
    sftp: &SftpSession,
    local: &PathBuf,
    remote: &str,
    event_tx: &Sender<SftpEvent>,
    label: &str,
) -> Result<String, String> {
    let total = tokio::fs::metadata(local).await.map(|m| m.len()).ok();
    let mut local_file = tokio::fs::File::open(local)
        .await
        .map_err(|e| e.to_string())?;
    let mut remote_file = sftp.create(remote).await.map_err(|e| e.to_string())?;
    let mut transferred = 0u64;
    let mut buf = vec![0u8; 32 * 1024];
    loop {
        let n = local_file.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        remote_file
            .write_all(&buf[..n])
            .await
            .map_err(|e| e.to_string())?;
        transferred += n as u64;
        let _ = event_tx
            .send(SftpEvent::Progress {
                label: label.to_string(),
                transferred,
                total,
            })
            .await;
    }
    remote_file.flush().await.ok();
    remote_file.shutdown().await.ok();
    Ok(format!("{transferred} bytes"))
}

async fn report_done(event_tx: &Sender<SftpEvent>, label: String, res: Result<String, String>) {
    match res {
        Ok(msg) => {
            let _ = event_tx
                .send(SftpEvent::Done {
                    label,
                    ok: true,
                    message: msg,
                })
                .await;
        }
        Err(e) => {
            let _ = event_tx
                .send(SftpEvent::Done {
                    label,
                    ok: false,
                    message: e,
                })
                .await;
        }
    }
}

async fn report_saved(event_tx: &Sender<SftpEvent>, remote: String, res: Result<String, String>) {
    let event = match res {
        Ok(message) => SftpEvent::Saved {
            remote,
            ok: true,
            message,
        },
        Err(message) => SftpEvent::Saved {
            remote,
            ok: false,
            message,
        },
    };
    let _ = event_tx.send(event).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_file_limit_is_rendered_human_readably() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(MAX_EDITOR_FILE_BYTES), "4.0 MB");
    }
}
