//! Not the original's: gamepads (XInput and the platforms' own, through `gilrs`), for the match and the menus.
//!
//! The original read joysticks through DirectInput, and its bindings name any input by a key id
//! (`docs/FECK_VM.md`, "Key bindings"). Its joystick groups (2 and 3 of the default table, `qwc_feck::keys`) put
//! the move stick on two axes, the hat's four directions on the stick's halves, and seven buttons from button 0:
//! Action, Shoot (and the Special move), Boost, Dodge, the last button (the uber move), the left combo and the right
//! combo. A modern pad follows that order on its face buttons:
//!
//! | Pad | Match | Menus |
//! |---|---|---|
//! | left stick, d-pad | move (the stick is analogue) | up, down, left, right |
//! | right stick | the second stick (the pass and shot aim) | |
//! | A (south) | Action: pass, or tackle | choose |
//! | B (east) | Shoot, and the Special move | cancel |
//! | X (west) | Boost | front-end button 8 (Space) |
//! | Y (north) | Dodge | front-end button 4 (Tab, the Inventory) |
//! | LB, RB | the left and the right combo | |
//! | LT | the uber move (the pad's last button) | |
//! | RT | Boost too | |
//! | Start | | front-end button `0x200` (`Match.vso` pauses on it) |
//! | Back | | front-end button `0x100` |
//!
//! The players take the pads in the order a button is first pressed on them: a pad that never sends anything, as
//! some keyboards' and receivers' game-controller interfaces do, takes no one's place. `qwc-game pads` lists the
//! pads and prints what each sends.

use gilrs::{Axis, Button, EventType, GamepadId, Gilrs};
use qwc_feck::pad::PadWord;
use qwc_sim::human::action;

/// One pad as the match takes it: the two sticks (`x` right, `y` up, -1 to 1), and the actions held.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PadState {
    pub left: [f32; 2],
    pub right: [f32; 2],
    /// Bit `a` set: action `a` (`qwc_sim::human::action`) held.
    pub actions: u32,
}

/// The pad's buttons for the match's actions.
const ACTIONS: [(Button, &[usize]); 8] = [
    (Button::South, &[action::ACTION]),
    (Button::East, &[action::SHOOT, action::SPECIAL]),
    (Button::West, &[action::BOOST]),
    (Button::RightTrigger2, &[action::BOOST]),
    (Button::North, &[action::DODGE]),
    (Button::LeftTrigger, &[action::COMBO_LEFT]),
    (Button::RightTrigger, &[action::COMBO_RIGHT]),
    (Button::LeftTrigger2, &[action::LAST]),
];

/// The d-pad's directions as the move stick's digital halves.
const DPAD: [(Button, usize); 4] = [
    (Button::DPadUp, action::MOVE_V[1]),
    (Button::DPadDown, action::MOVE_V[2]),
    (Button::DPadLeft, action::MOVE_H[1]),
    (Button::DPadRight, action::MOVE_H[2]),
];

/// The pad's buttons for the menus' pad words.
const MENU: [(Button, PadWord); 10] = [
    (Button::DPadUp, PadWord::Up),
    (Button::DPadDown, PadWord::Down),
    (Button::DPadLeft, PadWord::Left),
    (Button::DPadRight, PadWord::Right),
    (Button::South, PadWord::Choose),
    (Button::East, PadWord::Cancel),
    (Button::West, PadWord::Button8),
    (Button::North, PadWord::Button4),
    (Button::Start, PadWord::Button200),
    (Button::Select, PadWord::Button100),
];

/// How far the left stick must lean to move through a menu, and how far back it must come to let go.
const MENU_PRESS: f32 = 0.6;
const MENU_RELEASE: f32 = 0.4;

/// The connected pads, and which player has each.
pub struct Gamepads {
    gilrs: Option<Gilrs>,
    order: Vec<GamepadId>,
    /// The pads in the order they were first used: player 1's first.
    players: Vec<GamepadId>,
    /// The left stick's menu directions held, per player (up, down, left, right).
    stick: [[bool; 4]; 2],
    /// A button went down since the last [`Gamepads::take_pressed`] (to skip a movie or a cutscene).
    pressed: bool,
    /// The actions whose buttons went down since the pad's last [`Gamepads::state`]: a tap shorter than a tick
    /// still counts for one.
    latched: Vec<(GamepadId, u32)>,
}

/// The actions button `b` gives in the match.
fn button_actions(b: Button) -> u32 {
    let mut out = 0;
    for (k, acts) in ACTIONS {
        if k == b {
            out |= acts.iter().fold(0, |m, &a| m | 1 << a);
        }
    }
    for (k, a) in DPAD {
        if k == b {
            out |= 1 << a;
        }
    }
    out
}

impl Default for Gamepads {
    fn default() -> Gamepads {
        Gamepads::new()
    }
}

impl Gamepads {
    /// The pads (none when the platform's input cannot be opened, which is said once).
    pub fn new() -> Gamepads {
        let gilrs = match Gilrs::new() {
            Ok(g) => Some(g),
            Err(e) => {
                eprintln!("gamepads: {e}");
                None
            }
        };
        let order = gilrs.as_ref().map_or_else(Vec::new, |g| g.gamepads().map(|(id, _)| id).collect());
        for (i, id) in order.iter().enumerate() {
            if let Some(g) = &gilrs {
                eprintln!("gamepad {}: {}", i + 1, g.gamepad(*id).name());
            }
        }
        Gamepads { gilrs, order, players: Vec::new(), stick: [[false; 4]; 2], pressed: false, latched: Vec::new() }
    }

