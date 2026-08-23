//! Steering / follow-up prompt queue, aligned with pi's `MessageQueue`.

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
    /// 对齐 pi 的 clearQueue：清空并返回原队列，用于恢复到输入框。
    pub fn clear_queue(&mut self) -> (Vec<String>, Vec<String>) {
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
}
