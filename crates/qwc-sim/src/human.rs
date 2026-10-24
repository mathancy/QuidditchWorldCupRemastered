//! The human player's controls (`docs/GAMEPLAY.md` "The human player"): the pad record the controllers object keeps
//! (`0x4031d0`), the input struct the human team's update fills from it (`0x442690`), and the states the human
//! brain runs (`0x446390`-`0x447480`), which [`crate::world::Match`] steps. **[T]** unless marked.
//!
//! The pad comes from the engine's gameplay pad variable (`[0x1043748]`), one word per key-binding action
//! (`qwc_feck::keys`): 0-11 make four stick axes (each an analog word with a digital half-axis each way), 12-22 the
//! buttons. [`Pad::update`] makes the record from those words, [`Input::from_pad`] the input struct.

use crate::vec3::Vec3;

/// The key-binding actions (the words of the gameplay pad variable).
pub mod action {
    /// The four axes, each `[analog, negative, positive]`: the move stick's vertical (up negative) and horizontal
    /// (left negative), then the second stick's (the two pass directions).
    pub const MOVE_V: [usize; 3] = [0, 1, 2];
    pub const MOVE_H: [usize; 3] = [3, 4, 5];
    pub const PASS_V: [usize; 3] = [6, 7, 8];
    pub const PASS_H: [usize; 3] = [9, 10, 11];
    /// "Action": pass with the ball, tackle without it.
    pub const ACTION: usize = 12;
    pub const DODGE: usize = 14;
    pub const BOOST: usize = 15;
    pub const SHOOT: usize = 16;
    /// Also "Special Move" (bound with Shoot on the PC, `0x10` and `0x11` together).
    pub const SPECIAL: usize = 17;
    pub const COMBO_LEFT: usize = 18;
    pub const COMBO_RIGHT: usize = 19;
    /// The pad's last button (`+0x20`): its press starts the uber move `0x43ed10` (Space or Return by default).
    pub const LAST: usize = 22;
    pub const COUNT: usize = 23;
}

/// The axis words' scales (`0x4031d0`): a half-axis `w / 65535`, an analog axis `(w - 32767) / 32767`.
const UNSIGNED: f64 = 1.5259022e-05;
const SIGNED: f64 = 3.051851e-05;

/// `0x403170`: an axis from its analog value and its two digital halves; the stronger one wins.
fn combine(analog: f64, negative: f64, positive: f64) -> f64 {
    let lo = analog.min(-negative);
    let hi = positive.max(analog);
    if hi <= -lo {
        lo
    } else {
        hi
    }
}

/// The stick dead zone (`0x4033e0`): under 0.2 nothing, past it the rest scaled back to 1.
fn dead_zone(v: f64) -> f64 {
    if v.abs() < 0.2 {
        0.0
    } else if v >= 0.0 {
        (v - 0.2) * 1.25
    } else {
        (v + 0.2) * 1.25
    }
}

/// One player's record (`0x105ff58 + 0x50 n`): the four axes (`+0x00`-`+0x0c`), and of the buttons (actions 12-22)
/// which are held and which went down this tick.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pad {
    pub axes: [f64; 4],
    pub held: [bool; 11],
    pub pressed: [bool; 11],
}

impl Pad {
    /// The record from this tick's action words (`0x4031d0`): a key's word is 0 or 65535, an analog axis's
    /// centred on 32767. Bit `k` of `present` says word `k` is bound to something on this machine (the pad
    /// variable's mask); an axis word that is not reads 0.
    pub fn update(&mut self, words: &[u16; action::COUNT], present: u32) {
        let w = |i: usize, scale: f64, offset: f64| {
            if present & 1 << i != 0 {
                (f64::from(words[i]) - offset) * scale
            } else {
                0.0
            }
        };
        let axis = |a: [usize; 3]| {
            dead_zone(combine(w(a[0], SIGNED, 32767.0), w(a[1], UNSIGNED, 0.0), w(a[2], UNSIGNED, 0.0)))
        };
        self.axes = [axis(action::MOVE_V), axis(action::MOVE_H), axis(action::PASS_V), axis(action::PASS_H)];
        for k in 0..11 {
            let down = words[12 + k] != 0;
            self.pressed[k] = down && !self.held[k];
            self.held[k] = down;
        }
    }

    pub fn held(&self, a: usize) -> bool {
        self.held[a - 12]
    }

    pub fn pressed(&self, a: usize) -> bool {
        self.pressed[a - 12]
    }
}

