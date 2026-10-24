//! The front end's particles (traced 2026-10-19, `docs/FECK_VM.md` "Particles"): a `PTCL` object is an emitter
//! (0x118 bytes, built by `0x4d8f50`: `+0xbc` emitting, set to 1), its description (`LayerObj_Particle_SetDesc`, 19
//! floats at `+0xc0`) names a template object whose texture and material the particles are drawn with, and the
//! renderer runs a pool of 128 particles (`0x4e42c0`) fed by the emitters the frame drew (`0x4dc050`, at most 32).
//!
//! **[I]** The pool runs once per front-end tick here: its steps are 1/24 s, but the original runs it in the
//! renderer, whose rate against the ticks is not traced.

use crate::draw::{pack_colour, Spark, Tex, Vertex};
use crate::layer::Kind;
use crate::FrontEnd;

/// The pool's size (`0xfb7b84`, 0x28-byte records).
pub const POOL: usize = 128;
/// The step (`0x5bf048`).
const STEP: f32 = 0.041_666_668;
/// The random fraction's scale (`0x5befe0`, 1 / 32767).
const UNIT: f32 = 3.051_851e-5;

/// One particle (the pool's record).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    /// `+0x00`: the emitter (an object index).
    pub emitter: usize,
    /// `+0x04`: frames left.
    pub life: i32,
    /// `+0x08`: the frames left when the fade starts.
    pub fade: i32,
    /// `+0x0c`, `+0x10`
    pub pos: [f32; 2],
    /// `+0x14`, `+0x18`
    pub vel: [f32; 2],
    /// `+0x1c`, `+0x20`: the turn and its speed.
    pub rot: f32,
    pub spin: f32,
    /// `+0x24`
    pub size: f32,
}

/// The pool and the random state (`[0x5f1984]`, starting at 1).
#[derive(Debug, Clone)]
pub struct Particles {
    pub pool: [Option<Particle>; POOL],
    pub seed: u32,
}

impl Default for Particles {
    fn default() -> Self {
        Particles { pool: [None; POOL], seed: 1 }
    }
}

impl Particles {
    /// The game's random step: the 15-bit result is kept as the next state.
    fn next(&mut self) -> f32 {
        self.seed = (self.seed.wrapping_mul(0x343fd).wrapping_add(0x269ec3) >> 16) & 0x7fff;
        self.seed as f32 * UNIT
    }
}

/// Description field `k` (`+0xc0 + 4k`):
/// 0 the template's id; 1, 2 particles a second and its spread; 3, 4 the life in seconds and its spread; 5 the part of
/// the life spent fading; 6, 7, 8 the size, its spread and its growth a second; 9, 10 the speed and its spread; 11, 12
/// the spin and its spread; 13, 14 the direction, 15 its spread; 16 the drag a step; 17, 18 the acceleration.
fn desc(fe: &FrontEnd, e: usize, k: u16) -> f32 {
    f32::from_bits(fe.objects[e].slot(0xc0 + 4 * k))
}

/// A float truncated toward zero, as `_ftol` (`0x581d24`).
fn ftol(x: f32) -> i32 {
    x as i32
}

impl FrontEnd {
    /// Not the original's: the remaster's [`crate::draw::Adjust`]s on object `i` and its parents — whether one
    /// hides it, and their offsets added up (what the walk bakes into a sprite's position; the pool's particles
    /// take it at their quad, since their positions are the emitter's own).
    fn adjusted_above(&self, mut i: usize) -> (bool, [f32; 2]) {
        let mut off = [0.0f32; 2];
        loop {
            let o = &self.objects[i];
            if let Some(a) = self.adjust.get(&(self.layers[o.layer].hash, o.id)) {
                if a.hidden {
                    return (true, off);
                }
                off = [off[0] + a.offset[0], off[1] + a.offset[1]];
            }
            match o.parent {
                Some(p) => i = p,
                None => return (false, off),
            }
        }
    }

