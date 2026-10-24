//! The match's controls: the window's keys and mouse buttons as the original's key ids (`docs/FECK_VM.md` "Key
//! bindings": DirectInput scan codes + 42 for the keyboard, 11-18 the mouse), through the key bindings
//! (`qwc_feck::keys`, the defaults) into the gameplay pad variable's words (`qwc_sim::human`).
//!
//! A gamepad (`crate::gamepad`) adds its sticks as the analogue axes and its buttons as actions
//! ([`Controls::gamepad`]).
//!
//! The bindings are the front end's (its Controls menu, the file beside the save, [`Controls::bound`]) in a match
//! started from it, the file's in `qwc-game play`, else the defaults.
//!
//! **[I]** one player has the keyboard to himself: player 0's group (the arrows, the numpad, Return, the mouse) and
//! player 1's (WASD, J K H U Y I, Space) both drive him. The default bindings: move on the arrows or WASD; Action
//! (pass with the ball, tackle without) on Numpad 5, J or the left mouse button; Shoot on Numpad 6, K or the right
//! button; Boost on Numpad 4 or H; Dodge on Numpad 8 or U; the combos on Numpad 7 and 9, Y and I.

use std::collections::HashSet;

use qwc_feck::keys::{DEFAULTS, END, SEPARATOR};
use qwc_sim::human::{action, Pad};
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

/// A key's DirectInput scan code (`DIK_*`).
fn scan_code(k: KeyCode) -> Option<u32> {
    use KeyCode::*;
    Some(match k {
        Escape => 0x01,
        Digit1 => 0x02,
        Digit2 => 0x03,
        Digit3 => 0x04,
        Digit4 => 0x05,
        Digit5 => 0x06,
        Digit6 => 0x07,
        Digit7 => 0x08,
        Digit8 => 0x09,
        Digit9 => 0x0a,
        Digit0 => 0x0b,
        Minus => 0x0c,
        Equal => 0x0d,
        Backspace => 0x0e,
        Tab => 0x0f,
        KeyQ => 0x10,
        KeyW => 0x11,
        KeyE => 0x12,
        KeyR => 0x13,
        KeyT => 0x14,
        KeyY => 0x15,
        KeyU => 0x16,
        KeyI => 0x17,
        KeyO => 0x18,
        KeyP => 0x19,
        BracketLeft => 0x1a,
        BracketRight => 0x1b,
        Enter => 0x1c,
        ControlLeft => 0x1d,
        KeyA => 0x1e,
        KeyS => 0x1f,
        KeyD => 0x20,
        KeyF => 0x21,
        KeyG => 0x22,
        KeyH => 0x23,
        KeyJ => 0x24,
        KeyK => 0x25,
        KeyL => 0x26,
        Semicolon => 0x27,
        Quote => 0x28,
        Backquote => 0x29,
        ShiftLeft => 0x2a,
        Backslash => 0x2b,
        KeyZ => 0x2c,
        KeyX => 0x2d,
        KeyC => 0x2e,
        KeyV => 0x2f,
        KeyB => 0x30,
        KeyN => 0x31,
        KeyM => 0x32,
        Comma => 0x33,
        Period => 0x34,
        Slash => 0x35,
        ShiftRight => 0x36,
        NumpadMultiply => 0x37,
        AltLeft => 0x38,
        Space => 0x39,
        Numpad7 => 0x47,
        Numpad8 => 0x48,
        Numpad9 => 0x49,
        NumpadSubtract => 0x4a,
        Numpad4 => 0x4b,
        Numpad5 => 0x4c,
        Numpad6 => 0x4d,
        NumpadAdd => 0x4e,
        Numpad1 => 0x4f,
        Numpad2 => 0x50,
        Numpad3 => 0x51,
        Numpad0 => 0x52,
        NumpadDecimal => 0x53,
        NumpadEnter => 0x9c,
        ControlRight => 0x9d,
        NumpadDivide => 0xb5,
        AltRight => 0xb8,
        Home => 0xc7,
        ArrowUp => 0xc8,
        PageUp => 0xc9,
        ArrowLeft => 0xcb,
        ArrowRight => 0xcd,
        End => 0xcf,
        ArrowDown => 0xd0,
        PageDown => 0xd1,
        Insert => 0xd2,
        Delete => 0xd3,
        _ => return None,
    })
}