/// The input struct (`0x4337c0`, `0x43` bytes) a human team's update fills (`0x442690`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Input {
    /// `+0x00`: the move stick turned by the camera's heading; `+0x10` the second stick, turned alike; `+0x20` the
    /// move stick as it is (`(across, 0, up)`).
    pub stick: Vec3,
    pub stick2: Vec3,
    pub raw: Vec3,
    /// `+0x30`-`+0x42`.
    pub action_pressed: bool,
    pub action_held: bool,
    pub shoot_pressed: bool,
    pub shoot_held: bool,
    /// `+0x34`: Shoot pressed while Boost is held (the special move).
    pub special: bool,
    pub boost_pressed: bool,
    pub boost_held: bool,
    pub dodge_pressed: bool,
    pub special_pressed: bool,
    pub special_held: bool,
    pub combo_left_pressed: bool,
    pub combo_left_held: bool,
    pub combo_right_pressed: bool,
    pub combo_right_held: bool,
    /// Word 22 pressed: the uber move (`0x442690` calls `0x43ed10`).
    pub uber_pressed: bool,
}

impl Input {
    /// `0x442690`: the sticks turned by the camera's heading `(cx, cz)` (unit, across the ground: up on the stick
    /// flies along it), the buttons copied, the combo buttons cleared unless combos are allowed (DataStore
    /// `0x1527016b` bit 3).
    pub fn from_pad(pad: &Pad, camera: [f64; 2], combos: bool) -> Input {
        let (cx, cz) = (camera[0], camera[1]);
        let turn = |up: f64, across: f64| [cz * across - cx * up, 0.0, -cx * across - up * cz];
        let [v, h, v2, h2] = pad.axes;
        let mut i = Input {
            stick: turn(v, h),
            stick2: turn(v2, h2),
            raw: [h, 0.0, -v],
            action_pressed: pad.pressed(action::ACTION),
            action_held: pad.held(action::ACTION),
            shoot_pressed: pad.pressed(action::SHOOT),
            shoot_held: pad.held(action::SHOOT),
            special: false,
            boost_pressed: pad.pressed(action::BOOST),
            boost_held: pad.held(action::BOOST),
            dodge_pressed: pad.pressed(action::DODGE),
            special_pressed: pad.pressed(action::SPECIAL),
            special_held: pad.held(action::SPECIAL),
            combo_left_pressed: pad.pressed(action::COMBO_LEFT),
            combo_left_held: pad.held(action::COMBO_LEFT),
            combo_right_pressed: pad.pressed(action::COMBO_RIGHT),
            combo_right_held: pad.held(action::COMBO_RIGHT),
            uber_pressed: pad.pressed(action::LAST),
        };
        i.special = i.shoot_pressed && i.boost_held;
        if !combos {
            i.combo_left_pressed = false;
            i.combo_left_held = false;
            i.combo_right_pressed = false;
            i.combo_right_held = false;
        }
        i
    }

    /// The dodge the input asks for (`0x436750`): with Dodge pressed, kind 8 with both combo buttons held, 6 with
    /// the left, 7 with the right, else 9.
    pub fn dodge(&self) -> Option<u8> {
        if !self.dodge_pressed {
            return None;
        }
        Some(match (self.combo_left_held, self.combo_right_held) {
            (true, true) => 8,
            (true, false) => 6,
            (false, true) => 7,
            (false, false) => 9,
        })
    }
}

/// The wind-up limits of a human's throw (`0x668ef4`, `0x668ef8`): at least 6 ticks, at most 24.
pub const WIND_UP_MIN: i32 = 6;
pub const WIND_UP_MAX: i32 = 24;

/// The human throw (state 7, `0x446de0` and `0x446eb0`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Throw {
    /// `+0x18` it is a shot (Shoot held in the goal area when it began, `+0x30` as it began).
    pub shot: bool,
    pub began_as_shot: bool,
    /// `+0x10` the wind-up's ticks, `+0x14` those after the button went up, `+0x1c` the direction kept.
    pub count: i32,
    pub after: i32,
    pub dir: Vec3,
    /// `+0x19` the wind-up began (`0x43dbd0`), `+0x31` the pass aim found its receiver in the cone.
    pub started: bool,
    pub in_cone: bool,
    /// `+0x04`.
    pub done: bool,
    /// The carrier's `+0x184` as the combo buttons set it during the throw: 1 a combo pass, 6 a combo shot.
    pub combo: u8,
}

