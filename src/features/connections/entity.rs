//! GPUI adapter for the pure SSH connection engine.

use std::sync::Arc;

use gpui::{App, AppContext, Entity, Task};
use tokio::sync::oneshot;

use crate::infrastructure::config::{HostConfig, SshConfig};
use crate::infrastructure::ssh::{
    AuthChoice, ConnEvent, ConnectionHandle, ConnectionState, CredentialKind, ForwardKind,
    HostKeyDecision, RemoteCommandEvent,
};
use crate::infrastructure::ssh::{SftpCmd, SftpEvent};

/// A UI-owned connection entity. All network work lives in `ConnectionHandle`.
pub(crate) struct Connection {
    handle: ConnectionHandle,
    pub(crate) state: ConnectionState,
    pub(crate) pending_prompt: Option<PendingPrompt>,
    _drain: Option<Task<()>>,
}

/// A prompt waiting for a user response.
pub(crate) enum PendingPrompt {
    HostKey {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
        changed: bool,
        reply: Option<oneshot::Sender<HostKeyDecision>>,
    },
    Credential {
        kind: CredentialKind,
        prompt: String,
        reply: Option<oneshot::Sender<Option<String>>>,
    },
}

impl Connection {
    pub(crate) fn open(
        host: HostConfig,
        methods: Vec<AuthChoice>,
        ssh_config: Arc<SshConfig>,
        cx: &mut App,
    ) -> Entity<Self> {
        let (handle, event_rx) = ConnectionHandle::start(host, methods, ssh_config);
        let entity = cx.new(|_| Self {
            handle,
            state: ConnectionState::Connecting,
            pending_prompt: None,
            _drain: None,
        });

        let weak = entity.downgrade();
        let drain = cx.spawn(async move |cx| {
            while let Ok(event) = event_rx.recv().await {
                let updated = weak.update(cx, |this, cx| {
                    this.apply_event(event);
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
            }
        });
        entity.update(cx, |this, _| this._drain = Some(drain));
        entity
    }

    fn apply_event(&mut self, event: ConnEvent) {
        match event {
            ConnEvent::Connected => self.state = ConnectionState::Connected,
            ConnEvent::Error(error) => self.state = ConnectionState::Error(error),
            ConnEvent::Closed => self.state = ConnectionState::Closed,
            ConnEvent::NeedHostKey {
                host,
                port,
                key_type,
                fingerprint,
                changed,
                reply,
            } => {
                self.pending_prompt = Some(PendingPrompt::HostKey {
                    host,
                    port,
                    key_type,
                    fingerprint,
                    changed,
                    reply: Some(reply),
                });
            }
            ConnEvent::NeedCredential {
                kind,
                prompt,
                reply,
            } => {
                self.pending_prompt = Some(PendingPrompt::Credential {
                    kind,
                    prompt,
                    reply: Some(reply),
                });
            }
        }
    }

    pub(crate) fn open_command(
        &mut self,
        command: String,
        cwd: String,
    ) -> (u64, async_channel::Receiver<RemoteCommandEvent>) {
        self.handle.open_command(command, cwd)
    }

    pub(crate) fn stop_command(&self, id: u64) {
        self.handle.stop_command(id);
    }

    pub(crate) fn open_sftp(
        &self,
    ) -> (
        async_channel::Sender<SftpCmd>,
        async_channel::Receiver<SftpEvent>,
    ) {
        self.handle.open_sftp()
    }

    pub(crate) fn start_forward(
        &self,
        spec: crate::infrastructure::config::ForwardSpec,
        kind: ForwardKind,
    ) -> oneshot::Receiver<Result<(), String>> {
        self.handle.start_forward(spec, kind)
    }

    pub(crate) fn stop_forward(
        &self,
        spec: crate::infrastructure::config::ForwardSpec,
        kind: ForwardKind,
    ) {
        self.handle.stop_forward(spec, kind);
    }

    pub(crate) fn resolve_host_key(&mut self, decision: HostKeyDecision) {
        if let Some(PendingPrompt::HostKey { reply, .. }) = &mut self.pending_prompt
            && let Some(reply) = reply.take()
        {
            let _ = reply.send(decision);
        }
        self.pending_prompt = None;
    }

    pub(crate) fn resolve_credential(&mut self, value: Option<String>) {
        if let Some(PendingPrompt::Credential { reply, .. }) = &mut self.pending_prompt
            && let Some(reply) = reply.take()
        {
            let _ = reply.send(value);
        }
        self.pending_prompt = None;
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.handle.shutdown();
    }
}
