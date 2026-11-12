//! 帧编解码：`u32` 小端长度 + 1 字节 kind + 载荷。
//!
//! - 结构化载荷用 `serde_json`，输入/输出用原始字节（不带编码开销）。
//! - 帧头与载荷用一次 `write_vectored` 写出；socket 设 `TCP_NODELAY`（见 daemon 侧）。
//! - `MAX_FRAME = 8 MiB`：PTY 单次读远小于此，超出直接拒绝而不是尝试分配。

use std::io::{self, IoSlice, Write};

use serde::{Deserialize, Serialize};

/// 单帧上限 8 MiB（含 kind 字节，不含 4 字节长度头）。
pub const MAX_FRAME: usize = 8 * 1024 * 1024;

/// 帧 kind。数值稳定，属协议一部分，新增只能追加。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageKind {
    /// 握手请求（`HandshakeRequest` JSON）。
    Hello = 1,
    /// 握手成功应答（`HandshakeOk` JSON）。
    Welcome = 2,
    /// 握手/请求错误（`ErrorPayload` JSON，固定文案）。
    Error = 3,
    /// 会话列表请求（空载荷）。
    List = 4,
    /// 会话列表应答（JSON）。
    Listed = 5,
    /// 建会话请求（JSON）。
    Create = 6,
    /// 建会话应答（JSON）。
    Created = 7,
    /// 客户端 → 会话输入（原始字节）。
    Input = 8,
    /// 会话 → 客户端输出（原始字节）。
    Output = 9,
    /// 尺寸上报（JSON `{cols,rows}`）。
    Resize = 10,
    /// 关闭会话（JSON `{id}`）。
    Close = 11,
    /// 回放结束标记（JSON `{end_seq}`；回放与实时的边界帧）。
    ReplayEnd = 12,
    /// 接入已有会话（JSON `AttachRequest`）。
    Attach = 13,
    /// 接入应答（JSON `AttachedReply`；随后先到回放字节，再到 `ReplayEnd`）。
    Attached = 14,
    /// 会话结束通知（JSON `ExitedNotice`；服务端 → 当时在接的客户端）。
    Exited = 15,
    /// 停 daemon（空载荷；握手后已认证连接发送，服务端清理后退出进程）。
    Shutdown = 16,
}

impl MessageKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Hello),
            2 => Some(Self::Welcome),
            3 => Some(Self::Error),
            4 => Some(Self::List),
            5 => Some(Self::Listed),
            6 => Some(Self::Create),
            7 => Some(Self::Created),
            8 => Some(Self::Input),
            9 => Some(Self::Output),
            10 => Some(Self::Resize),
            11 => Some(Self::Close),
            12 => Some(Self::ReplayEnd),
            13 => Some(Self::Attach),
            14 => Some(Self::Attached),
            15 => Some(Self::Exited),
            16 => Some(Self::Shutdown),
            _ => None,
        }
    }
}

/// 编码一帧：4 字节小端长度（kind + 载荷）+ kind + 载荷。
pub fn encode_frame(kind: MessageKind, payload: &[u8]) -> io::Result<Vec<u8>> {
    if payload.len().saturating_add(1) > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame too large",
        ));
    }
    let len = (payload.len() + 1) as u32;
    let mut out = Vec::with_capacity(4 + 1 + payload.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.push(kind as u8);
    out.extend_from_slice(payload);
    Ok(out)
}

/// 单次 `write_vectored` 写出一帧（头 + kind + 载荷三分片一次调用）。
/// 返回写入字节数；调用方负责 flush。底层短写时循环补齐。
pub fn write_frame_vectored(
    w: &mut impl Write,
    kind: MessageKind,
    payload: &[u8],
) -> io::Result<usize> {
    if payload.len().saturating_add(1) > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame too large",
        ));
    }
    let len = ((payload.len() + 1) as u32).to_le_bytes();
    let kind_byte = [kind as u8];
    // 剩余三片的游标。
    let mut off0 = 0usize;
    let mut off1 = 0usize;
    let mut off2 = 0usize;
    let total = 4 + 1 + payload.len();
    let mut wrote = 0usize;
    while wrote < total {
        let slices = [
            IoSlice::new(&len[off0..]),
            IoSlice::new(&kind_byte[off1..]),
            IoSlice::new(&payload[off2..]),
        ];
        let n = w.write_vectored(&slices)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "write_vectored wrote zero",
            ));
        }
        // 按写入量推进三片游标。
        let mut rest = n;
        let take0 = (len.len() - off0).min(rest);
        off0 += take0;
        rest -= take0;
        let take1 = (kind_byte.len() - off1).min(rest);
        off1 += take1;
        rest -= take1;
        off2 += (payload.len() - off2).min(rest);
        wrote += n;
    }
    Ok(wrote)
}

/// 解码一帧。输入为完整帧（含 4 字节头）时返回 `(kind, payload)`；
/// 长度头声明超限返回错误；输入截断返回 `UnexpectedEof`，不 panic。
pub fn decode_frame(buf: &[u8]) -> io::Result<(MessageKind, Vec<u8>)> {
    if buf.len() < 4 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "truncated length header",
        ));
    }
    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if len < 1 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "empty frame"));
    }
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    if buf.len() - 4 < len {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "truncated payload",
        ));
    }
    let kind = MessageKind::from_u8(buf[4])
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unknown kind"))?;
    Ok((kind, buf[5..4 + len].to_vec()))
}

