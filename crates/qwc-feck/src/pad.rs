//! The front end's controllers (`docs/FECK_VM.md` "Controllers"): how the pads' buttons become input, choose and
//! cancel events.
//!
//! - **[T]** Each game tick the controllers object (`0x403100`, `0x105ff58`) keeps per player which of 16 button
//!   words (the engine's pad table `[0x1043784]`, a short per button) are held and which were pressed this tick
//!   (`0x403d60`).
//! - **[T]** The front-end manager's update (`0x4e2950`) first steps two controllers (`0x4d7700`, pads 0 and 1),
//!   then pumps the events. A controller (`0x4d73d0`) reads ten newly-pressed flags as front-end buttons
//!   (`0x4d7750`, [`FE_BUTTONS`]) and compares them with the last tick's: a release posts an input event with flags
//!   1, a press one with flags 2, and button 1 (or else 2) also posts choose (or cancel) with the same flags.
//!   Handlers act on the release (flags 1): the shipped scripts test exactly that.
//! - **[T]** A held button repeats when the controller has repeats on (`+0x04` bit 0) for it (`+0x10`): after
//!   `+0x28` ticks, then every `+0x29`. Since a controller reads newly-pressed flags, a button is down for one tick
//!   only and the repeat never runs.
//! - **[T]** While `General_ToggleInputOnOff` has input off (`[0x5f1980]` not 1), a controller reads nothing and
//!   counts (`[0x104391c]`, shared); the second read turns input on again, so both controllers of one frame do.
//!
//! Not traced: how the PC's keyboard and joysticks fill the pad table (the `PadManager` maps keys to actions,
//! `qwc_feck::keys`; the table is an engine variable). [`PadWord`] names the words by what the front end does with
//! them; the host fills [`Pads::words`].

/// A pad word (`[0x1043784]`, by index) by what the front end does with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum PadWord {
    /// Moves a vertical menu back (front-end button `0x10`).
    Up = 0,
    /// Forward in a vertical menu (`0x20`).
    Down = 1,
    /// Back in a horizontal menu (`0x40`).
    Left = 2,
    /// Forward in a horizontal menu (`0x80`).
    Right = 3,
    /// Choose (`1`).
    Choose = 4,
    /// Front-end button `4` (the team select's Inventory).
    Button4 = 5,
    /// Front-end button `8`.
    Button8 = 6,
    /// Cancel (`2`).
    Cancel = 7,
    /// Front-end button `0x200`.
    Button200 = 8,
    /// Front-end button `0x100`.
    Button100 = 9,
}

/// The pad words a controller reads, in `0x4d7750`'s order, with the front-end button each gives.
pub const FE_BUTTONS: [(PadWord, u32); 10] = [
    (PadWord::Up, 0x10),
    (PadWord::Down, 0x20),
    (PadWord::Left, 0x40),
    (PadWord::Right, 0x80),
    (PadWord::Choose, 1),
    (PadWord::Cancel, 2),
    (PadWord::Button4, 4),
    (PadWord::Button8, 8),
    (PadWord::Button200, 0x200),
    (PadWord::Button100, 0x100),
];

/// The words `0x403d60` follows (10 and 11 are a stick's).
const FOLLOWED: [usize; 14] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 12, 13, 14, 15];

/// One player's buttons (`0x105ff58 +0xa0 + 0x24 × player`): held (`+0x00`-`+0x0d`), pressed this tick
/// (`+0x0e`-`+0x1b`), by pad word.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Buttons {
    pub held: [bool; 16],
    pub pressed: [bool; 16],
}

impl Buttons {
    /// `0x403d60`: a word not 0 is held; held now and not last tick is pressed.
    pub fn update(&mut self, words: &[u16; 16]) {
        for k in FOLLOWED {
            let now = words[k] != 0;
            self.pressed[k] = now && !self.held[k];
            self.held[k] = now;
        }
    }
}

/// A front-end controller (0x4c bytes at the front-end manager, two of them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Controller {
    /// `+0x2a`
    pub pad: u8,
    /// `+0x04` bit 0: repeats on.
    pub repeats: bool,
    /// `+0x10`: the buttons that repeat.
    pub repeat_mask: u32,
    /// `+0x28`, `+0x29`: ticks before the first repeat, and between repeats.
    pub delay: u8,
    pub interval: u8,
    /// `+0x14`: buttons to ignore once each (`0x4d7750` drops them from one read and clears them).
    pub eaten: u32,
    /// `+0x08`, `+0x0c`: this tick's buttons and the last tick's.
    pub buttons: u32,
    pub previous: u32,
    /// `+0x18`: per button, the ticks to its next repeat (0xff once released).
    pub counters: [u8; 16],
}

impl Controller {
    /// **[I]** the constructor is not traced: no repeats.
    pub fn new(pad: u8) -> Controller {
        Controller {
            pad,
            repeats: false,
            repeat_mask: 0,
            delay: 0,
            interval: 0,
            eaten: 0,
            buttons: 0,
            previous: 0,
            counters: [0; 16],
        }
    }
}

/// The pads as the front end sees them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pads {
    /// The engine's pad table (`[0x1043784]`), a word per [`PadWord`] and player: the host fills it before each
    /// frame (any value but 0 is down).
    pub words: [[u16; 16]; 2],
    pub buttons: [Buttons; 2],
    pub controllers: [Controller; 2],
    /// `[0x104391c]`: frames since input was turned off.
    pub off_frames: u32,
}