impl Throw {
    /// `0x446fd0`, each tick of the throw: with a combo button held (and the team's flag `+0x70` clear, **[I]**
    /// always), Shoot held with a chain running (`chain`, team `+0x5c`) and only one of the two makes a combo shot;
    /// else Action held or the second stick past 0.5 a combo pass. Nothing clears it.
    pub fn combo_buttons(&mut self, input: &Input, chain: i32) {
        let (left, right) = (input.combo_left_held, input.combo_right_held);
        if !(left || right) {
            return;
        }
        let l2 = (input.stick2[0].powi(2) + input.stick2[1].powi(2) + input.stick2[2].powi(2)).sqrt();
        if input.shoot_held && chain > 0 && !(left && right) {
            self.combo = 6;
        } else if input.action_held || l2 >= 0.5 {
            self.combo = 1;
        }
    }
}

/// What the throw does this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ThrowStep {
    /// Still winding up: aim the shot or the pass along `dir` at power `f`.
    AimShot {
        dir: Vec3,
        f: f64,
    },
    AimPass {
        dir: Vec3,
        f: f64,
    },
    /// Released: shoot (flat when wound to the end) or throw the pass (`a` its extra time, `volley`).
    Shoot {
        flat: bool,
    },
    Pass {
        a: f64,
        volley: bool,
    },
    /// Nothing yet (winding on to the least wind-up after an early release).
    Wait,
}

impl Throw {
    /// The tick of `0x446eb0` for input `i` (the shoot buttons already cleared unless it began as a shot, and the
    /// pass ones if it did), the carrier's forward `fwd`, and whether the stick points at a hoop the human may shoot
    /// at (`0x4419e0`).
    pub fn step(&mut self, i: &Input, fwd: Vec3, aims_at_hoop: bool) -> ThrowStep {
        let l2 = (i.stick2[0] * i.stick2[0] + i.stick2[1] * i.stick2[1] + i.stick2[2] * i.stick2[2]).sqrt();
        let d = if l2 >= 0.5 { i.stick2 } else { crate::vec3::unit(fwd) };
        let mut held = l2 >= 0.5 || i.action_held || i.shoot_held;
        let mut forced = false;
        let mut finished = true;
        if held {
            self.count += 1;
            self.after = 0;
            if self.count > WIND_UP_MAX {
                forced = true;
                held = false;
            }
            self.dir = d;
        } else if self.count < WIND_UP_MIN {
            self.count += 1;
            self.after += 1;
            finished = false;
        }
        let f = (self.count - WIND_UP_MIN) as f64 / (WIND_UP_MAX - WIND_UP_MIN) as f64;
        if held {
            self.started = true;
            if (i.shoot_held && self.shot) || (aims_at_hoop && !i.action_held) {
                self.shot = true;
                return ThrowStep::AimShot { dir: d, f };
            }
            self.shot = false;
            return ThrowStep::AimPass { dir: d, f };
        }
        if !finished {
            return ThrowStep::Wait;
        }
        self.done = true;
        if !self.started {
            return ThrowStep::Wait;
        }
        if self.shot {
            ThrowStep::Shoot { flat: forced }
        } else {
            ThrowStep::Pass { a: self.after as f64 / WIND_UP_MIN as f64, volley: forced && self.in_cone }
        }
    }
}

/// The human brain's state (the root state 1, `0x4463c0`, picks the branch each tick).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum State {
    /// State 3 (`0x446600`): the team has the Quaffle; the carrier is the active player. `armed` (`+0x10`): a throw
    /// may start (cleared by one, set again by the second stick).
    Attack { sub: AttackSub, armed: bool },
    /// State 4 (`0x446af0`): the other team has it.
    Defend { sub: DefendSub },
    /// State 2 (`0x446430`): no one has it.
    Loose,
}