/// 会话级请求/应答载荷（`serde_json` 编码，字段只增不改）。
/// 全部 `deny_unknown_fields` 缺席：旧端忽略新字段（与能力协商一致）。
/// 建会话请求（`Create`）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CreateRequest {
    /// 指定 shell 程序（`None` = 系统默认）。
    #[serde(default)]
    pub shell: Option<String>,
    /// 初始工作目录（`None` = daemon 当前目录）。
    #[serde(default)]
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
    /// 非空时直接执行该命令（`[程序, 参数...]`），不装配交互 shell 集成
    /// 脚本（`run --` 一次性命令用；无 prompt、无 cwd 嗅探）。
    #[serde(default)]
    pub command: Option<Vec<String>>,
}

/// 会话信息（`Created`/`Listed`/`Attached` 共用）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SessionInfo {
    pub id: String,
    pub cwd: Option<String>,
    pub program: Option<String>,
    pub controller: Option<u64>,
    pub alive: bool,
    pub exit_code: Option<i32>,
}

/// 建会话应答（`Created`）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CreatedReply {
    pub session: SessionInfo,
}

/// 列表应答（`Listed`）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ListReply {
    pub sessions: Vec<SessionInfo>,
}

/// 接入请求（`Attach`）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AttachRequest {
    pub id: String,
}

/// 接入应答（`Attached`）：随后先到回放 `Output`，再到 `ReplayEnd`。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AttachedReply {
    pub session: SessionInfo,
    /// 回放快照的末端序号；客户端只转发 `end_seq` 更大的实时块。
    pub end_seq: u64,
}

/// 尺寸上报（`Resize`）：作用于当前接入绑定的会话。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ResizeRequest {
    pub cols: u16,
    pub rows: u16,
}

/// 关会话请求（`Close`）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CloseRequest {
    pub id: String,
}

/// 会话结束通知（`Exited`）：服务端 → 当时在接的客户端。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExitedNotice {
    pub id: String,
    pub code: i32,
}

/// 回放结束标记（`ReplayEnd`）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ReplayEndMarker {
    pub end_seq: u64,
}

/// `Output` 载荷：8 字节小端 `end_seq` + 原始字节。
///
/// `end_seq` 是该块末尾在会话总字节流中的偏移（`FeedState::seq`）。
/// 推送与序号在同一把锁内先后发生，广播保序 → 客户端按序号去重/续接。
pub fn encode_output(end_seq: u64, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + bytes.len());
    out.extend_from_slice(&end_seq.to_le_bytes());
    out.extend_from_slice(bytes);
    out
}

pub fn decode_output(payload: &[u8]) -> io::Result<(u64, Vec<u8>)> {
    if payload.len() < 8 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "short output"));
    }
    let mut seq = [0u8; 8];
    seq.copy_from_slice(&payload[..8]);
    Ok((u64::from_le_bytes(seq), payload[8..].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn roundtrip_structured_and_raw() {
        let payload = serde_json::json!({"cols": 80, "rows": 24})
            .to_string()
            .into_bytes();
        let frame = encode_frame(MessageKind::Resize, &payload).unwrap();
        let (kind, body) = decode_frame(&frame).unwrap();
        assert_eq!(kind, MessageKind::Resize);
        assert_eq!(body, payload);

        let raw = vec![0x1b, b'[', b'A', 0x0a];
        let frame = encode_frame(MessageKind::Output, &raw).unwrap();
        let (kind, body) = decode_frame(&frame).unwrap();
        assert_eq!(kind, MessageKind::Output);
        assert_eq!(body, raw);
    }

    #[test]
    fn oversize_rejected_before_alloc() {
        let big = vec![0u8; MAX_FRAME];
        assert!(encode_frame(MessageKind::Output, &big).is_err());
        let mut header = (MAX_FRAME as u32 + 1).to_le_bytes().to_vec();
        header.push(MessageKind::Hello as u8);
        assert!(decode_frame(&header).is_err());
    }

    #[test]
    fn truncated_input_is_error_not_panic() {
        assert!(decode_frame(&[]).is_err());
        assert!(decode_frame(&[1, 0]).is_err());
        let frame = encode_frame(MessageKind::Input, b"hi").unwrap();
        assert!(decode_frame(&frame[..frame.len() - 1]).is_err());
    }

    /// 单帧只产生一次 `write_vectored`（自定义 `Write` 计数）。
    #[test]
    fn single_write_vectored_per_frame() {
        struct Counting {
            calls: Rc<Cell<usize>>,
            buf: Vec<u8>,
        }
        impl Write for Counting {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.buf.extend_from_slice(b);
                Ok(b.len())
            }
            fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
                self.calls.set(self.calls.get() + 1);
                let mut n = 0;
                for b in bufs {
                    self.buf.extend_from_slice(b);
                    n += b.len();
                }
                Ok(n)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let calls = Rc::new(Cell::new(0));
        let mut w = Counting {
            calls: calls.clone(),
            buf: Vec::new(),
        };
        write_frame_vectored(&mut w, MessageKind::Input, b"hello").unwrap();
        assert_eq!(calls.get(), 1);
        let (kind, body) = decode_frame(&w.buf).unwrap();
        assert_eq!(kind, MessageKind::Input);
        assert_eq!(body, b"hello");
    }
}
