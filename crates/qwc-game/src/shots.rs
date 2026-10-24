//! The match camera's shots over the follow camera (`crate::matchcam`): the kick-off's side view and the goal's fixed
//! camera. `crate::restart` has the fades around them.
//!
//! - **The kick-off** (the match's start, `0x4519d0`): the camera system is idle (mode 4, `0x47eb50`) until a catch
//!   asks for the follow camera without a cut (`0x431510` posts `0x1043b68`, handler `0x482a70`), so the view holds
//!   through the toss and eases into play when the kicker takes the Quaffle. **[I]** the held view is the remaster's
//!   own: side on to the pitch's centre, low, the chasers lined up facing each other, the camera team's on the left
//!   attacking right (the original's is the intro cutscene's end, the user's account of it), and the ease takes 1.5 s.
//! - **The goal** (`0x1043b64`, handler `0x482a90`): unless a special or uber move scored it, mode 1 (`0x47fbc0`)
//!   with a cut, the followed point held at the Quaffle as it scored: the eye at `(-86, 15.3, 13.2)`, or `(86, 15.3,
//!   -13.2)` at the other end, across to the point's side; looking at the point with a 55 degree field of view. The
//!   kick-off after (`0x451730` posts `0x1043a94` with `{0, cut}`) puts it back on the follow mode with a cut.
//!   **[I]** the eye's height is from the goal height (20 on the default pitch, where it is the traced 15.3), and
//!   the end is the point's for either team's camera.

use qwc_render::glam::Vec3;
use qwc_sim::world::Event;
use qwc_sim::Match;

use crate::matchcam::{v3, View};

/// The ease from the kick-off's view into play, in ticks.
pub const EASE: u32 = 36;

/// The kick-off view: how far to the side of the centre, how high over its floor, the point looked at over the
/// floor, and the field of view (**[I]**).
const SIDE: f32 = 20.0;
const SIDE_UP: f32 = 3.0;
const SIDE_AT: f32 = 2.0;
const SIDE_FOV: f32 = 55.0;

/// The goal camera (`0x47fbc0`: `0x5bf2d0`-`0x5bf2e0`, `0x5a5ca8`).
const GOAL_X: f32 = 86.0;
const GOAL_Y: f32 = 15.3;
const GOAL_Z: f32 = 13.2;
const GOAL_FOV: f32 = 55.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Shot {
    Follow,
    /// The toss: the side view.
    Toss,
    /// Easing from the side view into the follow camera, `ticks` in.
    Into {
        ticks: u32,
    },
    /// The goal camera on the point the Quaffle scored at.
    Goal {
        point: Vec3,
    },
}

/// Which shot the camera is on.
#[derive(Debug, Clone)]
pub struct Shots {
    shot: Shot,
    seen: usize,
}

impl Default for Shots {
    fn default() -> Shots {
        Shots { shot: Shot::Follow, seen: 0 }
    }
}

/// `t` eased in and out.
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Shots {
    /// After a tick of `m`: its events since the last call move the shot on; answers whether the follow cameras
    /// cut, as the original's does.
    pub fn tick(&mut self, m: &Match) -> bool {
        let mut cut = false;
        let start = self.seen.min(m.events.len());
        self.seen = m.events.len();
        for e in &m.events[start..] {
            match *e {
                Event::Goal { tick, .. } => {
                    let special = m.events[start..].iter().any(|e| {
                        matches!(*e, Event::SpecialMove { tick: t, .. } | Event::UberMove { tick: t, .. } if t == tick)
                    });
                    if !special {
                        self.shot = Shot::Goal { point: v3(m.quaffle) };
                    }
                }
                Event::KickOff { .. } => {
                    self.shot = Shot::Follow;
                    cut = true;
                }
                _ => {}
            }
        }
        self.shot = match self.shot {
            Shot::Follow if m.kickoff.is_some() && m.ball_team.is_none() => Shot::Toss,
            // the take: the follow camera settled on the kicker, eased into
            Shot::Toss if m.kickoff.is_none() => {
                cut = true;
                Shot::Into { ticks: 0 }
            }
            Shot::Into { ticks } if ticks + 1 >= EASE => Shot::Follow,
            Shot::Into { ticks } => Shot::Into { ticks: ticks + 1 },
            s => s,
        };
        cut
    }

    /// The kick-off's or the goal's view for the camera of `team`, or the follow camera's `follow`.
    pub fn view(&self, m: &Match, team: usize, follow: View) -> View {
        match self.shot {
            Shot::Follow => follow,
            Shot::Toss => side_view(m, team),
            Shot::Into { ticks } => {
                let from = side_view(m, team);
                let t = ease(ticks as f32 / EASE as f32);
                let dir = slerp(from.dir, follow.dir, t);
                View {
                    eye: from.eye.lerp(follow.eye, t),
                    dir,
                    up: from.up.lerp(follow.up, t),
                    fov: from.fov + (follow.fov - from.fov) * t,
                }
            }
            Shot::Goal { point } => goal_view(m, point),
        }
    }

    /// Whether the view is the follow camera's own (the sticks steer by it).
    pub fn following(&self) -> bool {
        self.shot == Shot::Follow
    }
}

