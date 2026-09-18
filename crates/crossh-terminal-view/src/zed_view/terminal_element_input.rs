//! Mouse and IME input handling for the terminal element.
//! Positional split from `terminal_element.rs` (Zed revision 90d024b88abc91264d9a0ad260eb4f365fa695c3);
//! see that file's header for the split map.
// SPDX-License-Identifier: GPL-3.0-or-later

use gpui::{
    App, Bounds, Context, DispatchPhase, Entity, FocusHandle, Hitbox, InputHandler, MouseButton,
    MouseMoveEvent, MouseUpEvent, Pixels, Point as GpuiPoint, UTF16Selection, Window,
};
use terminal::{Modes, Terminal};

use super::TerminalElement;
use crate::view::TerminalView;

impl TerminalElement {
    fn generic_button_handler<E>(
        connection: Entity<Terminal>,
        focus_handle: FocusHandle,
        steal_focus: bool,
        f: impl Fn(&mut Terminal, &E, &mut Context<Terminal>),
    ) -> impl Fn(&E, &mut Window, &mut App) {
        move |event, window, cx| {
            if steal_focus {
                window.focus(&focus_handle, cx);
            } else if !focus_handle.is_focused(window) {
                return;
            }
            connection.update(cx, |terminal, cx| {
                f(terminal, event, cx);

                cx.notify();
            })
        }
    }

    fn right_button_handler(
        terminal: Entity<Terminal>,
        terminal_view: Entity<TerminalView>,
        focus_handle: FocusHandle,
    ) -> impl Fn(&MouseUpEvent, &mut Window, &mut App) {
        move |event, window, cx| {
            if !focus_handle.is_focused(window) {
                return;
            }

            let forward_to_terminal =
                terminal_view.update(cx, |terminal_view, _| terminal_view.take_right_mouse_down());
            if forward_to_terminal {
                terminal.update(cx, |terminal, terminal_cx| {
                    terminal.mouse_up(event, terminal_cx);
                    terminal_cx.notify();
                });
                cx.stop_propagation();
            }
        }
    }