impl Default for Pads {
    fn default() -> Pads {
        Pads {
            words: [[0; 16]; 2],
            buttons: [Buttons::default(); 2],
            controllers: [Controller::new(0), Controller::new(1)],
            off_frames: 0,
        }
    }
}

/// What a controller's step posts, in order (`0x4d73d0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posted {
    /// Type 1: flags 1 a release, 2 a press.
    Input {
        pad: u8,
        flags: u32,
        buttons: u32,
    },
    /// Type 2 (button 1) or 3 (button 2), with the same flags.
    Choose {
        pad: u8,
        flags: u32,
    },
    Cancel {
        pad: u8,
        flags: u32,
    },
}

impl Pads {
    /// `0x4d7750`: the front-end buttons of `player`'s newly-pressed flags; nothing while input is off
    /// (`toggle` not 1), which the second such read turns on.
    fn read(&mut self, player: usize, toggle: &mut u8) -> u32 {
        let mut bits = 0;
        if *toggle == 1 {
            let pressed = &self.buttons[player].pressed;
            for (w, b) in FE_BUTTONS {
                if pressed[w as usize] {
                    bits |= b;
                }
            }
        } else {
            self.off_frames += 1;
            if self.off_frames == 2 {
                self.off_frames = 0;
                *toggle = 1;
            }
        }
        let c = &mut self.controllers[player];
        let eaten = c.eaten & bits;
        if eaten != 0 {
            bits ^= eaten;
            c.eaten ^= eaten;
        }
        bits
    }

    /// One tick: each player's buttons (`0x403d60`), then both controllers' steps (`0x4d7700` -> `0x4d73d0`),
    /// returning what they post. `toggle` is `General_ToggleInputOnOff`'s flag.
    pub fn step(&mut self, toggle: &mut u8) -> Vec<Posted> {
        let mut out = Vec::new();
        for p in 0..2 {
            let words = self.words[p];
            self.buttons[p].update(&words);
        }
        for p in 0..2 {
            let cur = self.read(p, toggle);
            let c = &mut self.controllers[p];
            c.previous = c.buttons;
            c.buttons = cur;
            let (prev, mut press, mut release) = (c.previous, 0u32, 0u32);
            for k in 0..16 {
                let b = 1u32 << k;
                // as written: once a press is found this tick, later changes count as unchanged
                if (prev ^ cur) & b == 0 || press != 0 {
                    if cur & b != 0 && c.repeats && c.repeat_mask & b != 0 {
                        if c.counters[k] != 0 {
                            c.counters[k] -= 1;
                        } else {
                            press |= b;
                            c.counters[k] = c.interval;
                        }
                    }
                } else if cur & b == 0 {
                    release |= b;
                    c.counters[k] = 0xff;
                } else {
                    c.counters[k] = c.delay;
                    press = b;
                }
            }
            if press == 0 && release == 0 {
                continue;
            }
            let pad = c.pad;
            if release != 0 {
                out.push(Posted::Input { pad, flags: 1, buttons: release });
            }
            if press != 0 {
                out.push(Posted::Input { pad, flags: 2, buttons: press });
            }
            if (press | release) & 1 != 0 {
                out.push(Posted::Choose { pad, flags: if press & 1 != 0 { 2 } else { 1 } });
            } else if (press | release) & 2 != 0 {
                out.push(Posted::Cancel { pad, flags: if press & 2 != 0 { 2 } else { 1 } });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_posts_its_press_then_its_release() {
        let (mut pads, mut on) = (Pads::default(), 1u8);
        pads.words[0][PadWord::Choose as usize] = 1;
        assert_eq!(
            pads.step(&mut on),
            [Posted::Input { pad: 0, flags: 2, buttons: 1 }, Posted::Choose { pad: 0, flags: 2 }]
        );
        // still held: the newly-pressed flag is gone, so the controller sees a release
        assert_eq!(
            pads.step(&mut on),
            [Posted::Input { pad: 0, flags: 1, buttons: 1 }, Posted::Choose { pad: 0, flags: 1 }]
        );
        assert_eq!(pads.step(&mut on), []);
        // cancel on the second pad, and an arrow
        pads.words[0][PadWord::Choose as usize] = 0;
        pads.words[1][PadWord::Cancel as usize] = 1;
        pads.words[0][PadWord::Down as usize] = 1;
        assert_eq!(
            pads.step(&mut on),
            [
                Posted::Input { pad: 0, flags: 2, buttons: 0x20 },
                Posted::Input { pad: 1, flags: 2, buttons: 2 },
                Posted::Cancel { pad: 1, flags: 2 }
            ]
        );
    }

    #[test]
    fn input_off_reads_nothing_and_comes_back() {
        let (mut pads, mut toggle) = (Pads::default(), 0u8);
        pads.words[0][PadWord::Up as usize] = 1;
        // both controllers read nothing; the second read turns input on
        assert_eq!((pads.step(&mut toggle), toggle), (vec![], 1));
        // the press was this tick's; next tick the word is still held, so nothing is new
        assert_eq!(pads.step(&mut toggle), []);
    }
}
