//! Play restarting after a goal: the screen fading to black while the Quaffle goes through the hoop, the goal's
//! cutscenes in the black, then a fade back in on the kick-off, the keeper holding the Quaffle, with the HUD and its
//! sound. The camera's part is `crate::shots`'.
//!
//! **[T]** (`RE_NOTES.md`, "After a goal"):
//! - a goal with no cutscene fades to black over 0.75 s from the goal (`0x4322e0` posts `{0, 0.75, reset}` on the
//!   screen fade `0x1043bd0`, whose handler `0x49a770` darkens at 1/0.75 a second from clear);
//! - a goal's cutscenes play, then every goal queues an unnamed cutscene instance (type 2, `0x488600`);
//! - its end (`0x419be0`) posts the kick-off message `0x1043b50`: the HUD starts again (`0x4f7cc0`, its appearance
//!   over 24 ticks) with `HudStart` (`0x1043b20` kind 1, `osd.map` event 28); and the type-2 event restarts play
//!   (`0x451856`, `0x451730`: the placements, the keeper given the Quaffle, the camera back on the follow mode with
//!   a cut).
//!
//! **[I]**: the goal's cutscenes start once the screen is black (the original's start with a 0.1 s fade of their
//! own); the black holds until the simulation's kick-off; the fade back in takes 0.5 s, the HUD fading in with it,
//! and play waits for it. A special or uber move's goal is black at once (its cutscene plays from the goal).

use qwc_render::overlay::{OverlayQuad, OverlayVertex};
use qwc_sim::world::Event;
use qwc_sim::Match;

/// The fade to black after a goal, in ticks (0.75 s, `0x4323c1`).
pub const FADE_OUT: u32 = 18;
/// The fade back in on the kick-off, in ticks.
pub const FADE_IN: u32 = 12;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Stage {
    Play,
    /// Fading to black, `ticks` since the goal.
    Out {
        ticks: u32,
    },
    Black,
    /// Fading back in, `ticks` since it began.
    In {
        ticks: u32,
    },
}

/// What a tick of the restart asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// The fade back in began: the HUD's `HudStart`.
    HudStart,
}

/// The restart after a goal, stepped at the game's 24 Hz whether the match steps or holds.
#[derive(Debug, Clone)]
pub struct Restart {
    stage: Stage,
    /// The kick-off has happened since the goal.
    kicked: bool,
    /// The match's events already looked at.
    seen: usize,
}

impl Default for Restart {
    fn default() -> Restart {
        Restart { stage: Stage::Play, kicked: false, seen: 0 }
    }
}

impl Restart {
    /// One tick, with `m` as it stands and whether a cutscene plays or waits to.
    pub fn tick(&mut self, m: &Match, cutscene: bool) -> Option<Cue> {
        let start = self.seen.min(m.events.len());
        self.seen = m.events.len();
        let events = &m.events[start..];
        for e in events {
            match *e {
                Event::Goal { tick, .. } => {
                    // a special or uber move's cutscene shows its goal: black at once behind it
                    let special = events.iter().any(|e| {
                        matches!(*e, Event::SpecialMove { tick: t, .. } | Event::UberMove { tick: t, .. } if t == tick)
                    });
                    self.stage = if special { Stage::Black } else { Stage::Out { ticks: 0 } };
                    self.kicked = false;
                }
                Event::KickOff { .. } if self.stage != Stage::Play => self.kicked = true,
                _ => {}
            }
        }
        match self.stage {
            Stage::Play => None,
            Stage::Out { ticks } => {
                self.stage = if ticks + 1 >= FADE_OUT { Stage::Black } else { Stage::Out { ticks: ticks + 1 } };
                None
            }
            Stage::Black if self.kicked && !cutscene => {
                self.stage = Stage::In { ticks: 0 };
                Some(Cue::HudStart)
            }
            Stage::Black => None,
            Stage::In { ticks } => {
                self.stage = if ticks + 1 >= FADE_IN { Stage::Play } else { Stage::In { ticks: ticks + 1 } };
                None
            }
        }
    }