    pub(super) fn register_mouse_listeners(
        &mut self,
        mode: Modes,
        hitbox: &Hitbox,
        window: &mut Window,
    ) {
        let focus = self.focus.clone();
        let terminal = self.terminal.clone();
        let terminal_view = self.terminal_view.clone();

        self.interactivity.on_mouse_down(MouseButton::Left, {
            let terminal = terminal.clone();
            let focus = focus.clone();

            move |e, window, cx| {
                window.focus(&focus, cx);
                terminal.update(cx, |terminal, cx| {
                    terminal.mouse_down(e, cx);
                    cx.notify();
                })
            }
        });

        window.on_mouse_event({
            let terminal = self.terminal.clone();
            let hitbox = hitbox.clone();
            let focus = focus.clone();
            move |e: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }

                if e.pressed_button.is_some() && !cx.has_active_drag() && focus.is_focused(window) {
                    let hovered = hitbox.is_hovered(window);

                    terminal.update(cx, |terminal, cx| {
                        if terminal.selection_started() || hovered {
                            terminal.mouse_drag(e, hitbox.bounds, cx);
                            cx.notify();
                        }
                    })
                }

                if hitbox.is_hovered(window) {
                    terminal.update(cx, |terminal, cx| {
                        terminal.mouse_move(e, cx);
                    })
                }
            }
        });

        self.interactivity.on_mouse_up(
            MouseButton::Left,
            TerminalElement::generic_button_handler(
                terminal.clone(),
                focus.clone(),
                false,
                move |terminal, e, cx| {
                    terminal.mouse_up(e, cx);
                },
            ),
        );
        self.interactivity.on_mouse_down(
            MouseButton::Middle,
            TerminalElement::generic_button_handler(
                terminal.clone(),
                focus.clone(),
                true,
                move |terminal, e, cx| {
                    terminal.mouse_down(e, cx);
                },
            ),
        );

        // Windows-Terminal-style right click: the press is either forwarded to
        // the PTY (application mouse-tracking mode, unless Shift is held) or
        // resolved by TerminalView as copy-selection / paste. No popup menu,
        // and a bare click never synthesizes a word selection.
        let forwards_right_click = mode.intersects(Modes::MOUSE_MODE);
        self.interactivity.on_mouse_down(MouseButton::Right, {
            let terminal = terminal.clone();
            let terminal_view = terminal_view.clone();
            let focus = focus.clone();
            move |event, window, cx| {
                let forward_to_terminal = forwards_right_click && !event.modifiers.shift;
                if forward_to_terminal {
                    terminal_view.update(cx, |terminal_view, _| {
                        terminal_view.set_right_mouse_forwarded(true);
                    });
                    window.focus(&focus, cx);
                    terminal.update(cx, |terminal, terminal_cx| {
                        terminal.mouse_down(event, terminal_cx);
                        terminal_cx.notify();
                    });
                } else {
                    terminal_view.update(cx, |terminal_view, terminal_cx| {
                        terminal_view.handle_right_click(window, terminal_cx);
                    });
                }
                cx.stop_propagation();
            }
        });

        self.interactivity.on_mouse_up(
            MouseButton::Right,
            TerminalElement::right_button_handler(
                terminal.clone(),
                terminal_view.clone(),
                focus.clone(),
            ),
        );
        self.interactivity.on_mouse_up_out(
            MouseButton::Right,
            TerminalElement::right_button_handler(terminal.clone(), terminal_view, focus.clone()),
        );

        self.interactivity.on_scroll_wheel({
            let terminal = self.terminal.clone();
            move |event, _window, cx| {
                terminal.update(cx, |terminal, terminal_cx| {
                    let multiplier =
                        terminal::terminal_settings::TerminalSettings::get_global(terminal_cx)
                            .scroll_multiplier
                            .max(0.01);
                    terminal.scroll_wheel(event, multiplier);
                    terminal_cx.notify();
                });
            }
        });

        // Mouse mode handlers: middle-button release is only needed when the
        // terminal application is tracking mouse input.
        if mode.intersects(Modes::MOUSE_MODE) {
            self.interactivity.on_mouse_up(
                MouseButton::Middle,
                TerminalElement::generic_button_handler(
                    terminal,
                    focus,
                    false,
                    move |terminal, e, cx| {
                        terminal.mouse_up(e, cx);
                    },
                ),
            );
        }
    }
}

pub(super) struct TerminalInputHandler {
    pub(super) terminal_view: Entity<TerminalView>,
    pub(super) cursor_bounds: Option<Bounds<Pixels>>,
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _cx: &mut App,
    ) -> Option<UTF16Selection> {
        // Always return a valid selection for IME positioning,
        // even in ALT_SCREEN mode (fullscreen TUI apps like opencode, vim, etc.)
        // The terminal still has a cursor position that should be used for IME candidate window placement.
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &mut self,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<std::ops::Range<usize>> {
        let marked_text = &self.terminal_view.read(cx).ime_marked_text;
        (!marked_text.is_empty()).then(|| 0..marked_text.encode_utf16().count())
    }

    fn text_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<std::ops::Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.terminal_view.update(cx, |view, view_cx| {
            view.ime_marked_text.clear();
            if !text.is_empty() {
                view.zed_terminal.update(view_cx, |terminal, _| {
                    terminal.input(text.as_bytes().to_vec())
                });
            }
            view_cx.notify();
        });

        window.invalidate_character_coordinates();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_marked_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) {
        self.terminal_view.update(cx, |view, view_cx| {
            view.ime_marked_text.clear();
            view.ime_marked_text.push_str(new_text);
            view_cx.notify();
        });
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
        self.terminal_view.update(cx, |view, view_cx| {
            view.ime_marked_text.clear();
            view_cx.notify();
        });
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let term_bounds = self
            .terminal_view
            .read(cx)
            .zed_terminal
            .read(cx)
            .last_content()
            .terminal_bounds;

        let mut bounds = self.cursor_bounds?;
        let offset_x = term_bounds.cell_width * range_utf16.start as f32;
        bounds.origin.x += offset_x;

        Some(bounds)
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }

    fn character_index_for_point(
        &mut self,
        _point: GpuiPoint<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }
}
