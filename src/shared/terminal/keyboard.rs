//! Protocol-owned keyboard mode state.
//!
//! Zed's terminal core owns the screen and exposes the standard terminal mode
//! bits. Kitty keyboard flags and xterm modifyOtherKeys are application-facing
//! input protocols, so Crossh keeps their small state machine here.

const MAX_KITTY_KEYBOARD_STACK: usize = 16;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct KittyScreenState {
    flags: u8,
    stack: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct KeyboardProtocolState {
    modify_other_keys: u8,
    main: KittyScreenState,
    alternate: KittyScreenState,
    alternate_screen: bool,
}

impl KeyboardProtocolState {
    pub(crate) fn modify_other_keys(&self) -> u8 {
        self.modify_other_keys
    }

    pub(crate) fn set_modify_other_keys(&mut self, level: u8) {
        if level <= 3 {
            self.modify_other_keys = level;
        }
    }

    pub(crate) fn kitty_flags(&self) -> u8 {
        self.active_kitty_state().flags
    }

    pub(crate) fn kitty_set(&mut self, bits: u8, behavior: u8) {
        let state = self.active_kitty_state_mut();
        match behavior {
            1 => state.flags = bits,
            2 => state.flags |= bits,
            3 => state.flags &= !bits,
            _ => {}
        }
    }

    pub(crate) fn kitty_push(&mut self, bits: u8) {
        let state = self.active_kitty_state_mut();
        if state.stack.len() == MAX_KITTY_KEYBOARD_STACK {
            state.stack.remove(0);
        }
        state.stack.push(state.flags);
        state.flags = bits;
    }

    pub(crate) fn kitty_pop(&mut self, count: u16) {
        let state = self.active_kitty_state_mut();
        let count = count.max(1);
        for _ in 0..count {
            state.flags = state.stack.pop().unwrap_or_default();
        }
    }

    pub(crate) fn switch_screen(&mut self, alternate: bool) {
        self.alternate_screen = alternate;
    }

    pub(crate) fn reset(&mut self) {
        self.modify_other_keys = 0;
        self.main = KittyScreenState::default();
        self.alternate = KittyScreenState::default();
    }

    fn active_kitty_state(&self) -> &KittyScreenState {
        if self.alternate_screen {
            &self.alternate
        } else {
            &self.main
        }
    }

    fn active_kitty_state_mut(&mut self) -> &mut KittyScreenState {
        if self.alternate_screen {
            &mut self.alternate
        } else {
            &mut self.main
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kitty_stack_is_independent_per_screen() {
        let mut state = KeyboardProtocolState::default();
        state.kitty_set(1, 1);
        state.kitty_push(2);
        state.switch_screen(true);
        assert_eq!(state.kitty_flags(), 0);
        state.kitty_set(4, 1);
        state.kitty_push(8);
        state.kitty_pop(1);
        assert_eq!(state.kitty_flags(), 4);
        state.switch_screen(false);
        assert_eq!(state.kitty_flags(), 1);
        state.kitty_pop(1);
        assert_eq!(state.kitty_flags(), 1);
    }

    #[test]
    fn kitty_pop_from_empty_stack_resets_flags() {
        let mut state = KeyboardProtocolState::default();
        state.kitty_set(0x1f, 1);
        state.kitty_pop(1);
        assert_eq!(state.kitty_flags(), 0);
    }

    #[test]
    fn reset_clears_modify_other_keys_and_both_keyboard_screens() {
        let mut state = KeyboardProtocolState::default();
        state.set_modify_other_keys(2);
        state.kitty_set(1, 1);
        state.switch_screen(true);
        state.kitty_set(2, 1);
        state.reset();
        assert_eq!(state.modify_other_keys(), 0);
        assert_eq!(state.kitty_flags(), 0);
        state.switch_screen(false);
        assert_eq!(state.kitty_flags(), 0);
    }
}
