//! AgentEventBus, mirroring pi-agent's `EventBus` + `AgentSessionEvent`.

use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentSessionEvent {
    AgentEnd {
        will_retry: bool,
    },
    AgentSettled,
    QueueUpdate {
        steering: Vec<String>,
        follow_up: Vec<String>,
    },
    CompactionStart {
        reason: String,
    },
    CompactionEnd {
        reason: String,
        aborted: bool,
        will_retry: bool,
    },
    EntryAppended {
        entry_id: String,
    },
    SessionInfoChanged {
        name: Option<String>,
    },
    ThinkingLevelChanged {
        level: String,
    },
    ModelChanged {
        provider: String,
        model_id: String,
    },
}

type Listener = Arc<dyn Fn(&AgentSessionEvent) + Send + Sync>;

#[derive(Default, Clone)]
pub struct EventBus {
    listeners: Arc<Mutex<Vec<Listener>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn subscribe<F>(&self, f: F)
    where
        F: Fn(&AgentSessionEvent) + Send + Sync + 'static,
    {
        self.listeners.lock().unwrap().push(Arc::new(f));
    }
    pub fn emit(&self, event: AgentSessionEvent) {
        // Snapshot Arc pointers while holding the lock, then release before calling.
        let snapshot = {
            let guard = self.listeners.lock().unwrap();
            guard.clone()
        };
        for l in snapshot.iter() {
            l(&event);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MessageQueue {
    pub steering: Vec<String>,
    pub follow_up: Vec<String>,
}

impl MessageQueue {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push_steering(&mut self, text: String) {
        self.steering.push(text);
    }
    pub fn push_follow_up(&mut self, text: String) {
        self.follow_up.push(text);
    }
    pub fn pop_next(&mut self) -> Option<String> {
        if !self.steering.is_empty() {
            Some(self.steering.remove(0))
        } else if !self.follow_up.is_empty() {
            Some(self.follow_up.remove(0))
        } else {
            None
        }
    }
    pub fn take_all(&mut self) -> (Vec<String>, Vec<String>) {
        (
            std::mem::take(&mut self.steering),
            std::mem::take(&mut self.follow_up),
        )
    }
    pub fn restore_to_input(&mut self, input: &mut String) {
        let mut all = Vec::new();
        all.append(&mut self.steering);
        all.append(&mut self.follow_up);
        if !all.is_empty() {
            if !input.is_empty() {
                input.push('\n');
            }
            input.push_str(&all.join("\n"));
        }
    }
    pub fn is_empty(&self) -> bool {
        self.steering.is_empty() && self.follow_up.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_20260821_agent_runtime_queue_update_emits() {
        let bus = EventBus::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        bus.subscribe(move |e| seen2.lock().unwrap().push(e.clone()));
        bus.emit(AgentSessionEvent::QueueUpdate {
            steering: vec!["a".into()],
            follow_up: vec!["b".into()],
        });
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn spec_20260821_agent_runtime_esc_restores_queue_to_input() {
        let mut q = MessageQueue::new();
        q.push_steering("steer".into());
        q.push_follow_up("follow".into());
        let mut input = String::from("current");
        q.restore_to_input(&mut input);
        assert!(input.contains("steer"));
        assert!(input.contains("follow"));
        assert!(q.is_empty());
    }

    #[test]
    fn event_bus_does_not_deadlock_when_listener_subscribes() {
        let bus = EventBus::new();
        let bus2 = bus.clone();
        bus.subscribe(move |_| {
            // Re-entrant subscribe must not deadlock because emit dropped the lock.
            bus2.subscribe(|_| {});
        });
        bus.emit(AgentSessionEvent::AgentSettled);
    }
}