/// A key's id in the bindings (`General_GetRawKey`, `0x508960`).
pub fn key_id(k: KeyCode) -> Option<u32> {
    scan_code(k).map(|c| c + 42)
}

/// A mouse button's id (11-18, `0xeb0af8`): **[I]** left 11, right 12, middle 13.
pub fn mouse_id(b: MouseButton) -> Option<u32> {
    match b {
        MouseButton::Left => Some(11),
        MouseButton::Right => Some(12),
        MouseButton::Middle => Some(13),
        _ => None,
    }
}

/// The actions bound to `key` in groups 0 and 1 of `table`.
fn actions(table: &[u32], key: u32) -> Vec<usize> {
    let mut group = 0;
    let mut out = Vec::new();
    for &w in table {
        match w {
            END => break,
            SEPARATOR => group += 1,
            _ if group <= 1 && w >> 16 == key => out.push((w & 0xffff) as usize),
            _ => {}
        }
    }
    out
}

/// The keys held and the pad made from them each tick.
#[derive(Default)]
pub(crate) struct Controls {
    held: HashSet<u32>,
    /// Keys that went down since the last tick: a tap shorter than a tick (1/24 s) still counts for one.
    latched: HashSet<u32>,
    /// Keys pressed by a script (`--keys`), held for some ticks more.
    taps: Vec<(u32, u32)>,
    /// The player's gamepad this tick, when one is connected (`crate::gamepad`): its sticks are the analogue axes,
    /// and its buttons and d-pad the actions, on top of the keys.
    pub gamepad: Option<crate::gamepad::PadState>,
    pub pad: Pad,
    /// The key bindings' table (`qwc_feck::keys`): none for the defaults.
    bindings: Option<Vec<u32>>,
}

impl Controls {
    /// Controls on the bindings `table`.
    pub fn bound(table: &[u32]) -> Controls {
        Controls { bindings: Some(table.to_vec()), ..Default::default() }
    }

    /// Controls on the bindings saved beside the save (`crate::record::keys_file`), else the defaults.
    pub fn saved() -> Controls {
        match crate::record::keys_file().filter(|f| f.is_file()) {
            Some(f) => Controls::bound(&qwc_feck::keys::Keys::with_file(f).table),
            None => Controls::default(),
        }
    }

    pub fn set(&mut self, id: u32, down: bool) {
        if down {
            self.held.insert(id);
            self.latched.insert(id);
        } else {
            self.held.remove(&id);
        }
    }

    /// One tick (`0x4031d0`): every action a held key is bound to is down. A keyboard has no analog axes.
    /// Hold key `id` for the next `ticks` ticks (a scripted press).
    pub fn tap(&mut self, id: u32, ticks: u32) {
        self.taps.push((id, ticks));
    }