    /// How dark the screen is over the match (0 clear, 1 black).
    pub fn darkness(&self) -> f32 {
        match self.stage {
            Stage::Play => 0.0,
            Stage::Out { ticks } => (ticks as f32 / FADE_OUT as f32).min(1.0),
            Stage::Black => 1.0,
            Stage::In { ticks } => 1.0 - ticks as f32 / FADE_IN as f32,
        }
    }

    /// How much of the HUD shows: it fades with the screen.
    pub fn hud(&self) -> f32 {
        1.0 - self.darkness()
    }

    /// Whether a goal's cutscene must wait: the screen is still fading to black.
    pub fn cutscenes_wait(&self) -> bool {
        matches!(self.stage, Stage::Out { .. })
    }

    /// Whether the match holds: the kick-off is done and the screen is black or fading back in.
    pub fn holds(&self) -> bool {
        match self.stage {
            Stage::Black => self.kicked,
            Stage::In { .. } => true,
            _ => false,
        }
    }
}

/// The black over the match at `dark` (the overlay's white texel, texture 0; its alpha is not doubled), over the
/// whole window `fit` spans and under the HUD; none when clear.
pub fn black(fit: qwc_feck::draw::Fit, dark: f32) -> Option<OverlayQuad> {
    if dark <= 0.0 {
        return None;
    }
    let [l, t, r, b] = fit.span;
    let v = |x, y| OverlayVertex { pos: [x, y], uv: [0.0, 0.0], colour: [0.0, 0.0, 0.0, dark.min(1.0)] };
    Some(OverlayQuad { texture: 0, blend: 2, layer: 0, corners: [v(l, b), v(r, b), v(l, t), v(r, t)] })
}

/// The HUD's quads at `opacity` of their own.
pub fn fade(quads: &mut [OverlayQuad], opacity: f32) {
    if opacity >= 1.0 {
        return;
    }
    for q in quads {
        q.corners.iter_mut().for_each(|c| c.colour[3] *= opacity.max(0.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwc_sim::console::Console;
    use qwc_sim::rng::Rand;

    fn goal(m: &mut Match, special: bool) {
        let tick = m.tick;
        if special {
            m.events.push(Event::SpecialMove { tick, team: 0, player: 1, hoop: 4, kind: 0 });
        }
        m.events.push(Event::Goal { tick, team: 0, hoop: 4, combo: false });
    }

    #[test]
    fn a_goal_fades_out_waits_for_the_kick_off_and_fades_back_in() {
        let mut m = Match::new(&Console::new(), Rand::new(7));
        let mut r = Restart::default();
        assert_eq!((r.tick(&m, false), r.darkness()), (None, 0.0));
        goal(&mut m, false);
        r.tick(&m, false);
        assert!(r.cutscenes_wait() && !r.holds());
        for _ in 0..FADE_OUT {
            r.tick(&m, false);
        }
        // black, and play goes on to the kick-off
        assert_eq!((r.darkness(), r.cutscenes_wait(), r.holds()), (1.0, false, false));
        m.events.push(Event::KickOff { tick: m.tick, team: 1 });
        // a cutscene keeps it black
        assert_eq!(r.tick(&m, true), None);
        assert!(r.holds() && r.darkness() == 1.0);
        // then the fade back in with the HUD's sound, play held until it is over
        assert_eq!(r.tick(&m, false), Some(Cue::HudStart));
        assert!(r.holds() && r.darkness() == 1.0 && r.hud() == 0.0);
        for _ in 0..FADE_IN {
            assert_eq!(r.tick(&m, false), None);
        }
        assert_eq!((r.darkness(), r.holds()), (0.0, false));
    }

    #[test]
    fn a_special_moves_goal_is_black_at_once() {
        let mut m = Match::new(&Console::new(), Rand::new(7));
        let mut r = Restart::default();
        goal(&mut m, true);
        r.tick(&m, true);
        assert_eq!((r.darkness(), r.cutscenes_wait()), (1.0, false));
    }
}