/// `a` turned `t` of the way to `b`, both unit directions.
fn slerp(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    use qwc_render::glam::Quat;
    (Quat::IDENTITY.slerp(Quat::from_rotation_arc(a, b), t) * a).try_normalize().unwrap_or(b)
}

/// The kick-off's side view for `team`'s camera: its chasers on the left, attacking right.
fn side_view(m: &Match, team: usize) -> View {
    let floor = m.pitch.floor([0.0; 3]) as f32;
    // looking across the attack: team 0 attacks -x, so it looks along -z from +z (screen right is -x)
    let side = if m.teams[team].attack_dir() < 0.0 { 1.0 } else { -1.0 };
    let eye = Vec3::new(0.0, floor + SIDE_UP, side * SIDE);
    let at = Vec3::new(0.0, floor + SIDE_AT, 0.0);
    View { eye, dir: (at - eye).normalize(), up: Vec3::Y, fov: SIDE_FOV }
}

/// The goal camera (`0x47fbc0`) on `point`: at its end of the pitch, across on its side. **[I]** for either team's
/// camera; the original's end test (`point.x * hoops.x * ScaleGoals < 0`, `+0x1f8` the hoops the camera's team
/// attacks) gives this for team 0's and would put team 1's at the far end.
fn goal_view(m: &Match, point: Vec3) -> View {
    let (mut x, mut z) = (-GOAL_X, GOAL_Z);
    if point.x > 0.0 {
        (x, z) = (GOAL_X, -GOAL_Z);
    }
    if point.z * z < 0.0 {
        z = -z;
    }
    let y = GOAL_Y + (m.pitch.goal_height - 20.0) as f32;
    let eye = Vec3::new(x, y, z);
    View { eye, dir: (point - eye).try_normalize().unwrap_or(Vec3::X), up: Vec3::Y, fov: GOAL_FOV }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwc_sim::console::Console;
    use qwc_sim::rng::Rand;

    #[test]
    fn the_goal_camera_sits_at_the_scoring_end_on_the_points_side() {
        let m = Match::new(&Console::new(), Rand::new(7));
        // a goal at the -x end, on the +z side, is seen from (-86, 15.3, 13.2), looking on and up at it
        let v = goal_view(&m, Vec3::new(-97.0, 27.5, 5.0));
        assert_eq!(v.eye, Vec3::new(-86.0, 15.3, 13.2));
        assert!(v.dir.x < 0.0 && v.dir.y > 0.0, "{v:?}");
        // the other end, the -z side: (86, 15.3, -13.2)
        assert_eq!(goal_view(&m, Vec3::new(97.0, 27.5, -5.0)).eye, Vec3::new(86.0, 15.3, -13.2));
        // and across to the point's side
        assert_eq!(goal_view(&m, Vec3::new(97.0, 27.5, 5.0)).eye, Vec3::new(86.0, 15.3, 13.2));
    }

    #[test]
    fn the_kick_off_holds_the_side_view_and_eases_into_play_at_the_take() {
        let mut m = Match::new(&Console::new(), Rand::new(7));
        m.start_toss();
        let mut shots = Shots::default();
        let behind = View { eye: Vec3::new(10.0, 25.0, 0.0), dir: Vec3::NEG_X, up: Vec3::Y, fov: 70.0 };
        assert!(!shots.tick(&m));
        let side = shots.view(&m, 0, behind);
        // side on, looking across the pitch at its centre
        assert!(side.dir.z < -0.9 && side.eye.z > 0.0, "{side:?}");
        let mut cut = false;
        while m.kickoff.is_some() {
            m.step();
            cut = shots.tick(&m);
            assert!(!shots.following());
        }
        // the take cuts the follow camera onto the kicker, and the view eases into it
        assert!(cut);
        for _ in 0..EASE / 2 {
            assert!(!shots.tick(&m));
        }
        let mid = shots.view(&m, 0, behind);
        assert!(mid.dir.x < -0.3 && mid.dir.z < -0.3, "{mid:?}");
        for _ in 0..EASE {
            shots.tick(&m);
        }
        assert!(shots.following() && shots.view(&m, 0, behind) == behind);
    }
}