    pub fn tick(&mut self) {
        let mut words = [0u16; action::COUNT];
        let tapped: Vec<u32> = self.taps.iter().map(|t| t.0).collect();
        self.taps.iter_mut().for_each(|t| t.1 -= 1);
        self.taps.retain(|t| t.1 > 0);
        let latched = std::mem::take(&mut self.latched);
        let table = self.bindings.as_deref().unwrap_or(&DEFAULTS);
        for &k in self.held.iter().chain(&tapped).chain(&latched) {
            for a in actions(table, k).into_iter().filter(|&a| a < action::COUNT) {
                words[a] = 0xffff;
            }
        }
        let analog = 1 << action::MOVE_V[0] | 1 << action::MOVE_H[0] | 1 << action::PASS_V[0] | 1 << action::PASS_H[0];
        let mut present = !analog;
        if let Some(g) = self.gamepad {
            use crate::gamepad::axis_word;
            // the axes' words: across right positive, the vertical's up negative
            words[action::MOVE_H[0]] = axis_word(g.left[0]);
            words[action::MOVE_V[0]] = axis_word(-g.left[1]);
            words[action::PASS_H[0]] = axis_word(g.right[0]);
            words[action::PASS_V[0]] = axis_word(-g.right[1]);
            present |= analog;
            for (a, w) in words.iter_mut().enumerate() {
                if g.actions & 1 << a != 0 {
                    *w = 0xffff;
                }
            }
        }
        self.pad.update(&words, present);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_keys() {
        let id = |k| key_id(k).unwrap();
        let actions = |k| actions(&DEFAULTS, k);
        assert_eq!(actions(id(KeyCode::ArrowUp)), [action::MOVE_V[1]]);
        assert_eq!(actions(id(KeyCode::KeyD)), [action::MOVE_H[2]]);
        assert_eq!(actions(id(KeyCode::Numpad5)), [action::ACTION]);
        assert_eq!(actions(id(KeyCode::KeyK)), [action::SHOOT, action::SPECIAL]);
        assert_eq!(actions(mouse_id(MouseButton::Left).unwrap()), [action::ACTION]);
        let mut c = Controls::default();
        c.set(id(KeyCode::ArrowLeft), true);
        c.set(id(KeyCode::KeyJ), true);
        c.tick();
        assert!((c.pad.axes[1] + 1.0).abs() < 1e-4);
        assert!(c.pad.pressed(action::ACTION));
    }

    #[test]
    fn rebound_keys_drive_the_match() {
        let id = |k| key_id(k).unwrap();
        let mut keys = qwc_feck::keys::Keys::default();
        // player 0's shoot on F: K (player 1's shoot) still shoots, F now does too, and Numpad 6 no longer
        keys.rebind(0, 0x10, id(KeyCode::KeyF));
        let mut c = Controls::bound(&keys.table);
        c.set(id(KeyCode::KeyF), true);
        c.tick();
        assert!(c.pad.pressed(action::SHOOT));
        let mut c = Controls::bound(&keys.table);
        c.set(id(KeyCode::Numpad6), true);
        c.tick();
        assert!(!c.pad.held(action::SHOOT));
    }

    #[test]
    fn a_tap_between_two_ticks_counts() {
        let id = key_id(KeyCode::KeyJ).unwrap();
        let mut c = Controls::default();
        c.tick();
        // down and up again before the tick: pressed on it, and let go on the next
        c.set(id, true);
        c.set(id, false);
        c.tick();
        assert!(c.pad.pressed(action::ACTION));
        c.tick();
        assert!(!c.pad.held(action::ACTION));
    }

    #[test]
    fn a_gamepads_sticks_are_the_analogue_axes() {
        use crate::gamepad::PadState;
        // the left stick half up and fully right; the right stick down; Dodge held
        let pad = PadState { left: [1.0, 0.5], right: [0.0, -1.0], actions: 1 << action::DODGE };
        let mut c = Controls { gamepad: Some(pad), ..Default::default() };
        c.tick();
        // the dead zone takes 0.2 and scales the rest by 1.25: 0.5 up is -(0.5 - 0.2) * 1.25
        assert!((c.pad.axes[0] + 0.375).abs() < 1e-3, "{:?}", c.pad.axes);
        assert!((c.pad.axes[1] - 1.0).abs() < 1e-3);
        assert!((c.pad.axes[2] - 1.0).abs() < 1e-3);
        assert!(c.pad.pressed(action::DODGE));
        // centred: nothing moves
        c.gamepad = Some(PadState::default());
        c.tick();
        assert_eq!(c.pad.axes, [0.0; 4]);
    }
}
