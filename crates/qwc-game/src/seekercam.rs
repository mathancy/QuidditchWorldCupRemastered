//! The Seeker game's camera (`CameraSystem`'s mode `SeekerCam`, `0x47b4d0`; `RE_NOTES.md`, "The Seeker game's
//! camera"), one view per seeker, stepped each tick like the match camera.
//!
//! **[T]** Each step, from the seeker's velocity `V` and matrix (right `R`, up `U`, forward `F`, position `P`):
//! - a smoothed velocity `S += (V - S) / 4`; a weight `w` from `B = 0.9 B + min(a, 8)`: 1 below 10, 0 from 78,
//!   `1 - (B - 10) / 68` between; a speed sum `C = 0.8 (C + |V|)` giving a pull `z`: 1 up to 80, 2 past 180,
//!   `1 + (1 + sin(pi/2 (C - 80) / 100 - pi/4)) / 2` between;
//! - for 48 ticks after the seeker is tackled (`+0x1d0c`, `0x448d10`) a roll of `-sin(15 (1 - e)) e / 2`, else
//!   after it barges (`+0x1d08`, `0x44ae60`) `-sqrt(e) sin(15 (1 - e)) / 5`, with `e` the time left over 48, turning
//!   `R` and `U` about `F`;
//! - the look `Q = unit(F + 5 S)`, `g = (z - 1) / 2 + w / 8 + 1`, the human team's reach `k` eased between 100 and
//!   `BoostDestination` (350) at `BoostRate` (3) while boosting (`0x47be00`), as a factor `k / 100` pulled to 1 by
//!   `w`; the eye `P + U (2 - 0.7 z + 4 w) + Q (k / 100 - 5.25 g)`, looking along `unit(Q - 0.9 w U)` with `U` up;
//! - the field of view: team 0's eased from the view's own to `BoostFOV` (105) while boosting (`0x47bc70`), then
//!   pulled towards `70 - 45 w` by `w`.
//!
//! **[I]**: `a`, the camera system's `+0xb0` per view, has no writer a search of the exe finds (2026-10-21: no
//! direct, indexed or computed store to it; the camera's caller `0x47c74f` sets only the boost tunings). It was read
//! as the seeker's distance to the Snitch, which put the camera high and looking down whenever the seeker came close,
//! where the user's footage of the original keeps the chase view with the seeker's hand and the Snitch in frame. It
//! is now read as the seeker's speed: at racing speed `w` stays 0, and the camera rises and narrows only when he
//! slows (and for a second as the race view starts, while the sum builds). The roll's matrix is
//! `D3DXMatrixRotationZ`'s (`0x571dba` goes through the D3DX table); the seeker's up is the world's.

use qwc_render::glam::Vec3;

use crate::matchcam::View;

/// The ticks a shake lasts (`[0x6065e8]`).
const SHAKE: u32 = 48;

/// What a view's step reads of its seeker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inputs {
    pub pos: Vec3,
    pub vel: Vec3,
    /// The seeker's heading (its matrix's forward).
    pub fwd: Vec3,
    /// The view's `+0xb0` (**[I]** the seeker's speed; see the module's notes).
    pub reach: f32,
    pub boosting: bool,
    /// The team is the human's (`+0x8d` clear), which eases the reach while boosting.
    pub human: bool,
    /// The seeker is team 0's, whose view widens its field of view while boosting (`0x47b530`).
    pub team0: bool,
    /// Ticks since the seeker was last tackled and since it last barged.
    pub since_hit: Option<u32>,
    pub since_barge: Option<u32>,
    /// The match's tick (the shakes start only past 48).
    pub tick: u32,
}

/// One view's state between ticks.
#[derive(Debug, Clone, PartialEq)]
pub struct SeekerCam {
    /// `+0xc90`: the smoothed velocity.
    smooth: Vec3,
    /// `+0xb8`: the weight's sum; `+0xcb0`: the speed sum.
    b: f32,
    c: f32,
    /// `+0xcd8`: the reach (100 until a boost).
    reach: f32,
    /// The view's field of view and the one it had when the Seeker game began (`0x105f860`).
    fov: f32,
    base_fov: f32,
}

/// `0x47be00`: `cur` eased from `base` towards `dest` while `on` (else back towards `base`), a `rate`th of the way a
/// tick, but by at least a fiftieth of the span while still nearer where it starts (`0x47bea0`).
fn ease(cur: f32, base: f32, dest: f32, rate: f32, on: bool) -> f32 {
    let (from, to) = if on { (base, dest) } else { (dest, base) };
    if (from - cur).abs() <= (to - cur).abs() {
        let step = (cur - from).abs().max((from - to).abs() * 0.02);
        if to - cur > 0.0 {
            cur + step / rate
        } else {
            cur - step / rate
        }
    } else {
        cur + (to - cur) / rate
    }
}

impl SeekerCam {
    /// The camera as the Seeker game begins, from the match view's field of view.
    pub fn new(fov: f32) -> SeekerCam {
        SeekerCam { smooth: Vec3::ZERO, b: 0.0, c: 0.0, reach: 100.0, fov, base_fov: fov }
    }

