//! PTY 旁路监听（TeePty）。
//!
//! alacritty 的 `EventLoop` 直接消费 PTY 输出，vte 0.15 的 `ansi::Handler`
//! 没有未知 OSC 的覆写点，因此唯一的最小拦截位置是 PTY 本身：用 [`TeePty`]
//! 包装真实 PTY，读取路径上先经 [`TeeReader`] 流式扫描 OSC 7501，再把原始
//! 字节原样交给终端解析器。写路径与子进程事件全部零改动转发。
//!
//! 所有权设计：内层 PTY 整体放在读端 [`TeeReader`] 里；写端经由
//! `self.reader.pty.writer()` 的借用链取到，无需复制句柄或 unsafe。

use std::io;
use std::sync::Arc;

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite};
use crossh_terminal::Osc7501Scanner;
use futures::channel::mpsc::UnboundedSender;
use polling::{Event, PollMode, Poller};

use crate::{PtyEvent, TerminalBackendEvent};

/// 包装真实 PTY，在读路径上旁路收集 OSC 7501 状态报告。
pub(super) struct TeePty<P: EventedReadWrite> {
    reader: TeeReader<P>,
}

impl<P: EventedReadWrite> TeePty<P> {
    pub(super) fn new(pty: P, events_tx: UnboundedSender<PtyEvent>) -> Self {
        Self {
            reader: TeeReader {
                pty,
                scanner: Osc7501Scanner::default(),
                events_tx,
            },
        }
    }
}

impl<P: EventedReadWrite> EventedReadWrite for TeePty<P> {
    type Reader = TeeReader<P>;
    type Writer = <P as EventedReadWrite>::Writer;

    // SAFETY: 仅转发内层 PTY 的注册，来源存活契约由内层实现维持。
    unsafe fn register(
        &mut self,
        poll: &Arc<Poller>,
        interest: Event,
        mode: PollMode,
    ) -> io::Result<()> {
        unsafe { self.reader.pty.register(poll, interest, mode) }
    }

    fn reregister(
        &mut self,
        poll: &Arc<Poller>,
        interest: Event,
        mode: PollMode,
    ) -> io::Result<()> {
        self.reader.pty.reregister(poll, interest, mode)
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        self.reader.pty.deregister(poll)
    }

    fn reader(&mut self) -> &mut Self::Reader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut Self::Writer {
        self.reader.pty.writer()
    }
}

impl<P: EventedPty> EventedPty for TeePty<P> {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.reader.pty.next_child_event()
    }
}

impl<P: EventedReadWrite + OnResize> OnResize for TeePty<P> {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.reader.pty.on_resize(window_size);
    }
}

/// 读端包装：先把读到的字节喂给 OSC 7501 扫描器，再原样返回。
pub(super) struct TeeReader<P: EventedReadWrite> {
    pty: P,
    scanner: Osc7501Scanner,
    events_tx: UnboundedSender<PtyEvent>,
}

impl<P: EventedReadWrite> io::Read for TeeReader<P> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.pty.reader().read(buf)?;
        if n > 0 {
            for update in self.scanner.feed(&buf[..n]) {
                let event = PtyEvent::Event(TerminalBackendEvent::ProgramStatus(update));
                if self.events_tx.unbounded_send(event).is_err() {
                    // 接收端已关闭：无法上报，读取本身不受影响
                    break;
                }
            }
        }
        Ok(n)
    }
}