    /// `0x4dc050` (the particle object's draw), its first part: a new description finds its template by id among
    /// the layer's objects (`+0x10c`; here the object's index plus one, 0 for none).
    pub(crate) fn particle_template(&mut self, e: usize) {
        if self.objects[e].slot(0x110) & 0xff == 0 {
            return;
        }
        let id = self.objects[e].slot(0xc0);
        let l = self.objects[e].layer;
        let t = self.layers[l].objects.iter().copied().find(|&k| self.objects[k].id == id).map_or(0, |k| k as u32 + 1);
        let o = &mut self.objects[e];
        o.set_slot(0x10c, t);
        o.set_slot(0x110, 0);
    }

    /// `0x4e42c0`: one step of the pool with the frame's emitters; answers the particles' quads.
    /// - Each emitter that emits (`+0xbc`) gets `trunc((r × spread + rate) / 24)` particles to place (`+0x114`).
    /// - Each record in turn: a live one is stepped (`0x4e46b0`); an empty one takes a new particle from the first
    ///   emitter with any left (`0x4e4440`, stepped at once).
    pub fn particles_frame(&mut self, emitters: &[usize]) -> Vec<Spark> {
        // a destroyed emitter's particles go with it (`0x4dc0b0`, its `+0x0c`), and a hidden one's too (the
        // remaster's Adjust: the walk already keeps it from emitting, this takes what is still in the pool)
        let gone: Vec<usize> = (0..POOL)
            .filter(|&slot| {
                self.particles.pool[slot].is_some_and(|p| {
                    !self.layers[self.objects[p.emitter].layer].alive || self.adjusted_above(p.emitter).0
                })
            })
            .collect();
        for slot in gone {
            self.particles.pool[slot] = None;
        }
        for &e in emitters {
            let n = if self.objects[e].slot(0xbc) == 0 {
                0
            } else {
                let r = self.particles.next();
                ftol((r * desc(self, e, 2) + desc(self, e, 1)) * STEP).max(0)
            };
            self.objects[e].set_slot(0x114, n as u32);
        }
        let left = |fe: &FrontEnd, k: usize| fe.objects[emitters[k]].slot(0x114) as i32 != 0;
        let mut cur = (0..emitters.len()).find(|&k| left(self, k));
        let mut sparks = Vec::new();
        for slot in 0..POOL {
            if self.particles.pool[slot].is_some() {
                sparks.extend(self.particle_step(slot));
                continue;
            }
            let Some(k) = cur else { continue };
            let e = emitters[k];
            let n = self.objects[e].slot(0x114) as i32 - 1;
            self.objects[e].set_slot(0x114, n as u32);
            if n <= 0 {
                cur = (k + 1..emitters.len()).find(|&j| left(self, j));
            }
            sparks.extend(self.particle_spawn(e, slot));
        }
        sparks
    }

    /// `0x4e4440`: a new particle of emitter `e` in record `slot`, then its first step. The random draws, in order:
    /// the life, the direction's x and y, the speed, the turn, the spin, the size.
    fn particle_spawn(&mut self, e: usize, slot: usize) -> Option<Spark> {
        let dk: [f32; 19] = std::array::from_fn(|k| desc(self, e, k as u16));
        let d = |k: usize| dk[k];
        let p = &mut self.particles;
        let life = ftol(((p.next() - 0.5) * d(4) + d(3)) * 24.0);
        let fade = ftol((1.0 - d(5)) * life as f32);
        let o = &self.objects[e];
        let pos = [o.pos[0], o.pos[1]];
        let mut dir = [(p.next() * 2.0 - 1.0) * d(15) + d(13), 0.0];
        dir[1] = (p.next() * 2.0 - 1.0) * d(15) + d(14);
        let len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
        if len > 0.001 {
            dir = dir.map(|x| x * (1.0 / len));
        }
        let speed = (p.next() - 0.5) * d(10) + d(9);
        let vel = dir.map(|x| x * speed);
        let rot = (p.next() - 0.5) * std::f32::consts::TAU;
        let spin = (p.next() - 0.5) * d(12) + d(11);
        let size = (p.next() - 0.5) * d(7) + d(6);
        p.pool[slot] = Some(Particle { emitter: e, life, fade, pos, vel, rot, spin, size });
        self.particle_step(slot)
    }