    pub fn step(&mut self, i: &Inputs) -> View {
        self.smooth += (i.vel - self.smooth) * 0.25;
        self.b = self.b * 0.9 + i.reach.min(8.0);
        let w = if self.b >= 78.0 {
            0.0
        } else if self.b < 10.0 {
            1.0
        } else {
            1.0 - (self.b - 10.0) * (1.0 / 68.0)
        };
        self.c = (self.c + i.vel.length()) * 0.8;
        // the seeker's frame, rolled by a shake
        let fwd = i.fwd.try_normalize().unwrap_or(Vec3::Z);
        let mut up = Vec3::Y;
        let right = up.cross(fwd).try_normalize().unwrap_or(Vec3::X);
        up = fwd.cross(right);
        if i.tick > SHAKE {
            let shake = |since: Option<u32>| since.filter(|&s| s < SHAKE).map(|s| 1.0 - s as f32 / SHAKE as f32);
            let roll = if let Some(e) = shake(i.since_hit) {
                Some((15.0 * (1.0 - e)).sin() * e * 0.5)
            } else {
                shake(i.since_barge).map(|e| e.sqrt() * (15.0 * (1.0 - e)).sin() * 0.2)
            };
            if let Some(r) = roll {
                let (s, c) = (-r).sin_cos();
                // only the up is used after (the right turns with it, c R + s U)
                up = up * c - right * s;
            }
        }
        let q = (fwd + self.smooth * 5.0).try_normalize().unwrap_or(fwd);
        let z = if self.c > 180.0 {
            2.0
        } else if self.c > 80.0 {
            let x = (self.c - 80.0) / 100.0;
            ((x * std::f32::consts::FRAC_PI_2 - std::f32::consts::FRAC_PI_4).sin() + 1.0) * 0.5 + 1.0
        } else {
            1.0
        };
        let g = (z - 1.0) * 0.5 + w / 8.0 + 1.0;
        if i.human {
            self.reach = ease(self.reach, 100.0, 350.0, 3.0, i.boosting);
        }
        let mut k = self.reach * 0.01;
        if w > 1e-4 && k > 1.0001 {
            k -= (k - 1.0) * w;
        }
        let eye = i.pos + up * (2.0 - 0.7 * z + 4.0 * w) + q * (k - 5.25 * g);
        let dir = (q - up * (0.9 * w)).try_normalize().unwrap_or(q);
        if i.team0 {
            self.fov = ease(self.fov, self.base_fov, 105.0, 3.0, i.boosting);
        }
        let mut fov = self.fov;
        if w > 1e-4 {
            fov -= (fov - (70.0 - 45.0 * w)) * w;
        }
        View { eye, dir, up, fov }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(vel: Vec3) -> Inputs {
        Inputs {
            pos: Vec3::new(10.0, 20.0, 0.0),
            vel,
            fwd: Vec3::X,
            reach: 8.0,
            boosting: false,
            human: true,
            team0: true,
            since_hit: None,
            since_barge: None,
            tick: 100,
        }
    }

    #[test]
    fn it_sits_behind_and_above_the_seeker_and_pulls_back_with_speed() {
        let mut cam = SeekerCam::new(70.0);
        let still = (0..100).map(|_| cam.step(&inputs(Vec3::ZERO))).last().unwrap();
        // far from the Snitch, at rest: 4.25 behind along the heading, 1.3 up, at the view's field of view
        assert!((still.eye - Vec3::new(10.0 - 4.25, 21.3, 0.0)).length() < 1e-3, "{:?}", still.eye);
        assert!((still.dir - Vec3::X).length() < 1e-4 && (still.fov - 70.0).abs() < 1e-4);
        let mut cam = SeekerCam::new(70.0);
        let fast = (0..100).map(|_| cam.step(&inputs(Vec3::new(30.0, 0.0, 0.0)))).last().unwrap();
        // C settles at 4 |V| = 120, so z = 1 + (1 + sin(0.2 pi - pi/4)) / 2: farther back and lower
        let z = 1.0 + (1.0 + (0.2 * std::f32::consts::PI - std::f32::consts::FRAC_PI_4).sin()) * 0.5;
        let g = (z - 1.0) * 0.5 + 1.0;
        let want = Vec3::new(10.0 + 1.0 - 5.25 * g, 20.0 + 2.0 - 0.7 * z, 0.0);
        assert!((fast.eye - want).length() < 1e-3, "{:?} {want:?}", fast.eye);
    }

    #[test]
    fn near_the_snitch_it_closes_in_and_narrows() {
        let mut cam = SeekerCam::new(70.0);
        let mut i = inputs(Vec3::ZERO);
        i.reach = 0.5;
        let v = (0..100).map(|_| cam.step(&i)).last().unwrap();
        // w = 1: the field of view 25, raised by 4 and looking down
        assert!((v.fov - 25.0).abs() < 1e-3 && v.dir.y < -0.5 && v.eye.y > 24.0);
    }

    #[test]
    fn a_boost_widens_the_view_and_a_tackle_rolls_it() {
        let mut cam = SeekerCam::new(70.0);
        let mut i = inputs(Vec3::ZERO);
        i.boosting = true;
        let v = (0..60).map(|_| cam.step(&i)).last().unwrap();
        assert!(v.fov > 104.0 && (v.eye.x - (10.0 + 3.5 - 5.25)).abs() < 0.01);
        i.boosting = false;
        i.since_hit = Some(5);
        let v = cam.step(&i);
        assert!(v.up.z.abs() > 0.01, "{:?}", v.up);
        assert_eq!(ease(100.0, 100.0, 350.0, 3.0, true), 100.0 + 5.0 / 3.0);
    }
}