/// The defence's sub-states (`0x446af0`, jump table `0x446d10`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefendSub {
    /// 9 (`0x4472b0`): the active player flies at the ball on the stick; Action tackles, Boost summons the Bludger.
    Chase,
    /// 12 (`0x447480`): the Bludger is being summoned.
    Summon,
    /// 11 (`0x4474f0`): the Bludger flies on the stick.
    Steer,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AttackSub {
    /// State 5: flying with the ball (`0x4362b0` on the input).
    Carry,
    /// State 7: the throw.
    Throw(Throw),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A keyboard's words: the keys in `on` down; every half-axis and button bound, no analog axes.
    fn words(on: &[usize]) -> [u16; action::COUNT] {
        let mut w = [0u16; action::COUNT];
        for &a in on {
            w[a] = 0xffff;
        }
        w
    }

    const KEYS: u32 = !(1 | 1 << 3 | 1 << 6 | 1 << 9);

    #[test]
    fn keys_make_full_sticks_and_edges() {
        let mut p = Pad::default();
        p.update(&words(&[action::MOVE_V[1], action::MOVE_H[2], action::ACTION]), KEYS);
        // up is negative on the record; a key's full deflection survives the dead zone
        assert!((p.axes[0] + 1.0).abs() < 1e-4 && (p.axes[1] - 1.0).abs() < 1e-4);
        assert!(p.pressed(action::ACTION) && p.held(action::ACTION));
        p.update(&words(&[action::ACTION]), KEYS);
        assert!(!p.pressed(action::ACTION) && p.held(action::ACTION));
        assert_eq!(p.axes, [0.0; 4]);
        assert_eq!(dead_zone(0.1), 0.0);
        assert!((dead_zone(-0.6) + 0.5).abs() < 1e-12);
    }

    #[test]
    fn the_stick_turns_with_the_camera() {
        let mut p = Pad::default();
        p.update(&words(&[action::MOVE_V[1]]), KEYS);
        // up flies along the camera's heading; right across it, to the heading's right
        let i = Input::from_pad(&p, [0.0, 1.0], false);
        assert!((i.stick[2] - 1.0).abs() < 1e-4 && i.stick[0].abs() < 1e-4);
        p.update(&words(&[action::MOVE_H[2]]), KEYS);
        let i = Input::from_pad(&p, [0.0, 1.0], false);
        assert!((i.stick[0] - 1.0).abs() < 1e-4);
        let i = Input::from_pad(&p, [-1.0, 0.0], false);
        assert!((i.stick[2] - 1.0).abs() < 1e-4, "{:?}", i.stick);
    }

    #[test]
    fn combos_and_dodges() {
        let mut p = Pad::default();
        p.update(&words(&[action::DODGE, action::COMBO_LEFT, action::SHOOT, action::BOOST]), KEYS);
        let i = Input::from_pad(&p, [0.0, 1.0], true);
        assert_eq!(i.dodge(), Some(6));
        assert!(i.special);
        assert_eq!(Input::from_pad(&p, [0.0, 1.0], false).dodge(), Some(9));
    }

    #[test]
    fn the_combo_buttons_make_a_combo_pass_or_a_combo_shot() {
        let mut t = Throw::default();
        t.combo_buttons(&Input { action_held: true, ..Input::default() }, 0);
        assert_eq!(t.combo, 0);
        t.combo_buttons(&Input { action_held: true, combo_left_held: true, ..Input::default() }, 0);
        assert_eq!(t.combo, 1);
        // Shoot with one combo button: a combo shot once a chain runs
        let mut s = Throw::default();
        let shoot = Input { shoot_held: true, combo_right_held: true, ..Input::default() };
        s.combo_buttons(&shoot, 0);
        assert_eq!(s.combo, 0);
        s.combo_buttons(&shoot, 2);
        assert_eq!(s.combo, 6);
        // both buttons: a combo pass when Action is held
        let both =
            Input { shoot_held: true, action_held: true, combo_left_held: true, combo_right_held: true, ..shoot };
        let mut b = Throw::default();
        b.combo_buttons(&both, 2);
        assert_eq!(b.combo, 1);
    }

    #[test]
    fn a_throw_winds_up_while_held_and_goes_on_release() {
        let mut t = Throw::default();
        let held = Input { action_held: true, ..Input::default() };
        for k in 1..=10 {
            assert!(matches!(t.step(&held, [1.0, 0.0, 0.0], false), ThrowStep::AimPass { .. }), "tick {k}");
        }
        let up = Input::default();
        t.in_cone = true;
        assert_eq!(t.step(&up, [1.0, 0.0, 0.0], false), ThrowStep::Pass { a: 0.0, volley: false });
        // released early: it winds on to the least, then goes, `a` counting the ticks after release
        let mut t = Throw::default();
        t.step(&held, [1.0, 0.0, 0.0], false);
        for _ in 0..5 {
            assert_eq!(t.step(&up, [1.0, 0.0, 0.0], false), ThrowStep::Wait);
        }
        assert_eq!(t.step(&up, [1.0, 0.0, 0.0], false), ThrowStep::Pass { a: 5.0 / 6.0, volley: false });
        // held to the end it goes at once, flat
        let mut t = Throw { shot: true, ..Throw::default() };
        let shoot = Input { shoot_held: true, ..Input::default() };
        for _ in 0..24 {
            assert!(matches!(t.step(&shoot, [1.0, 0.0, 0.0], false), ThrowStep::AimShot { .. }));
        }
        assert_eq!(t.step(&shoot, [1.0, 0.0, 0.0], false), ThrowStep::Shoot { flat: true });
    }
}
