//! GPUI adapter for the pure SSH connection engine.

use std::sync::Arc;

use gpui::{App, AppContext, Entity, Task};
use tokio::sync::oneshot;

use crossh_core::config::{HostConfig, SshConfig};
use crossh_ssh::{
    AuthChoice, ConnEvent, ConnectionHandle, ConnectionState, CredentialKind, ForwardKind,
    HostKeyDecision, RemoteCommandEvent,
};
use crossh_ssh::{SftpCmd, SftpEvent};

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
        apply_event(&mut self.state, &mut self.pending_prompt, event);
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
        spec: crossh_core::config::ForwardSpec,
        kind: ForwardKind,
    ) -> oneshot::Receiver<Result<(), String>> {
        self.handle.start_forward(spec, kind)
    }

    pub(crate) fn stop_forward(&self, spec: crossh_core::config::ForwardSpec, kind: ForwardKind) {
        self.handle.stop_forward(spec, kind);
    }

    pub(crate) fn resolve_host_key(&mut self, decision: HostKeyDecision) {
        resolve_host_key(&mut self.pending_prompt, decision);
    }

    pub(crate) fn resolve_credential(&mut self, value: Option<String>) {
        resolve_credential(&mut self.pending_prompt, value);
    }
}

fn apply_event(
    state: &mut ConnectionState,
    pending_prompt: &mut Option<PendingPrompt>,
    event: ConnEvent,
) {
    match event {
        ConnEvent::Connected => *state = ConnectionState::Connected,
        ConnEvent::Error(error) => *state = ConnectionState::Error(error),
        ConnEvent::Closed => *state = ConnectionState::Closed,
        ConnEvent::NeedHostKey {
            host,
            port,
            key_type,
            fingerprint,
            changed,
            reply,
        } => {
            *pending_prompt = Some(PendingPrompt::HostKey {
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
            *pending_prompt = Some(PendingPrompt::Credential {
                kind,
                prompt,
                reply: Some(reply),
            });
        }
    }
}

fn resolve_host_key(pending_prompt: &mut Option<PendingPrompt>, decision: HostKeyDecision) {
    if let Some(PendingPrompt::HostKey { reply, .. }) = pending_prompt
        && let Some(reply) = reply.take()
    {
        let _ = reply.send(decision);
    }
    *pending_prompt = None;
}

fn resolve_credential(pending_prompt: &mut Option<PendingPrompt>, value: Option<String>) {
    if let Some(PendingPrompt::Credential { reply, .. }) = pending_prompt
        && let Some(reply) = reply.take()
    {
        let _ = reply.send(value);
    }
    *pending_prompt = None;
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.handle.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_events_drive_state_without_a_real_transport() {
        let mut state = ConnectionState::Connecting;
        let mut prompt = None;
        apply_event(&mut state, &mut prompt, ConnEvent::Connected);
        assert_eq!(state, ConnectionState::Connected);

        apply_event(&mut state, &mut prompt, ConnEvent::Error("denied".into()));
        assert_eq!(state, ConnectionState::Error("denied".into()));

        apply_event(&mut state, &mut prompt, ConnEvent::Closed);
        assert_eq!(state, ConnectionState::Closed);
    }

    #[test]
    fn host_key_prompt_reply_is_consumed_once() {
        let mut state = ConnectionState::Connecting;
        let mut prompt = None;
        let (reply, mut response) = oneshot::channel();
        apply_event(
            &mut state,
            &mut prompt,
            ConnEvent::NeedHostKey {
                host: "example.com".into(),
                port: 22,
                key_type: "ssh-ed25519".into(),
                fingerprint: "SHA256:test".into(),
                changed: false,
                reply,
            },
        );
        assert!(matches!(prompt, Some(PendingPrompt::HostKey { .. })));

        resolve_host_key(&mut prompt, HostKeyDecision::AcceptOnce);
        assert_eq!(response.try_recv(), Ok(HostKeyDecision::AcceptOnce));
        assert!(prompt.is_none());
        resolve_host_key(&mut prompt, HostKeyDecision::Reject);
        assert!(response.try_recv().is_err());
    }

    #[test]
    fn credential_prompt_can_be_cancelled() {
        let mut state = ConnectionState::Connecting;
        let mut prompt = None;
        let (reply, mut response) = oneshot::channel();
        apply_event(
            &mut state,
            &mut prompt,
            ConnEvent::NeedCredential {
                kind: CredentialKind::Password,
                prompt: "Password:".into(),
                reply,
            },
        );
        assert!(matches!(prompt, Some(PendingPrompt::Credential { .. })));
        resolve_credential(&mut prompt, None);
        assert_eq!(response.try_recv(), Ok(None));
        assert!(prompt.is_none());
    }
}