    /// `0x4e46b0`: a step of record `slot` (one with no frames left is freed), then its quad (`0x4e4760`).
    fn particle_step(&mut self, slot: usize) -> Option<Spark> {
        let mut p = self.particles.pool[slot]?;
        let e = p.emitter;
        p.life -= 1;
        if p.life + 1 < 1 {
            self.particles.pool[slot] = None;
            return None;
        }
        let d = |k| desc(self, e, k);
        p.vel[0] += d(17) * STEP;
        p.vel[1] += d(18) * STEP;
        p.vel = p.vel.map(|v| (d(16) + 1.0) * v);
        p.pos[0] += p.vel[0] * STEP;
        p.pos[1] += p.vel[1] * STEP;
        p.rot += p.spin * STEP;
        p.size += d(8) * STEP;
        self.particles.pool[slot] = Some(p);
        self.particle_quad(&p)
    }

    /// `0x4e4760`: the unit square about the particle, scaled by its size and turned by minus its turn, in the
    /// emitter's colour with the alpha fading over the last frames, drawn with the template's texture and material.
    fn particle_quad(&self, p: &Particle) -> Option<Spark> {
        let o = &self.objects[p.emitter];
        let t = &self.objects[(o.slot(0x10c) as usize).checked_sub(1)?];
        let (tex, material) = match t.kind {
            Kind::Sprite => {
                (Tex { resource: t.slot(0xbc), frame: 0, script: t.script }, [0xc8u16, 0xc9, 0xca, 0xcb, 0xcc])
            }
            Kind::AnimatedSprite => (
                Tex { resource: t.slot(0xc0), frame: t.slot(0xc8) as u8, script: t.script },
                [0xed, 0xee, 0xef, 0xf0, 0xf1],
            ),
            _ => return None,
        };
        let alpha = if p.life < p.fade { p.life as f32 / p.fade as f32 } else { 1.0 };
        let argb = pack_colour([o.colour[0], o.colour[1], o.colour[2], alpha]);
        // the remaster's offsets above the emitter, which its spawn positions (the script's own) never took
        let at = self.adjusted_above(p.emitter).1;
        let (s, c) = (-p.rot).sin_cos();
        let corner = |x: f32, y: f32, uv: [i16; 2]| {
            let (x, y) = (x * p.size, y * p.size);
            Vertex { pos: [x * c - y * s + p.pos[0] + at[0], x * s + y * c + p.pos[1] + at[1], 0.0], uv, argb }
        };
        let corners = [
            corner(-0.5, 0.5, [0, 0x1000]),
            corner(0.5, 0.5, [0x1000, 0x1000]),
            corner(-0.5, -0.5, [0, 0]),
            corner(0.5, -0.5, [0x1000, 0]),
        ];
        Some(Spark {
            object: p.emitter,
            tex,
            material: material.map(|off| t.slot(off) as u8),
            order: t.layer_order,
            corners,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::{Layer, Object};

    /// A layer with a template sprite (id 5) and an emitter at (100, 50) making 48 particles a second (2 a
    /// frame) that live a second, fade over the second half, grow 24 a second from 10 and move right at 24.
    fn emitter() -> FrontEnd {
        let mut fe = FrontEnd::new();
        let mut t = Object::new(Kind::Sprite, 0);
        t.id = 5;
        t.set_slot(0xbc, 7);
        t.set_slot(0xc8, 2);
        t.layer_order = 60;
        let mut e = Object::new(Kind::Particles, 0);
        e.pos = [100.0, 50.0, 0.0, 0.0];
        e.colour = [0.5, 0.5, 0.5, 1.0];
        e.set_slot(0xbc, 1);
        let desc = [48.0f32, 0.0, 1.0, 0.0, 0.5, 10.0, 0.0, 24.0, 24.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        e.set_slot(0xc0, 5);
        for (k, v) in desc.iter().enumerate() {
            e.set_slot(0xc4 + 4 * k as u16, v.to_bits());
        }
        e.set_slot(0x110, 1);
        fe.objects.extend([t, e]);
        let (name, hash) = ("t.vso".into(), 0x1234);
        fe.layers.push(Layer {
            script: 0,
            name,
            hash,
            refs: 1,
            alive: true,
            screen: None,
            roots: vec![0, 1],
            objects: vec![0, 1],
            wipe: Default::default(),
        });
        fe
    }

    #[test]
    fn emitters_place_particles_that_move_fade_and_die() {
        let mut fe = emitter();
        fe.particle_template(1);
        assert_eq!((fe.objects[1].slot(0x10c), fe.objects[1].slot(0x110)), (1, 0));
        // two a frame, each stepped once as it is placed
        let s = fe.particles_frame(&[1]);
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].tex, s[0].material[0], s[0].order), (Tex { resource: 7, frame: 0, script: 0 }, 2, 60));
        let p = fe.particles.pool[0].unwrap();
        assert!((p.pos[0] - 101.0).abs() < 1e-4 && p.pos[1] == 50.0 && (p.size - 11.0).abs() < 1e-4);
        assert_eq!((p.life, p.fade), (23, 12));
        // the quad: the square of the size about the particle, fully opaque before the fade
        let xs: Vec<f32> = s[0].corners.iter().map(|v| v.pos[0] - p.pos[0]).collect();
        let spread = xs.iter().cloned().fold(f32::MIN, f32::max) - xs.iter().cloned().fold(f32::MAX, f32::min);
        assert!(spread <= 11.0 * std::f32::consts::SQRT_2 + 1e-3);
        assert_eq!(s[0].corners[0].argb >> 24, 0xff);
        // no more placed: the two live 24 frames in all, fading over the last 12
        fe.objects[1].set_slot(0xbc, 0);
        let mut last = Vec::new();
        for _ in 0..23 {
            last = fe.particles_frame(&[1]);
        }
        assert_eq!(last.len(), 2);
        assert!(last[0].corners[0].argb >> 24 < 0x20);
        assert!(fe.particles_frame(&[1]).is_empty());
        assert!(fe.particles.pool.iter().all(Option::is_none));
    }

    #[test]
    fn a_hidden_or_moved_emitter_takes_its_particles_with_it() {
        use crate::draw::Adjust;
        let mut fe = emitter();
        fe.particle_template(1);
        fe.particles_frame(&[1]);
        // the remaster's offset moves the quads; the pool's own positions stay the script's
        fe.adjust.insert((0x1234, 0), Adjust { offset: [40.0, -8.0], ..Default::default() });
        let s = fe.particles_frame(&[]);
        let p = fe.particles.pool[0].unwrap();
        let mid = s[0].corners.iter().map(|v| v.pos[0]).sum::<f32>() / 4.0;
        assert!((mid - p.pos[0] - 40.0).abs() < 1e-3, "{mid} vs {}", p.pos[0]);
        assert_eq!(s[0].object, 1);
        // hidden, through a parent: the pool's records go at once, not when their lives run out
        fe.objects[1].parent = Some(0);
        fe.adjust.insert((0x1234, 5), Adjust { hidden: true, ..Default::default() });
        assert!(fe.particles_frame(&[]).is_empty());
        assert!(fe.particles.pool.iter().all(Option::is_none));
    }

    #[test]
    fn a_dead_layer_takes_its_particles() {
        let mut fe = emitter();
        fe.particle_template(1);
        fe.particles_frame(&[1]);
        fe.layers[0].alive = false;
        assert!(fe.particles_frame(&[]).is_empty());
        assert!(fe.particles.pool.iter().all(Option::is_none));
    }

    #[test]
    fn the_random_step_keeps_fifteen_bits() {
        let mut p = Particles::default();
        // (1 * 0x343fd + 0x269ec3) >> 16 & 0x7fff
        p.next();
        assert_eq!(p.seed, 41);
        p.next();
        assert_eq!(p.seed, (41u32 * 0x343fd + 0x269ec3) >> 16 & 0x7fff);
    }
}