    /// Take the pads' events (connections, presses): call once a tick before reading them.
    pub fn poll(&mut self) {
        let Some(g) = &mut self.gilrs else { return };
        while let Some(e) = g.next_event() {
            match e.event {
                EventType::Connected if !self.order.contains(&e.id) => {
                    eprintln!("gamepad {} connected: {}", self.order.len() + 1, g.gamepad(e.id).name());
                    self.order.push(e.id);
                }
                EventType::Disconnected => {
                    eprintln!("gamepad disconnected: {}", g.gamepad(e.id).name());
                    self.order.retain(|&id| id != e.id);
                    self.players.retain(|&id| id != e.id);
                }
                EventType::ButtonPressed(b, _) => {
                    self.pressed = true;
                    match self.latched.iter_mut().find(|l| l.0 == e.id) {
                        Some(l) => l.1 |= button_actions(b),
                        None => self.latched.push((e.id, button_actions(b))),
                    }
                }
                _ => {}
            }
            // a press claims a player (an axis could rest away from its middle on a device that is no pad)
            let used = matches!(e.event, EventType::ButtonPressed(..));
            if used && !self.players.contains(&e.id) && self.order.contains(&e.id) {
                self.players.push(e.id);
                eprintln!("gamepad {} is player {}", g.gamepad(e.id).name(), self.players.len());
            }
        }
    }

    /// Whether a button went down since the last call.
    pub fn take_pressed(&mut self) -> bool {
        std::mem::take(&mut self.pressed)
    }

    /// Player `n`'s pad (0 the first), when one is connected: once a tick, as it takes the presses latched since.
    pub fn state(&mut self, n: usize) -> Option<PadState> {
        let id = *self.players.get(n)?;
        let mut actions = 0u32;
        if let Some(i) = self.latched.iter().position(|l| l.0 == id) {
            actions = self.latched.swap_remove(i).1;
        }
        let g = self.gilrs.as_ref()?;
        let pad = g.connected_gamepad(id)?;
        for (b, acts) in ACTIONS {
            if pad.is_pressed(b) {
                for &a in acts {
                    actions |= 1 << a;
                }
            }
        }
        for (b, a) in DPAD {
            if pad.is_pressed(b) {
                actions |= 1 << a;
            }
        }
        let v = |a: Axis| pad.value(a);
        Some(PadState {
            left: [v(Axis::LeftStickX), v(Axis::LeftStickY)],
            right: [v(Axis::RightStickX), v(Axis::RightStickY)],
            actions,
        })
    }

    /// Player `n`'s menu words held now (by [`PadWord`] index): the buttons, and the left stick leaning past
    /// [`MENU_PRESS`] (let go below [`MENU_RELEASE`]).
    pub fn menu_words(&mut self, n: usize) -> [bool; 16] {
        let mut out = [false; 16];
        let Some(g) = &self.gilrs else { return out };
        let Some(pad) = self.players.get(n).and_then(|&id| g.connected_gamepad(id)) else { return out };
        for (b, w) in MENU {
            if pad.is_pressed(b) {
                out[w as usize] = true;
            }
        }
        let (x, y) = (pad.value(Axis::LeftStickX), pad.value(Axis::LeftStickY));
        let held = &mut self.stick[n.min(1)];
        for (k, (lean, w)) in
            [(y, PadWord::Up), (-y, PadWord::Down), (-x, PadWord::Left), (x, PadWord::Right)].into_iter().enumerate()
        {
            held[k] = if held[k] { lean > MENU_RELEASE } else { lean > MENU_PRESS };
            out[w as usize] |= held[k];
        }
        out
    }
}

/// `qwc-game pads [--time SECS]`: each connected pad (its name, ids and whether `gilrs` knows its layout), then
/// every event for `secs` seconds (10 by default): what a pad that does nothing in the game sends.
pub fn diagnose(secs: f64) {
    let mut g = match Gilrs::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("gamepads: {e}");
            return;
        }
    };
    let describe = |g: &Gilrs, id: GamepadId| {
        let p = g.gamepad(id);
        format!(
            "{} (os name {:?}, vendor {:04x?}, product {:04x?}, layout {:?}, mapped {:?})",
            p.name(),
            p.os_name(),
            p.vendor_id(),
            p.product_id(),
            p.mapping_source(),
            p.map_name()
        )
    };
    let pads: Vec<GamepadId> = g.gamepads().map(|(id, _)| id).collect();
    println!("{} pad(s) at start", pads.len());
    for id in &pads {
        println!("  {id}: {}", describe(&g, *id));
    }
    println!("press buttons and move the sticks for {secs} s:");
    let end = std::time::Instant::now() + std::time::Duration::from_secs_f64(secs);
    while std::time::Instant::now() < end {
        while let Some(e) = g.next_event() {
            match e.event {
                EventType::Connected => println!("  {}: connected: {}", e.id, describe(&g, e.id)),
                EventType::AxisChanged(a, v, code) if v.abs() < 0.5 => {
                    let _ = (a, code);
                }
                ev => println!("  {}: {ev:?}", e.id),
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// A pad's stick as the gameplay pad's analogue axis word: centred on 32767, `up` the word's negative side.
pub fn axis_word(v: f32) -> u16 {
    (32767.0 + v.clamp(-1.0, 1.0) * 32767.0).round() as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stick_is_an_axis_word_centred_on_32767() {
        assert_eq!((axis_word(0.0), axis_word(1.0), axis_word(-1.0)), (32767, 65534, 0));
        assert_eq!(axis_word(2.0), 65534);
    }
}
