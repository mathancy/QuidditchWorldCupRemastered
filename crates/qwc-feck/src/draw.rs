//! Drawing the front end (`docs/FECK_VM.md` "Drawing"): each frame a screen's layers are walked into a draw list
//! (`0x4e2f00` -> `0x4d9be0` -> `0x4d9a00`), objects calling their type's draw (`vtable +0x20`) with what they
//! inherit from their parents; the renderer (`0x4e4d90`) then turns the list into quads (`0x4e6270`), text runs
//! (`0x4e73e0`) and models. This module is the walk, the records and the sprite quads, independent of any GPU.
//!
//! Coordinates are the front end's 512 × 512 screen (the root region, `0x4d9be0`; the wipes sample at `x / 512`).

use crate::frontend::FrontEnd;
use crate::layer::{ftol, Kind};
use crate::layout::{Layout, Metrics};
pub use crate::wipe::WipeRecord;

/// A render region (12 bytes on a 16-deep stack, `0x4d9d90`): the root (kind 0, 512 × 512), or what an object
/// with flag `0x20000` (kind 1, a viewport, `0x8001` with `0x100000`), `0x40000` (kind 2) or `0x80000` (kind 4,
/// rendered to a texture of at most 256 × 256 with a camera, `0x4e76f0`) opens for itself and its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub kind: u16,
    /// Counts the regions opened in the screen's draw (`[0xfa3278]`).
    pub id: u16,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

const ROOT: Region = Region { kind: 0, id: 0, x: 0, y: 0, w: 0x200, h: 0x200 };

/// What an object passes its children (0x34 bytes, `0x4db680`): positions and rotations add, colours multiply;
/// the draw order and the size are the object's own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inherited {
    pub pos: [f32; 4],
    pub colour: [f32; 4],
    pub rot: [f32; 3],
    pub order: u32,
    pub size: [u16; 2],
}

impl Default for Inherited {
    /// `0x4db700`: nothing moved, white.
    fn default() -> Inherited {
        Inherited { pos: [0.0; 4], colour: [1.0; 4], rot: [0.0; 3], order: 0, size: [0; 2] }
    }
}

/// Not the original's: the remaster's change to where an object is drawn ([`FrontEnd::adjust`], by its layer's
/// hash and its id). Only the drawing changes: the scripts, the extents and the measured places still see the
/// object where the scripts put it, so their animations run as they did, moved. Its children move with it.
/// - `offset`: drawn moved by this, in the 512 x 512 screen's units (an emitter's particles with it).
/// - `hidden`: not drawn at all (an emitter's particles go at once, not when their lives run out).
/// - `scale`: drawn this many times its size, about where it stands.
/// - `pin`: per axis, kept at an edge of a window wider (or taller) than the original's 4:3, anchored to a point
///   of it, or stretched to both edges ([`FrontEnd::margins`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Adjust {
    pub offset: [f32; 2],
    pub hidden: bool,
    pub scale: f32,
    pub pin: [Pin; 2],
}

impl Default for Adjust {
    fn default() -> Adjust {
        Adjust { offset: [0.0; 2], hidden: false, scale: 1.0, pin: [Pin::None; 2] }
    }
}

/// Where an [`Adjust`] keeps an object on one axis of a window of another shape than the original's.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Pin {
    /// In the 4:3 middle, as the original places it.
    #[default]
    None,
    /// Moved with the left (or top) edge.
    Start,
    /// Moved with the right (or bottom) edge.
    End,
    /// Anchored to the point this far along the window, 0 the left (or top) edge to 1 the right (or bottom),
    /// keeping its offset from it: `At(0.0)` is `Start`, `At(0.5)` the middle and `At(1.0)` `End`.
    At(f32),
    /// Stretched from edge to edge (a full-screen background).
    Stretch,
}

/// Not the original's: where the 512 x 512 screen lies on the window. The original stretches it over its 4:3
/// window; the remaster keeps that shape, the screen centred and as tall (or wide) as the window, which reaches
/// past it on two sides by [`Fit::margins`], where the [`Pin`]ned objects go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// The window's left, top, right and bottom, in the screen's units.
    pub span: [f32; 4],
}

impl Fit {
    /// Stretched over the whole window, as the original draws it.
    pub const STRETCHED: Fit = Fit { span: [0.0, 0.0, 512.0, 512.0] };

    /// The screen at 4:3 in the middle of a window of `size` pixels.
    pub fn four_three(size: (u32, u32)) -> Fit {
        let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let aspect = w / h / (4.0 / 3.0);
        let (mx, my) = if aspect >= 1.0 { (256.0 * (aspect - 1.0), 0.0) } else { (0.0, 256.0 * (1.0 / aspect - 1.0)) };
        Fit { span: [-mx, -my, 512.0 + mx, 512.0 + my] }
    }

    /// How far the window reaches past the screen on each side, across and down, in its units.
    pub fn margins(&self) -> [f32; 2] {
        [-self.span[0], -self.span[1]]
    }

    /// The screen's own size in pixels on a window of `size`: what its text and shapes are measured for.
    pub fn area(&self, size: (u32, u32)) -> (u32, u32) {
        let k = |px: u32, a: f32, b: f32| ((px as f32 * 512.0 / (b - a)).round() as u32).max(1);
        (k(size.0, self.span[0], self.span[2]), k(size.1, self.span[1], self.span[3]))
    }

    /// A point of a window of `size` pixels in the screen's units.
    pub fn from_window(&self, p: [f32; 2], size: (f32, f32)) -> [f32; 2] {
        let [l, t, r, b] = self.span;
        [l + p[0] / size.0.max(1.0) * (r - l), t + p[1] / size.1.max(1.0) * (b - t)]
    }

    /// A point of the screen on a window of `size` pixels.
    pub fn to_window(&self, p: [f32; 2], size: (f32, f32)) -> [f32; 2] {
        let [l, t, r, b] = self.span;
        [(p[0] - l) / (r - l) * size.0, (p[1] - t) / (b - t) * size.1]
    }
}

/// Not the original's: where the remaster's [`Adjust`]s draw a point of the 512 x 512 screen,
/// `p * scale + add` on each axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Xf {
    pub scale: [f32; 2],
    pub add: [f32; 2],
}

impl Default for Xf {
    fn default() -> Xf {
        Xf::IDENTITY
    }
}

impl Xf {
    pub const IDENTITY: Xf = Xf { scale: [1.0; 2], add: [0.0; 2] };

    pub fn at(&self, p: [f32; 2]) -> [f32; 2] {
        [p[0] * self.scale[0] + self.add[0], p[1] * self.scale[1] + self.add[1]]
    }

    /// `self`, then `outer`.
    pub fn then(&self, outer: &Xf) -> Xf {
        Xf {
            scale: [self.scale[0] * outer.scale[0], self.scale[1] * outer.scale[1]],
            add: [self.add[0] * outer.scale[0] + outer.add[0], self.add[1] * outer.scale[1] + outer.add[1]],
        }
    }

    /// Scaling by `k` about `c`.
    fn about(c: [f32; 2], k: [f32; 2]) -> Xf {
        Xf { scale: k, add: [c[0] - c[0] * k[0], c[1] - c[1] * k[1]] }
    }

    /// The pins of `adjust` on a window `margins` wider on each side, in the window's terms (after every scale).
    fn pinned(adjust: &Adjust, margins: [f32; 2]) -> Xf {
        let mut xf = Xf::IDENTITY;
        for (k, (&m, pin)) in margins.iter().zip(adjust.pin).enumerate() {
            let step = match pin {
                Pin::None => continue,
                Pin::Start => Xf { scale: [1.0; 2], add: if k == 0 { [-m, 0.0] } else { [0.0, -m] } },
                Pin::End => Xf { scale: [1.0; 2], add: if k == 0 { [m, 0.0] } else { [0.0, m] } },
                // the window spans -m to 512 + m: the point at `t` of it is `m (2t - 1)` from where it was at 4:3
                Pin::At(t) => {
                    let d = m * (2.0 * t - 1.0);
                    Xf { scale: [1.0; 2], add: if k == 0 { [d, 0.0] } else { [0.0, d] } }
                }
                Pin::Stretch => {
                    let s = (512.0 + 2.0 * m) / 512.0;
                    Xf::about([256.0; 2], if k == 0 { [s, 1.0] } else { [1.0, s] })
                }
            };
            xf = xf.then(&step);
        }
        xf
    }

    pub fn is_identity(&self) -> bool {
        *self == Xf::IDENTITY
    }
}

/// Not the original's: the script hash the remaster's minted image resources belong to ([`FrontEnd::image_over`],
/// [`FrontEnd::additions`]). No layer script has it; the host's assets resolve its resources to image files.
pub const IMAGE_SCRIPT: u32 = 0x524d_5452;

/// A texture a sprite shows: the resource (`_ResourceData` id, of the layer script `script`) and, for an animated
/// sprite, the frame (the `frame`-th resource after it, `0x4dafa0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tex {
    pub resource: u32,
    pub frame: u8,
    /// The layer script's hash ([`crate::layer::Object::script`]): ids are each script's own.
    pub script: u32,
}

/// A sprite record (100 bytes, `0x4dab20`; filled by `0x4dd270` and `0x4db0f0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sprite {
    pub id: u32,
    pub object: usize,
    /// `+0x04`-`+0x10`
    pub pos: [f32; 4],
    /// `+0x14`-`+0x1c`: x and y (a direction with flag `0x800000`) and the turn about the pivot.
    pub rot: [f32; 3],
    /// `+0x24`-`+0x30`
    pub colour: [f32; 4],
    /// `+0x34`
    pub order: u32,
    pub tex: Tex,
    /// `+0x3c`, `+0x40`: the drawn width and height.
    pub size: [f32; 2],
    /// `+0x44`, `+0x48`: the turn's pivot from the top left.
    pub pivot: [f32; 2],
    /// `+0x4c`: the object's flags.
    pub flags: u32,
    /// `+0x50`, `+0x52`: the texture's scroll (1/4096).
    pub scroll: [u16; 2],
    /// `+0x54`
    pub region: Region,
    /// `+0x60`: the layer's hash (its wipe is found by it).
    pub layer: u32,
    /// The texture's material: its blend type (a sprite's `+0xc8`, an animated sprite's `+0xed`, given to
    /// `0x502f10`) and the bytes after it (`+0xc9`-`+0xcc` or `+0xee`-`+0xf1`: a sub-mode, `0x503040`'s value,
    /// two flags).
    pub material: [u8; 5],
}

/// A particle's quad (`0x4e4760`): drawn with its template's texture and material, in the template's material
/// layer. The corners are bottom left, bottom right, top left, top right, as a sprite's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spark {
    /// The emitter (an object index), which the remaster's [`Adjust`]s find it by.
    pub object: usize,
    pub tex: Tex,
    pub material: [u8; 5],
    pub order: u32,
    pub corners: [Vertex; 4],
}

/// A text record (0x5c bytes, `0x4dd5f0`; one per layout line, `0x4dd7a0`).
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    /// The object's id plus the line's place counted from the end.
    pub id: u32,
    pub object: usize,
    /// The object's position plus the line's offset.
    pub pos: [f32; 4],
    /// The line's colour with the inherited alpha, or the inherited colour.
    pub colour: [f32; 4],
    /// `+0x34`: the font (`+0xc0`).
    pub font: u32,
    pub order: u32,
    /// The line's characters (the text from `start`, `count` of them).
    pub start: u16,
    pub count: u16,
    pub flags: u32,
    /// `+0x48`: the effect flags (`+0xc8`; 8 draws the run twice, `0x4e73e0`).
    pub effect: u32,
    pub region: Region,
    pub layer: u32,
}

/// A model record (`0x4dbf20`, `0x4dbf90`): its own position and rotation, not its parents'.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Model {
    pub id: u32,
    pub object: usize,
    /// The model resource (`+0xbc`).
    pub resource: u32,
    pub pos: [f32; 4],
    pub rot: [f32; 3],
    /// `+0xc4`-`+0xd0`
    pub extra: [f32; 4],
    /// `+0xd4` (`Model_SetRotationMode`)
    pub rotation_mode: u8,
    pub order: u32,
    /// `+0xc0`
    pub field: u32,
    pub region: Region,
}

/// A puppet record (`0x4dc0e0`, `0x4dc7b0`): on a spline, the spline's position and rotation; otherwise its own
/// position, unrotated.
#[derive(Debug, Clone, PartialEq)]
pub struct Puppet {
    pub id: u32,
    pub object: usize,
    /// `+0x131`: the spline is relative to where it stands.
    pub relative: bool,
    pub pos: [f32; 4],
    /// A quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    pub region: Region,
    /// Not the original's: where its character's update put it ([`FrontEnd::puppet_root`]) and its character's
    /// pose, as the frame left them, so the frames drawn between ticks can blend them ([`DrawList::blend`]).
    pub stands: Option<crate::place::Matrix>,
    pub pose: Option<qwc_anim::pose::Pose>,
}

/// One frame's draw list (the list object: sprites `+0x54`, text `+0x1950`, models, puppets).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DrawList {
    pub sprites: Vec<Sprite>,
    pub texts: Vec<TextRun>,
    pub models: Vec<Model>,
    pub puppets: Vec<Puppet>,
    /// Particle objects (`0x4dc050`, at most 32).
    pub particles: Vec<usize>,
    /// The particles' quads (`FrontEnd::particles_frame`, `Manager::draw` runs it).
    pub sparks: Vec<Spark>,
    /// The camera object drawn last (`0x4dbdd0`, `[0xfa3344]`).
    pub camera: Option<usize>,
    /// Its view as the frame ended (`0x4fe530` posts it; `Manager::draw` takes it).
    pub view: Option<View>,
    /// The layers' wipes (`+0x5080`, the count `+0x5828`; `qwc_feck::wipe`).
    pub wipes: Vec<WipeRecord>,
    /// Not the original's: where the remaster's [`Adjust`]s draw each object's records, by object index (only
    /// those not drawn as they stand). The regions are already moved.
    pub xf: Vec<(usize, Xf)>,
    /// Each object walked, by index, at its place before any [`Adjust`] (where it is scaled about, less the
    /// offsets): what [`DrawList::xf_under`] works another set of adjustments out from.
    pub places: Vec<(usize, [f32; 2])>,
}

/// What the front end tells the game's camera each frame (`0x4fe530`, posted on `[0x1043a04]` while
/// `[0x10175ea]` is clear): the frame's camera object as the view the world is drawn with. The render camera
/// (`0x4bf070`, `0x4bed60`) keeps the last posts and blends them like the draw list; `0x49f720` makes the projection:
/// a vertical field of view of `fov_y` degrees at the viewport's aspect, `near` to `far`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// The object's position (`+0x08`; the camera spline puts its key's seventh float in w).
    pub eye: [f32; 4],
    /// `+0xd4`
    pub up: [f32; 4],
    /// Towards the interest object when there is one, else the forward vector `+0xc4` (`0x4dbe90`).
    pub dir: [f32; 4],
    /// `+0xe8`, and that times 0.75 (`[0x5bf164]`), which the projection uses.
    pub fov: f32,
    pub fov_y: f32,
    /// `+0xec`, `+0xf0`
    pub near: f32,
    pub far: f32,
    /// `+0xf4` (the focus object's distance, `0x4dbde0`), `+0xf8`: the depth of field.
    pub focus: [f32; 2],
}

impl FrontEnd {
    /// The view a camera object gives (`0x4fe530`).
    pub fn camera_view(&self, cam: usize) -> View {
        let o = &self.objects[cam];
        let f = |off: u16| f32::from_bits(o.slot(off));
        let v4 = |off: u16| [f(off), f(off + 4), f(off + 8), f(off + 12)];
        // `0x4dbe90`: the interest's position less the camera's, normalised (`0x4186d0`), w 0
        let interest = crate::layer::object_index(o.slot(0xbc)).and_then(|i| self.objects.get(i));
        let dir = match interest {
            Some(t) => {
                let d = [t.pos[0] - o.pos[0], t.pos[1] - o.pos[1], t.pos[2] - o.pos[2]];
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                let k = if len != 0.0 { 1.0 / len } else { 0.0 };
                [d[0] * k, d[1] * k, d[2] * k, 0.0]
            }
            None => v4(0xc4),
        };
        View {
            eye: o.pos,
            up: v4(0xd4),
            dir,
            fov: f(0xe8),
            fov_y: f(0xe8) * 0.75,
            near: f(0xec),
            far: f(0xf0),
            focus: [f(0xf4), f(0xf8)],
        }
    }
}

impl DrawList {
    /// Another screen's records after these (the screens fill one list in turn); its camera, when it has one,
    /// becomes the current one.
    pub fn append(&mut self, mut other: DrawList) {
        self.sprites.append(&mut other.sprites);
        self.texts.append(&mut other.texts);
        self.models.append(&mut other.models);
        self.puppets.append(&mut other.puppets);
        self.particles.append(&mut other.particles);
        self.particles.truncate(32);
        self.sparks.append(&mut other.sparks);
        self.camera = other.camera.or(self.camera);
        self.view = other.view.or(self.view);
        self.wipes.append(&mut other.wipes);
        self.xf.append(&mut other.xf);
        self.places.append(&mut other.places);
    }

    /// Where object `object`'s records are drawn ([`DrawList::xf`]).
    pub fn xf_of(&self, object: usize) -> Xf {
        self.xf.iter().find(|(o, _)| *o == object).map_or(Xf::IDENTITY, |(_, x)| *x)
    }

    /// Where object `i`'s records would be drawn with the adjustments `adjust` instead (on a window `margins`
    /// wider each side): the walk's rule over its parents' places as this list has them. `None` when one hides it.
    pub fn xf_under(
        &self,
        fe: &FrontEnd,
        adjust: &std::collections::HashMap<(u32, u32), Adjust>,
        margins: [f32; 2],
        i: usize,
    ) -> Option<Xf> {
        let mut chain = vec![i];
        while let Some(p) = fe.objects[*chain.last()?].parent {
            chain.push(p);
        }
        let mut m = Moved::default();
        for &k in chain.iter().rev() {
            let o = &fe.objects[k];
            let a = adjust.get(&(fe.layers[o.layer].hash, o.id)).copied().unwrap_or_default();
            if a.hidden {
                return None;
            }
            let place = self.places.iter().find(|(o, _)| *o == k).map_or([0.0; 2], |(_, p)| *p);
            let at = [place[0] + m.shift[0] + a.offset[0], place[1] + m.shift[1] + a.offset[1]];
            m = m.with(&a, at, margins);
        }
        Some(m.xf())
    }

    /// The wipe the renderer fades a sprite of layer `hash` with (the first of the layer's, `0x4e6563`).
    pub fn wipe(&self, hash: u32) -> Option<Wipe<'_>> {
        self.wipes.iter().find(|w| w.layer == hash).map(|w| Wipe { layer: w.layer, mask: &w.mask })
    }

    /// `0x4e8310`: the list drawn between two ticks, `t` of the way from `prev` to `self` (clamped to 0..1, as
    /// `0x4be5c0` does for it). Each record takes the last tick's record of the same id (the first) and blends
    /// with it; one without stays as it is.
    /// - Sprites: the position relative to the region, the rotation, the colour, the size and the pivot; the
    ///   rest is this tick's.
    /// - Text runs: the position relative to the region (unless the run's flags have `0x10`) and the colour.
    /// - **[I]** models, puppets and cards are not blended here (the original slerps their rotations); they are
    ///   not drawn yet.
    /// - The camera's view blends as the render camera does (`View::blend`).
    /// - Wipes: the mask halfway between the last tick's and this one's, whatever `t` (`WipeRecord::blend`).
    pub fn blend(&self, prev: &DrawList, t: f32) -> DrawList {
        let t = t.clamp(0.0, 1.0);
        let lerp = |a: f32, b: f32| (b - a) * t + a;
        let mut out = self.clone();
        for s in &mut out.sprites {
            let Some(p) = prev.sprites.iter().find(|p| p.id == s.id) else { continue };
            for k in 0..2 {
                let (pr, sr) = (region_origin(&p.region)[k], region_origin(&s.region)[k]);
                s.pos[k] = sr + lerp(p.pos[k] - pr, s.pos[k] - sr);
            }
            s.pos[2] = lerp(p.pos[2], s.pos[2]);
            s.rot = std::array::from_fn(|k| lerp(p.rot[k], s.rot[k]));
            s.colour = std::array::from_fn(|k| lerp(p.colour[k], s.colour[k]));
            s.size = std::array::from_fn(|k| lerp(p.size[k], s.size[k]));
            s.pivot = std::array::from_fn(|k| lerp(p.pivot[k], s.pivot[k]));
        }
        for r in &mut out.texts {
            let Some(p) = prev.texts.iter().find(|p| p.id == r.id) else { continue };
            if r.flags & 0x10 == 0 {
                for k in 0..2 {
                    let (pr, rr) = (region_origin(&p.region)[k], region_origin(&r.region)[k]);
                    r.pos[k] = rr + lerp(p.pos[k] - pr, r.pos[k] - rr);
                }
                r.pos[2] = lerp(p.pos[2], r.pos[2]);
            }
            r.colour = std::array::from_fn(|k| lerp(p.colour[k], r.colour[k]));
        }
        // the models' and puppets' records as `0x4e8310` blends them, their quaternions by `slerp` (**[I]** a model's
        // angles the shorter way round); not the original's, where the puppets' characters stand and their poses
        for m in out.models.iter_mut().filter(|_| t < 1.0) {
            let Some(p) = prev.models.iter().find(|p| p.id == m.id && p.object == m.object) else { continue };
            m.pos = std::array::from_fn(|k| lerp(p.pos[k], m.pos[k]));
            m.rot = std::array::from_fn(|k| lerp_angle(p.rot[k], m.rot[k], t));
            if m.rotation_mode != 0 && p.rotation_mode != 0 {
                m.extra = crate::spline::slerp(p.extra, m.extra, t);
            }
        }
        for r in out.puppets.iter_mut().filter(|_| t < 1.0) {
            let Some(p) = prev.puppets.iter().find(|p| p.id == r.id && p.object == r.object) else { continue };
            r.pos = std::array::from_fn(|k| lerp(p.pos[k], r.pos[k]));
            r.rotation = crate::spline::slerp(p.rotation, r.rotation, t);
            if let (Some(s), Some(ps)) = (r.stands.as_mut(), &p.stands) {
                let q = crate::spline::slerp(crate::spline::quaternion(ps), crate::spline::quaternion(s), t);
                let at: [f32; 3] = std::array::from_fn(|k| lerp(ps[12 + k], s[12 + k]));
                *s = crate::spline::matrix(q);
                s[12..15].copy_from_slice(&at);
            }
            if let (Some(pose), Some(pp)) = (r.pose.as_mut(), &p.pose) {
                *pose = pose.blend(pp, t);
            }
        }
        if let (Some(v), Some(p)) = (self.view, prev.view) {
            out.view = Some(v.blend(&p, t));
        }
        for w in &mut out.wipes {
            if let Some(p) = prev.wipes.iter().find(|p| p.layer == w.layer) {
                w.blend(p);
            }
        }
        out
    }
}

/// The angle `t` of the way from `a` to `b` (radians), the shorter way round.
fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    a + ((b - a + PI).rem_euclid(TAU) - PI) * t
}

/// A region's top left (the record's `+0x58`, `+0x5a`), which blended positions are relative to.
fn region_origin(r: &Region) -> [f32; 2] {
    [r.x as f32, r.y as f32]
}

impl View {
    /// `0x4bed60`: the render camera's view `t` of the way from `prev` to `self`: everything blended (the
    /// direction normalised after), the depth of field off when either end has its focus at 0.
    pub fn blend(&self, prev: &View, t: f32) -> View {
        let lerp = |a: f32, b: f32| (b - a) * t + a;
        let l4 = |a: [f32; 4], b: [f32; 4]| -> [f32; 4] { std::array::from_fn(|k| lerp(a[k], b[k])) };
        let mut dir = l4(prev.dir, self.dir);
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        if len != 0.0 {
            dir[..3].iter_mut().for_each(|x| *x /= len);
        }
        let focus = if prev.focus[0] == 0.0 || self.focus[0] == 0.0 {
            [0.0; 2]
        } else {
            [lerp(prev.focus[0], self.focus[0]), lerp(prev.focus[1], self.focus[1])]
        };
        View {
            eye: l4(prev.eye, self.eye),
            up: l4(prev.up, self.up),
            dir,
            fov: lerp(prev.fov, self.fov),
            fov_y: lerp(prev.fov_y, self.fov_y),
            near: lerp(prev.near, self.near),
            far: lerp(prev.far, self.far),
            focus,
        }
    }
}

/// What drawing needs from the host: texture sizes and fonts.
pub trait Assets {
    /// The texture's width and height; `None` when it is not loaded (the sprite then draws nothing).
    fn texture(&self, tex: Tex) -> Option<[u16; 2]>;
    /// The font's metrics, for the text layout.
    fn font(&self, font: u32) -> Option<&dyn Metrics>;
    /// Run the resource records' choice functions again (`run(script, function)`), as a screen loads its layers
    /// (`0x4ea420`); without an index of them nothing happens.
    fn refresh_choices(&self, _run: &mut dyn FnMut(usize, u32) -> Option<u32>) {}
}

/// No textures and no fonts (a front end without a host's assets).
pub struct NoAssets;

impl Assets for NoAssets {
    fn texture(&self, _: Tex) -> Option<[u16; 2]> {
        None
    }
    fn font(&self, _: u32) -> Option<&dyn Metrics> {
        None
    }
}

/// The port's notes on a sprite's texture (slots past the object's end): the frame of its resource it shows, which
/// `LayerObj_Sprite_AddAction_Animate` moves (the original binds the frame's texture, `0x4d5320`), and the size of
/// the texture it bound (the original's `+0xc4` texture holds it).
const SPRITE_FRAME: u16 = 0xff0c;
const BOUND_SIZE: u16 = 0xff08;

/// The texture a sprite shows: its resource (`+0xbc`), at the frame an animate action left it on.
pub fn sprite_tex(o: &crate::layer::Object) -> Tex {
    Tex { resource: o.slot(0xbc), frame: o.slot(SPRITE_FRAME) as u8, script: o.script }
}

impl FrontEnd {
    /// The texture sprite `i` draws: the remaster's replacement picture when one is set ([`FrontEnd::image_over`]),
    /// else its own ([`sprite_tex`], an animated sprite's [`aspt_tex`]). The frame stays the live one, so a
    /// replaced animation still turns its pages (the host resolves a replacement's frames by its file convention).
    pub fn shown_tex(&self, i: usize) -> Tex {
        let o = &self.objects[i];
        let own = match o.kind {
            crate::layer::Kind::AnimatedSprite => aspt_tex(o),
            _ => sprite_tex(o),
        };
        match self.image_over.get(&(self.layers[o.layer].hash, o.id)) {
            Some(&r) => Tex { resource: r, frame: own.frame, script: IMAGE_SCRIPT },
            None => own,
        }
    }
}

/// Show `frame` of the sprite's resource (the animate action; 0 for a new resource).
pub fn set_sprite_frame(o: &mut crate::layer::Object, frame: u8) {
    o.set_slot(SPRITE_FRAME, frame as u32);
}

/// The frame of its resource a sprite has bound, if it has drawn.
pub fn bound_frame(o: &crate::layer::Object) -> Option<u8> {
    (o.slot(0xff00) != 0).then(|| (o.slot(0xff00) >> 23) as u8)
}

/// What the remaster's [`Adjust`]s above an object do to it: their offsets added up (the regions' measured places
/// take them), their scales, their pins (in the window's terms, after the scales), and the axes a pin holds.
#[derive(Clone, Copy, Default)]
struct Moved {
    shift: [f32; 2],
    scale: Xf,
    pin: Xf,
    pinned: [bool; 2],
}

impl Moved {
    /// With the object's own `adjust`, the object at `at` (its place with the offsets): scaled about there, and
    /// pinned on an axis no object above pins.
    fn with(&self, adjust: &Adjust, at: [f32; 2], margins: [f32; 2]) -> Moved {
        let mut a = *adjust;
        for (pin, pinned) in a.pin.iter_mut().zip(self.pinned) {
            if pinned {
                *pin = Pin::None;
            }
        }
        Moved {
            shift: [self.shift[0] + a.offset[0], self.shift[1] + a.offset[1]],
            scale: Xf::about(at, [a.scale; 2]).then(&self.scale),
            pin: self.pin.then(&Xf::pinned(&a, margins)),
            pinned: [0, 1].map(|k| self.pinned[k] || a.pin[k] != Pin::None),
        }
    }

    /// Where the records are drawn.
    fn xf(&self) -> Xf {
        self.scale.then(&self.pin)
    }
}

struct Walk<'a> {
    list: DrawList,
    regions: Vec<Region>,
    counter: u16,
    assets: &'a dyn Assets,
}

impl FrontEnd {
    /// `0x4e2f00`: a screen's draw. Its layers (`layers`, the screen's slots in order) each draw once a frame
    /// (`layer +0x50`, cleared by `begin_frame`); the region counter starts again.
    pub fn draw_screen(&mut self, layers: &[usize], drawn: &mut Vec<usize>, assets: &dyn Assets) -> DrawList {
        let mut w = Walk { list: DrawList::default(), regions: vec![ROOT], counter: 0, assets };
        for &l in layers {
            if drawn.contains(&l) || !self.layers.get(l).is_some_and(|x| x.alive) {
                continue;
            }
            drawn.push(l);
            // `0x4d9be0`: the first top-level object's places and extents from the screen's top left (`0x4d9c9a`;
            // the others are not measured), the root region, the walk from the first top-level object along its
            // siblings, then the layer's wipe (`0x4d9d0d`)
            let roots = self.layers[l].roots.clone();
            if let Some(&r) = roots.first() {
                self.measure(r, [0.0; 4], assets);
            }
            w.regions.truncate(1);
            for r in roots {
                self.walk(r, Inherited::default(), Moved::default(), &mut w);
            }
            self.step_wipe(l, &mut w.list);
        }
        w.list
    }

    /// `0x4da340`: object `i`'s place on the screen and its extent, from its parent's place `origin`, storing both
    /// for it and every object below it (`+0x3c`-`+0x48`, `+0x4c`-`+0x58`; the regions and the wipe watchers read
    /// them). The place is the parent's plus the object's position, moved by the type's `+0x38` (its offset, as
    /// drawn); the children are measured from it. The extent is the type's own (`+4`, [`FrontEnd::own_extent`]),
    /// grown to every child's far side, then moved by `+0x38` too. Answers the extent.
    pub fn measure(&mut self, i: usize, origin: [f32; 4], assets: &dyn Assets) -> [f32; 4] {
        let mut out = Vec::new();
        let (_, extent) = self.extent_walk(i, origin, assets, &mut out);
        for (k, pos, ext) in out {
            let o = &mut self.objects[k];
            for j in 0..4 {
                o.set_slot(0x3c + 4 * j as u16, pos[j].to_bits());
                o.set_slot(0x4c + 4 * j as u16, ext[j].to_bits());
            }
        }
        extent
    }

    /// [`FrontEnd::measure`] without storing: each object's (index, place, extent) into `out`.
    pub fn extent_walk(
        &self,
        i: usize,
        origin: [f32; 4],
        assets: &dyn Assets,
        out: &mut Vec<(usize, [f32; 4], [f32; 4])>,
    ) -> ([f32; 4], [f32; 4]) {
        let o = &self.objects[i];
        let pos = [origin[0] + o.pos[0], origin[1] + o.pos[1], origin[2] + o.pos[2], 0.0];
        let mut place = Inherited { pos, ..Default::default() };
        self.own_offset(i, &mut place, assets);
        let pos = place.pos;
        let mut ext = self.own_extent(i, assets);
        for &c in &o.children {
            let (cp, ce) = self.extent_walk(c, pos, assets, out);
            for k in 0..3 {
                let far = cp[k] + ce[k] - pos[k];
                if far > ext[k] {
                    ext[k] = far;
                }
            }
        }
        let mut moved = Inherited { pos: ext, ..Default::default() };
        self.own_offset(i, &mut moved, assets);
        out.push((i, pos, moved.pos));
        (pos, moved.pos)
    }

    /// The type's own extent (`+4`): the `SIZE` (`0x4daae0`, every type without its own); a sprite's bound
    /// texture (`0x4dd540`; nothing before it draws); an animated sprite's frame times its scales (`0x4db3a0`); a
    /// text's laid-out width and height (`+0xd4`, `+0xd6`, `0x4de660`). With flag `0x10000` each takes its `SIZE`
    /// (times an animated sprite's scales).
    pub fn own_extent(&self, i: usize, assets: &dyn Assets) -> [f32; 4] {
        let o = &self.objects[i];
        let sized = [o.size[0] as f32, o.size[1] as f32];
        let sized_flag = o.flags & 0x10000 != 0;
        let [w, h] = match o.kind {
            Kind::Sprite if !sized_flag => match bound_frame(o) {
                Some(_) => {
                    let s = o.slot(BOUND_SIZE);
                    [(s & 0xffff) as f32, (s >> 16) as f32]
                }
                None => [0.0; 2],
            },
            Kind::AnimatedSprite => {
                let (sx, sy) = (f32::from_bits(o.slot(0x108)), f32::from_bits(o.slot(0x10c)));
                let size = if sized_flag {
                    Some(sized)
                } else {
                    assets.texture(self.shown_tex(i)).map(|[tw, th]| [tw as f32, th as f32])
                };
                size.map_or([0.0; 2], |[w, h]| [w * sx, h * sy])
            }
            Kind::Text if !sized_flag => [o.slot(0xd4) as u16 as f32, o.slot(0xd6) as u16 as f32],
            _ => sized,
        };
        [w, h, 0.0, 0.0]
    }

    /// `0x4ea830` for layer `l`: its wipe's step, the record into `list`, the watchers' events queued, and the
    /// callback when it ends. **[I]** The original runs the callback there, inside the draw, with the layer's first
    /// top-level object (`layer +0x60`); the port draws after the frame and runs it at the start of the next
    /// ([`FrontEnd::run_wipe_callbacks`]).
    fn step_wipe(&mut self, l: usize, list: &mut DrawList) {
        let hash = self.layers[l].hash;
        let s = self.layers[l].wipe.step(hash, &self.objects);
        list.wipes.extend(s.record);
        self.queue.extend(s.events);
        if let Some(callback) = s.ended {
            let root = self.layers[l].roots.first().map_or(0, |&r| crate::layer::object_handle(r));
            self.wipe_callbacks.push((self.layers[l].script, callback, root));
        }
    }

    /// `0x4d9a00` for one object: its region, what it passes on, its draw (unless flags `0xc`), its children.
    /// **[I]** the original also runs each object's per-frame process here (`0x4da880`); the port runs it in the
    /// screen manager's frame. `moved` is what the remaster's [`Adjust`]s above it do to it.
    fn walk(&mut self, i: usize, parent: Inherited, moved: Moved, w: &mut Walk) {
        let o = &self.objects[i];
        let adjust = self.adjust.get(&(self.layers[o.layer].hash, o.id)).copied().unwrap_or_default();
        if adjust.hidden {
            return;
        }
        let at = [0, 1].map(|k| parent.pos[k] + o.pos[k] + adjust.offset[k]);
        let moved = moved.with(&adjust, at, self.margins);
        let (shift, xf) = (moved.shift, moved.xf());
        if !xf.is_identity() {
            w.list.xf.push((i, xf));
        }
        w.list.places.push((i, [at[0] - shift[0], at[1] - shift[1]]));
        let f = o.flags;
        let kind = if f & 0x20000 != 0 {
            Some(if f & 0x100000 != 0 { 0x8001 } else { 1 })
        } else if f & 0x40000 != 0 {
            Some(2)
        } else if f & 0x80000 != 0 {
            Some(4)
        } else {
            None
        };
        if let Some(kind) = kind {
            // the object's place and extent as the layer's measuring left them (`FrontEnd::measure`); **[I]** 0 for
            // an object below a top-level object other than the first, which nothing measures
            let fl = |off: u16| ftol(f32::from_bits(o.slot(off))) as u16;
            let (rw, rh) = if f & 0x10000 != 0 { (o.size[0], o.size[1]) } else { (fl(0x4c), fl(0x50)) };
            w.counter = w.counter.wrapping_add(1);
            if w.regions.len() < 17 {
                let at = [0x3cu16, 0x40].map(|off| f32::from_bits(o.slot(off)));
                let [x, y] = xf.at([at[0] + shift[0], at[1] + shift[1]]);
                let (rw, rh) = (rw as f32 * xf.scale[0], rh as f32 * xf.scale[1]);
                let (x, y, rw, rh) = (ftol(x) as u16, ftol(y) as u16, ftol(rw) as u16, ftol(rh) as u16);
                w.regions.push(Region { kind, id: w.counter, x, y, w: rw, h: rh });
            }
        }
        let mut here = Inherited {
            pos: std::array::from_fn(|k| parent.pos[k] + o.pos[k] + adjust.offset.get(k).copied().unwrap_or(0.0)),
            colour: std::array::from_fn(|k| parent.colour[k] * o.colour[k]),
            rot: std::array::from_fn(|k| parent.rot[k] + o.rot[k]),
            order: o.layer_order,
            size: o.size,
        };
        self.own_offset(i, &mut here, w.assets);
        if self.objects[i].flags & 0xc == 0 {
            self.draw_object(i, &here, w);
        }
        let mut below = here;
        if self.objects[i].kind == Kind::PageMenu {
            // `0x4dc880`: a page menu's scroll moves its children
            let o = &self.objects[i];
            for (k, off) in [0x100u16, 0x104, 0x108].into_iter().enumerate() {
                below.pos[k] -= f32::from_bits(o.slot(off));
            }
        }
        for c in self.objects[i].children.clone() {
            self.walk(c, below, moved, w);
        }
        if kind.is_some() && w.regions.len() > 1 {
            w.regions.pop();
        }
    }

    /// The type's `+0x38`: a sprite's offset (`0x4dd4f0`, with `+0xd8 & 3`); an animated sprite's offset, and
    /// origin with `+0xfc & 3` (`0x4db4a0`, both by its scales). Not the original's: a sprite its draw will centre
    /// this frame (`+0xd8 & 2`, the texture not yet bound) takes the centring already. The original runs `+0x38`
    /// before its draw binds the texture and sets the offset, so the first frame (and its children's) is half the
    /// texture off, which the remaster's blending of frames would show as a slide.
    fn own_offset(&self, i: usize, a: &mut Inherited, assets: &dyn Assets) {
        let o = &self.objects[i];
        match o.kind {
            Kind::Sprite if o.slot(0xd8) & 3 != 0 => {
                let tex = self.shown_tex(i);
                let mut off = [o.slot(0xd2), o.slot(0xd4)].map(|v| v as u16 as i16 as f32);
                if o.slot(0xd8) & 2 != 0 && !bound(o, tex) {
                    if let Some([tw, th]) = assets.texture(tex) {
                        off = [tw, th].map(|t| (t >> 1).wrapping_neg() as i16 as f32);
                    }
                }
                a.pos[0] += off[0];
                a.pos[1] += off[1];
            }
            Kind::AnimatedSprite => {
                let (sx, sy) = (f32::from_bits(o.slot(0x108)), f32::from_bits(o.slot(0x10c)));
                let mut x = o.slot(0x100) as i32 as f32 * sx;
                let mut y = o.slot(0x104) as i32 as f32 * sy;
                if o.slot(0xfc) & 3 != 0 {
                    let (mut ox, mut oy) = (o.slot(0xf6) as u16 as i16 as f32, o.slot(0xf8) as u16 as i16 as f32);
                    if o.flags & 0x10000 == 0 {
                        ox *= sx;
                    } else {
                        if let Some([tw, th]) = assets.texture(self.shown_tex(i)) {
                            ox *= o.size[0] as f32 / tw as f32;
                            oy *= o.size[1] as f32 / th as f32;
                        }
                        ox *= sx;
                    }
                    x += ox;
                    y += oy * sy;
                }
                a.pos[0] += x;
                a.pos[1] += y;
            }
            _ => {}
        }
    }

    /// The type's draw (`+0x20`).
    fn draw_object(&mut self, i: usize, a: &Inherited, w: &mut Walk) {
        let region = *w.regions.last().unwrap();
        let layer = self.layers[self.objects[i].layer].hash;
        match self.objects[i].kind {
            Kind::Sprite => self.draw_sprite(i, a, region, layer, w),
            Kind::AnimatedSprite => self.draw_animated(i, a, region, layer, w),
            Kind::Text => self.draw_text(i, a, region, layer, w),
            Kind::Model => {
                // `0x4dbf90`
                let o = &self.objects[i];
                let extra = [0xc4u16, 0xc8, 0xcc, 0xd0].map(|off| f32::from_bits(o.slot(off)));
                w.list.models.push(Model {
                    id: o.id,
                    object: i,
                    resource: o.slot(0xbc),
                    pos: o.pos,
                    rot: o.rot,
                    extra,
                    rotation_mode: o.slot(0xd4) as u8,
                    order: o.layer_order,
                    field: o.slot(0xc0),
                    region,
                });
            }
            Kind::Puppet => {
                // `0x4dc7b0`
                let o = &self.objects[i];
                let on_spline = o.slot(0x130) & 0xff != 0;
                let (pos, rotation) = if on_spline {
                    let f = |off: u16| f32::from_bits(o.slot(off));
                    ([f(0xdc), f(0xe0), f(0xe4), f(0xe8)], [f(0xcc), f(0xd0), f(0xd4), f(0xd8)])
                } else {
                    (o.pos, [0.0, 0.0, 0.0, 1.0])
                };
                let relative = o.slot(0x131) & 0xff != 0;
                let stands = self.puppet_root(i);
                let pose = self.puppets.get(&i).and_then(|p| p.character.control.pose.clone());
                w.list.puppets.push(Puppet { id: o.id, object: i, relative, pos, rotation, region, stands, pose });
            }
            Kind::Particles => {
                self.particle_template(i);
                if w.list.particles.len() < 0x20 {
                    w.list.particles.push(i);
                }
            }
            Kind::Camera => w.list.camera = Some(i),
            _ => {}
        }
    }

    /// `0x4dd270`: a sprite of its texture's size (its `SIZE` with flag `0x10000`), with the inherited rotation.
    /// The first draw with the texture loaded centres it with `+0xd8 & 2` (`0x4dd3c0`; [`FrontEnd::own_offset`]
    /// places that frame centred already).
    fn draw_sprite(&mut self, i: usize, a: &Inherited, region: Region, layer: u32, w: &mut Walk) {
        let tex = self.shown_tex(i);
        let Some([tw, th]) = w.assets.texture(tex) else { return };
        let o = &mut self.objects[i];
        bind_sprite(o, tex, [tw, th]);
        let size = if o.flags & 0x10000 == 0 { [tw as f32, th as f32] } else { [o.size[0] as f32, o.size[1] as f32] };
        w.list.sprites.push(Sprite {
            id: o.id,
            object: i,
            pos: a.pos,
            rot: a.rot,
            colour: a.colour,
            order: o.layer_order,
            tex,
            size,
            // **[I]** the sprite draw leaves the pivot as the record held it; 0 here
            pivot: [0.0; 2],
            flags: o.flags,
            scroll: [o.slot(0xce) as u16, o.slot(0xd0) as u16],
            region,
            layer,
            material: [0xc8u16, 0xc9, 0xca, 0xcb, 0xcc].map(|off| o.slot(off) as u8),
        });
    }

    /// `0x4db0f0`: the frame's texture times the scales, turned by the sprite's own rotation (`+0x110`) about its
    /// origin. The frame's texture loading centres the origin with `+0xfc & 2` (`0x4db440`).
    fn draw_animated(&mut self, i: usize, a: &Inherited, region: Region, layer: u32, w: &mut Walk) {
        let tex = self.shown_tex(i);
        let Some([tw, th]) = w.assets.texture(tex) else { return };
        let o = &mut self.objects[i];
        if bind(o, tex) && o.slot(0xfc) & 2 != 0 {
            o.set_slot(0xf6, (tw >> 1).wrapping_neg() as u32);
            o.set_slot(0xf8, (th >> 1).wrapping_neg() as u32);
        }
        let (sx, sy) = (f32::from_bits(o.slot(0x108)), f32::from_bits(o.slot(0x10c)));
        let mut px = -(o.slot(0xf6) as u16 as i16 as i32 as f32);
        let mut py = -(o.slot(0xf8) as u16 as i16 as i32 as f32);
        let (fw, fh) = if o.flags & 0x10000 == 0 {
            (tw as f32, th as f32)
        } else {
            let (fw, fh) = (o.size[0] as f32, o.size[1] as f32);
            px *= fw / tw as f32;
            py *= fh / th as f32;
            (fw, fh)
        };
        w.list.sprites.push(Sprite {
            id: o.id,
            object: i,
            pos: a.pos,
            rot: [a.rot[0], a.rot[1], f32::from_bits(o.slot(0x110))],
            colour: a.colour,
            order: o.layer_order,
            tex,
            size: [fw * sx, fh * sy],
            pivot: [px * sx, py * sy],
            flags: o.flags,
            scroll: [o.slot(0xf2) as u16, o.slot(0xf4) as u16],
            region,
            layer,
            material: [0xedu16, 0xee, 0xef, 0xf0, 0xf1].map(|off| o.slot(off) as u8),
        });
    }

    /// An inline sprite's extent (`0x4dd8c0` measures the object of the id in the text's layer with `0x4da340`;
    /// [`FrontEnd::extent_walk`]).
    pub fn inline_extent(&self, l: usize, id: u32, assets: &dyn Assets) -> Option<(f32, f32)> {
        let k = self.layers[l].objects.iter().copied().find(|&k| self.objects[k].id == id)?;
        let (_, e) = self.extent_walk(k, [0.0; 4], assets, &mut Vec::new());
        Some((e[0], e[1]))
    }

    /// `0x4dd7a0`: a run per layout line, after a two-frame delay (`+0xec`) has run out. The layout's width and
    /// height are kept as the text's extent (`+0xd4`, `+0xd6`).
    fn draw_text(&mut self, i: usize, a: &Inherited, region: Region, layer: u32, w: &mut Walk) {
        let delay = self.objects[i].slot(0xec);
        if delay != 0 {
            self.objects[i].set_slot(0xec, delay - 1);
            return;
        }
        let font_id = self.objects[i].slot(0xc0);
        let Some(font) = w.assets.font(font_id) else { return };
        let l = self.objects[i].layer;
        let Some(Layout { lines, placed, width, height }) =
            self.layout_text(i, font, &|id| self.inline_extent(l, id, w.assets))
        else {
            return;
        };
        self.objects[i].set_slot(0xd4, width as u32);
        self.objects[i].set_slot(0xd6, height as u32);
        // the layout moves its inline sprites (`0x4dd8c0`): each to its line's x and the text's y plus the line's
        for p in placed {
            if let Some(k) = self.layers[l].objects.iter().copied().find(|&k| self.objects[k].id == p.id) {
                self.objects[k].pos[0] = p.x;
                self.objects[k].pos[1] = p.y;
            }
        }
        let o = &self.objects[i];
        let n = lines.len() as u32;
        for (k, line) in lines.iter().enumerate() {
            let colour = match line.colour {
                Some(c) => [c[0], c[1], c[2], a.colour[3] * c[3]],
                None => a.colour,
            };
            let mut pos = a.pos;
            pos[0] += line.x;
            pos[1] += line.y;
            w.list.texts.push(TextRun {
                id: o.id.wrapping_add(n - k as u32),
                object: i,
                pos,
                colour,
                font: font_id,
                order: o.layer_order,
                start: line.start,
                count: line.count,
                flags: o.flags,
                effect: o.slot(0xc8),
                region,
                layer,
            });
        }
    }
}

/// The port's note of the texture an object last bound (slots past the object's end; the original keeps texture
/// indices, `+0xc0` for a sprite and the frame table `+0xbc` for an animated sprite). Answers whether `tex` is new.
/// Whether the object's texture is already `tex` (as [`bind`] would find it).
fn bound(o: &crate::layer::Object, tex: Tex) -> bool {
    o.slot(0xff00) == 1 << 31 | (tex.frame as u32) << 23 && o.slot(0xff04) == tex.resource
}

/// `0x4dd3c0`: bind a sprite's texture `tex` of size `size`. A new texture is noted with its size, and with
/// `+0xd8 & 2` centres the sprite (its offset `+0xd2`, `+0xd4` minus half the size).
pub fn bind_sprite(o: &mut crate::layer::Object, tex: Tex, [tw, th]: [u16; 2]) {
    if bind(o, tex) {
        o.set_slot(BOUND_SIZE, tw as u32 | (th as u32) << 16);
        if o.slot(0xd8) & 2 != 0 {
            o.set_slot(0xd2, (tw >> 1).wrapping_neg() as u32);
            o.set_slot(0xd4, (th >> 1).wrapping_neg() as u32);
        }
    }
}

fn bind(o: &mut crate::layer::Object, tex: Tex) -> bool {
    let key = 1 << 31 | (tex.frame as u32) << 23;
    if o.slot(0xff00) == key && o.slot(0xff04) == tex.resource {
        return false;
    }
    o.set_slot(0xff00, key);
    o.set_slot(0xff04, tex.resource);
    true
}

/// An animated sprite's texture: its resource (`+0xc0`) and frame (`+0xc8`).
fn aspt_tex(o: &crate::layer::Object) -> Tex {
    Tex { resource: o.slot(0xc0), frame: o.slot(0xc8) as u8, script: o.script }
}

/// A vertex as the renderer sends it (`0x4e6270`): the position (front-end pixels, z clamped to -1..1), the
/// texture coordinate (in 1/4096 of the texture) and the colour (ARGB).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub uv: [i16; 2],
    pub argb: u32,
}

/// A layer's wipe mask as the renderer holds it (`0x404` bytes per entry: 32 × 32 bytes and the layer's hash).
pub struct Wipe<'a> {
    pub layer: u32,
    pub mask: &'a [u8; 1024],
}

/// `0x4e9bd0`: the mask at `(u, v)` (0 to 1 over the screen): 32 × 32 cells, `u × 30 + 0.5` across, bilinear,
/// a byte over 256. As in the original, the lower row's blend starts from the upper row's right cell.
pub fn wipe_alpha(mask: &[u8; 1024], u: f32, v: f32) -> f32 {
    let (fx, fy) = (u * 30.0 + 0.5, v * 30.0 + 0.5);
    let (ix, iy) = (ftol(fx), ftol(fy));
    let (tx, ty) = ((fx - ix as f32).clamp(0.0, 1.0), (fy - iy as f32).clamp(0.0, 1.0));
    // **[I]** the original reads past the mask for a corner beyond the screen; the port clamps
    let (x0, y0) = (ix.clamp(0, 31) as usize, iy.clamp(0, 31) as usize);
    let (x1, y1) = ((ix.saturating_add(1).clamp(0, 31)) as usize, (iy.saturating_add(1).clamp(0, 31)) as usize);
    let at = |x: usize, y: usize| mask[y * 32 + x] as f32 * 0.00390625;
    let top = (at(x1, y0) - at(x0, y0)) * tx + at(x0, y0);
    let bottom = (at(x1, y1) - at(x0, y1)) * tx + at(x1, y0);
    (bottom - top) * ty + top
}

/// `0x494280` and the alpha doubling after it: each channel × 128, truncated, clamped to 0..255; then the alpha
/// doubled (the PS2's 0x80 is opaque) and clamped. **[I]** the colour is presumably doubled again by the texture
/// stage (`MODULATE2X`), not traced.
pub fn pack_colour(c: [f32; 4]) -> u32 {
    let b = |v: f32| ftol(v * 128.0).clamp(0, 255) as u32;
    let (r, g, bl, a) = (b(c[0]), b(c[1]), b(c[2]), b(c[3]));
    let a2 = (a * 2).min(255);
    a2 << 24 | r << 16 | g << 8 | bl
}

/// `0x4e73e0` -> `0x500470`: a text run's glyph quads (corners as [`sprite_quad`]'s), from the run's position
/// truncated to whole pixels. A glyph is `w + 0.5` × `h + 0.5` pixels at its offsets (both signed) and samples
/// the atlas from half a texel before it to half after (`tex` the atlas size); a space (and an `@` not first)
/// moves by the width of `s`; a new line goes down a line height to the run's x; kerning applies between glyphs.
/// Stops after `run.count` characters. **[I]** the vertex colour is the run's, packed as a sprite's, and the
/// original kerns only fonts with its flag `+0x211d`.
pub fn text_quads(run: &TextRun, text: &[u16], font: &qwc_formats::ffn::Ffn, tex: [u16; 2]) -> Vec<[Vertex; 4]> {
    let x0 = ftol(run.pos[0]) as f32;
    let (mut x, mut y) = (x0, ftol(run.pos[1]) as f32);
    let argb = pack_colour(run.colour);
    // (0 when the font has no `s`)
    let space = Metrics::advance(font, 0x73) as f32;
    let line = Metrics::line_height(font) as i16 as f32;
    let (tw, th) = (tex[0].max(1) as i32, tex[1].max(1) as i32);
    let mut out = Vec::new();
    let chars = text.iter().skip(run.start as usize).take(run.count as usize);
    let mut prev = 0u16;
    for (k, &c) in chars.enumerate() {
        if c == 0 {
            break;
        }
        if c == 10 {
            y += line;
            x = x0;
        } else if c == 0x20 || (c == 0x40 && k > 0) {
            x += space;
        } else if let Some(g) = font.glyphs.iter().find(|g| g.code == c) {
            x += Metrics::kern(font, c, prev) as f32;
            let (gx, gy) = (g.x_offset as f32 + x, g.y_offset as i8 as f32 + y);
            let (gr, gb) = (g.w as f32 + gx + 0.5, g.h as f32 + gy + 0.5);
            let (u0, u1) = (g.u as i32, g.u as i32 + 1 + g.w as i32);
            let (v0, v1) = (g.v as i32, g.v as i32 + 1 + g.h as i32);
            let u = |t: i32| ((t * 0x1000 - 0x800) / tw) as i16;
            let v = |t: i32| ((t * 0x1000 - 0x800) / th) as i16;
            let corner = |px: f32, py: f32, uu: i16, vv: i16| Vertex { pos: [px, py, -1.0], uv: [uu, vv], argb };
            out.push([
                corner(gx, gb, u(u0), v(v1)),
                corner(gr, gb, u(u1), v(v1)),
                corner(gx, gy, u(u0), v(v0)),
                corner(gr, gy, u(u1), v(v0)),
            ]);
            x += Metrics::advance(font, c) as f32;
        }
        prev = c;
    }
    out
}

/// `0x4e6270`: a sprite's quad, corners in the order bottom left, bottom right, top left, top right. Turned by
/// `rot[2]` about `pos + pivot`, or with flag `0x800000` laid along the direction `(rot[0], rot[1])`; the texture
/// inset by half a texel (`tex` its size); with flag `0x4000` and the layer's wipe, each corner's alpha times the
/// mask there. **[I]** the region's matrix (identity for a viewport) is left out: positions stay in the front
/// end's pixels.
pub fn sprite_quad(s: &Sprite, tex: [u16; 2], wipe: Option<&Wipe>) -> [Vertex; 4] {
    let ([x, y, z, _], [w, h]) = (s.pos, s.size);
    let mut p = [[x, y + h], [x + w, y + h], [x, y], [x + w, y]];
    if s.flags & 0x80_0000 == 0 {
        if s.rot[2] != 0.0 {
            let (sn, cs) = s.rot[2].sin_cos();
            let (cx, cy) = (x + s.pivot[0], y + s.pivot[1]);
            for q in &mut p {
                let (dx, dy) = (q[0] - cx, q[1] - cy);
                *q = [(cs * dx - sn * dy) + cx, cs * dy + sn * dx + cy];
            }
        }
    } else {
        let (a, b) = (s.rot[0], s.rot[1]);
        let (f1, f2) = (a * h * 0.375, b * h * 0.5);
        let (f3, f4) = (b * w * 0.375, w * a * -0.5);
        p = [
            [(x - f3) - f1, (y - f4) - f2],
            [(f3 + x) - f1, (y + f4) - f2],
            [(x - f3) + f1, (y - f4) + f2],
            [f1 + f3 + x, y + f4 + f2],
        ];
    }
    let mut colours = [s.colour; 4];
    if s.flags & 0x4000 != 0 {
        if let Some(wp) = wipe.filter(|wp| wp.layer == s.layer) {
            for (c, q) in colours.iter_mut().zip(&p) {
                c[3] *= wipe_alpha(wp.mask, q[0] * 0.001953125, q[1] * 0.001953125);
            }
        }
    }
    let (su, sv) = ((s.scroll[0] & 0xfff) as i32, (s.scroll[1] & 0xfff) as i32);
    let hu = if tex[0] != 0 { 0x800 / tex[0] as i32 } else { 0 };
    let hv = if tex[1] != 0 { 0x800 / tex[1] as i32 } else { 0 };
    let uv = [
        [su + hu, sv - hv + 0x1000],
        [su + 0x1000 - hu, sv - hv + 0x1000],
        [su + hu, sv + hv],
        [su + 0x1000 - hu, sv + hv],
    ];
    std::array::from_fn(|k| Vertex {
        pos: [p[k][0], p[k][1], z.clamp(-1.0, 1.0)],
        uv: [uv[k][0] as i16, uv[k][1] as i16],
        argb: pack_colour(colours[k]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::{Layer, Object};

    struct Fixed;

    impl Assets for Fixed {
        fn texture(&self, tex: Tex) -> Option<[u16; 2]> {
            (tex.resource != 0).then_some([64, 32])
        }
        fn font(&self, _: u32) -> Option<&dyn Metrics> {
            None
        }
    }

    /// A sprite (resource 7) at (10, 20) holding a sprite at (5, 5), both at half colour.
    #[test]
    fn a_camera_looks_at_its_interest_or_ahead() {
        let mut fe = FrontEnd::new();
        let mut cam = Object::new(Kind::Camera, 0);
        cam.pos = [1.0, 2.0, 3.0, 0.5];
        for (off, v) in [(0xc4u16, 1.0f32), (0xd8, 1.0), (0xe8, 80.0), (0xec, 10.0), (0xf0, 1000.0)] {
            cam.set_slot(off, v.to_bits());
        }
        fe.objects.push(cam);
        let v = fe.camera_view(0);
        assert_eq!((v.eye, v.dir, v.up), ([1.0, 2.0, 3.0, 0.5], [1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]));
        assert_eq!((v.fov, v.fov_y, v.near, v.far), (80.0, 60.0, 10.0, 1000.0));
        // an interest object turns it: the way to it, normalised
        let mut target = Object::new(Kind::Null, 0);
        target.pos = [1.0, 2.0, 7.0, 0.0];
        fe.objects.push(target);
        fe.objects[0].set_slot(0xbc, crate::layer::object_handle(1));
        assert_eq!(fe.camera_view(0).dir, [0.0, 0.0, 1.0, 0.0]);
        // halfway between two views: blended, the direction normalised, the focus off when one end has none
        let (a, b) = (fe.camera_view(0), View { dir: [0.0, 1.0, 0.0, 0.0], focus: [0.0, 0.0], ..fe.camera_view(0) });
        let h = b.blend(&a, 0.5);
        let r = std::f32::consts::FRAC_1_SQRT_2;
        assert!((h.dir[1] - r).abs() < 1e-6 && (h.dir[2] - r).abs() < 1e-6 && h.focus == [0.0, 0.0]);
        // and the last camera drawn in a frame is the list's
        let mut a = DrawList { camera: Some(3), ..Default::default() };
        a.append(DrawList::default());
        assert_eq!(a.camera, Some(3));
        a.append(DrawList { camera: Some(4), ..Default::default() });
        assert_eq!(a.camera, Some(4));
    }

    fn two() -> FrontEnd {
        let mut fe = FrontEnd::new();
        for (k, pos) in [[10.0, 20.0], [5.0, 5.0]].into_iter().enumerate() {
            let mut o = Object::new(Kind::Sprite, 0);
            o.id = k as u32 + 1;
            o.pos[..2].copy_from_slice(&pos);
            o.colour = [0.5; 4];
            o.set_slot(0xbc, 7);
            fe.objects.push(o);
        }
        fe.objects[0].children = vec![1];
        fe.objects[1].parent = Some(0);
        let (name, hash) = ("t.vso".into(), 0x1234);
        fe.layers.push(Layer {
            script: 0,
            name,
            hash,
            refs: 1,
            alive: true,
            screen: None,
            roots: vec![0],
            objects: vec![0, 1],
            wipe: Default::default(),
        });
        fe
    }

    #[test]
    fn children_inherit_and_hidden_parents_still_lead() {
        let mut fe = two();
        let list = fe.draw_screen(&[0], &mut vec![], &Fixed);
        let s = &list.sprites;
        assert_eq!((s[0].pos[..2].to_vec(), s[1].pos[..2].to_vec()), (vec![10.0, 20.0], vec![15.0, 25.0]));
        assert_eq!((s[0].colour[0], s[1].colour[0]), (0.5, 0.25));
        assert_eq!((s[1].size, s[1].layer, s[1].region), ([64.0, 32.0], 0x1234, ROOT));
        // a hidden parent (flag 4) draws nothing but its child still draws where it inherits
        fe.objects[0].flags |= 4;
        let list = fe.draw_screen(&[0], &mut vec![], &Fixed);
        assert_eq!(list.sprites.len(), 1);
        assert_eq!(list.sprites[0].pos[0], 15.0);
        // a layer already drawn this frame (by another screen) is not drawn again
        let mut drawn = vec![0];
        assert!(fe.draw_screen(&[0], &mut drawn, &Fixed).sprites.is_empty());
    }

    #[test]
    fn an_adjusted_object_draws_moved_with_its_children_and_the_scripts_still_see_it_unmoved() {
        let mut fe = two();
        fe.adjust.insert((0x1234, 1), Adjust { offset: [3.0, -4.0], ..Default::default() });
        let list = fe.draw_screen(&[0], &mut vec![], &Fixed);
        let s = &list.sprites;
        assert_eq!((s[0].pos[..2].to_vec(), s[1].pos[..2].to_vec()), (vec![13.0, 16.0], vec![18.0, 21.0]));
        // the measured place the scripts and the wipes read is the scripts' own
        assert_eq!(f32::from_bits(fe.objects[0].slot(0x3c)), 10.0);
        assert_eq!(fe.objects[0].pos[0], 10.0);
        // the child's own adds to it; another layer's object of the same id is not moved
        fe.adjust.insert((0x1234, 2), Adjust { offset: [1.0, 0.0], ..Default::default() });
        fe.adjust.insert((0x9999, 1), Adjust { offset: [100.0, 0.0], ..Default::default() });
        assert_eq!(fe.draw_screen(&[0], &mut vec![], &Fixed).sprites[1].pos[0], 19.0);
        // hidden: neither it nor its children
        fe.adjust.insert((0x1234, 1), Adjust { hidden: true, ..Default::default() });
        assert!(fe.draw_screen(&[0], &mut vec![], &Fixed).sprites.is_empty());
    }

    #[test]
    fn a_scaled_or_pinned_object_draws_its_children_with_it() {
        let mut fe = two();
        // twice its size about where it stands (10, 20): the child 5 along and down is drawn 10 along and down
        fe.adjust.insert((0x1234, 1), Adjust { scale: 2.0, ..Default::default() });
        let list = fe.draw_screen(&[0], &mut vec![], &Fixed);
        let (parent, child) = (list.sprites[0].object, list.sprites[1].object);
        assert_eq!(list.xf_of(parent).at([10.0, 20.0]), [10.0, 20.0]);
        assert_eq!(list.xf_of(child).at([15.0, 25.0]), [20.0, 30.0]);
        // on a window 64 units wider each side: kept at the right edge, the child with it, even pinned left itself
        fe.margins = [64.0, 0.0];
        fe.adjust.insert((0x1234, 1), Adjust { pin: [Pin::End, Pin::None], ..Default::default() });
        fe.adjust.insert((0x1234, 2), Adjust { pin: [Pin::Start, Pin::None], ..Default::default() });
        let list = fe.draw_screen(&[0], &mut vec![], &Fixed);
        assert_eq!(list.xf_of(parent).at([10.0, 20.0]), [74.0, 20.0]);
        assert_eq!(list.xf_of(child).at([15.0, 25.0]), [79.0, 25.0]);
        // worked out again from the list's places, as the editor does for an edit not yet drawn
        assert_eq!(list.xf_under(&fe, &fe.adjust, fe.margins, child), Some(list.xf_of(child)));
        let mut scaled = fe.adjust.clone();
        scaled.insert((0x1234, 1), Adjust { scale: 2.0, ..Default::default() });
        let xf = list.xf_under(&fe, &scaled, fe.margins, child).unwrap();
        assert_eq!(xf.at([15.0, 25.0]), [20.0 - 64.0, 30.0]);
        // anchored to a point of the window: a quarter of the way across moves half the margin left, and the
        // named pins are its ends and middle
        fe.adjust.insert((0x1234, 1), Adjust { pin: [Pin::At(0.25), Pin::None], ..Default::default() });
        fe.adjust.remove(&(0x1234, 2));
        let list = fe.draw_screen(&[0], &mut vec![], &Fixed);
        assert_eq!(list.xf_of(parent).at([10.0, 20.0]), [-22.0, 20.0]);
        for (t, pin) in [(0.0, Pin::Start), (0.5, Pin::None), (1.0, Pin::End)] {
            let a: std::collections::HashMap<(u32, u32), Adjust> =
                [((0x1234u32, 1u32), Adjust { pin: [Pin::At(t), Pin::None], ..Default::default() })].into();
            let b = [((0x1234, 1), Adjust { pin: [pin, Pin::None], ..Default::default() })].into();
            assert_eq!(
                list.xf_under(&fe, &a, fe.margins, parent),
                list.xf_under(&fe, &b, fe.margins, parent),
                "At({t})"
            );
        }
        // stretched edge to edge: the screen's left reaches the window's
        fe.adjust.clear();
        fe.adjust.insert((0x1234, 1), Adjust { pin: [Pin::Stretch, Pin::None], ..Default::default() });
        let list = fe.draw_screen(&[0], &mut vec![], &Fixed);
        assert_eq!(list.xf_of(parent).at([0.0, 20.0]), [-64.0, 20.0]);
        assert_eq!(list.xf_of(parent).at([512.0, 20.0]), [576.0, 20.0]);
    }

    #[test]
    fn a_centred_sprite_is_centred_from_its_first_frame() {
        let mut fe = two();
        fe.objects[0].set_slot(0xd8, 2);
        // centred from the first frame (the original only from the next: its `+0x38` runs before its draw binds the
        // texture), and the child with it
        let first = fe.draw_screen(&[0], &mut vec![], &Fixed);
        assert_eq!(first.sprites[0].pos[..2].to_vec(), vec![10.0 - 32.0, 20.0 - 16.0]);
        assert_eq!(first.sprites[1].pos[0], 15.0 - 32.0);
        let next = fe.draw_screen(&[0], &mut vec![], &Fixed);
        assert_eq!(next.sprites[0].pos[..2].to_vec(), vec![10.0 - 32.0, 20.0 - 16.0]);
        assert_eq!(next.sprites[1].pos[0], 15.0 - 32.0);
    }

    #[test]
    fn layers_measure_places_and_extents() {
        let mut fe = two();
        let f = |fe: &FrontEnd, i: usize, off: u16| f32::from_bits(fe.objects[i].slot(off));
        // before they draw the sprites have no texture bound, so no extent of their own; the child's place is its
        // parent's plus its position, and the parent's extent reaches the child's far side (15 + 0 - 10)
        fe.measure(0, [0.0; 4], &Fixed);
        assert_eq!((f(&fe, 1, 0x3c), f(&fe, 1, 0x40), f(&fe, 1, 0x4c)), (15.0, 25.0, 0.0));
        assert_eq!((f(&fe, 0, 0x4c), f(&fe, 0, 0x50)), (5.0, 5.0));
        // drawn, each has its texture's 64 × 32, and the parent's reaches 15 + 64 - 10 across
        fe.draw_screen(&[0], &mut vec![], &Fixed);
        fe.measure(0, [0.0; 4], &Fixed);
        assert_eq!((f(&fe, 1, 0x4c), f(&fe, 1, 0x50)), (64.0, 32.0));
        assert_eq!((f(&fe, 0, 0x4c), f(&fe, 0, 0x50)), (69.0, 37.0));
        // with flag 0x10000 a sprite's `SIZE`
        fe.objects[1].flags |= 0x10000;
        fe.objects[1].size = [100, 8];
        assert_eq!(fe.measure(1, [0.0; 4], &Fixed)[..2], [100.0, 8.0]);
    }

    #[test]
    fn a_layer_wipe_steps_as_it_draws() {
        use crate::wipe::{LayerWipe, WipeData};
        let mut fe = two();
        let mut f = vec![32, 32, 2, 0];
        f.extend([0u8; 1024]);
        f.extend([255u8; 1024]);
        let data = std::rc::Rc::new(WipeData::new(&f).unwrap());
        fe.layers[0].wipe = LayerWipe { data: Some(data), frames: 2, callback: 0x55, ..Default::default() };
        let first = fe.draw_screen(&[0], &mut vec![], &Fixed);
        assert_eq!((first.wipes.len(), first.wipes[0].mask[0]), (1, 0));
        assert!(first.wipe(0x1234).is_some() && first.wipe(9).is_none());
        // the second and last draw: frame 1 (the one after is past the end)
        let second = fe.draw_screen(&[0], &mut vec![], &Fixed);
        assert_eq!(second.wipes[0].mask[0], 255);
        assert_eq!(fe.wipe_callbacks, [(0, 0x55, crate::layer::object_handle(0))]);
        // between the ticks the mask is halfway, whatever the time
        assert_eq!(second.blend(&first, 0.3).wipes[0].mask[0], 127);
        assert!(fe.draw_screen(&[0], &mut vec![], &Fixed).wipes.is_empty());
    }

    #[test]
    fn a_replaced_text_is_what_the_object_shows() {
        let mut fe = FrontEnd::new();
        let mut o = Object::new(Kind::Text, 0);
        o.id = 9;
        fe.objects.push(o);
        fe.layers.push(Layer {
            script: 0,
            name: "t.vso".into(),
            hash: 0x1234,
            refs: 1,
            alive: true,
            screen: None,
            roots: vec![0],
            objects: vec![0],
            wipe: Default::default(),
        });
        // no string manager: the scripts' text is none, yet the replacement shows
        assert_eq!(fe.text(0), None);
        fe.text_over.insert((0x1234, 9), "HI".encode_utf16().collect());
        assert_eq!(fe.text(0).map(String::from_utf16_lossy).as_deref(), Some("HI"));
        assert_eq!(fe.script_text(0), None);
        // another layer's object of the same id keeps its own
        fe.text_over.clear();
        fe.text_over.insert((0x9999, 9), "NO".encode_utf16().collect());
        assert_eq!(fe.text(0), None);
    }

    #[test]
    fn an_added_sprite_is_a_real_object_of_its_layer() {
        use crate::frontend::SpriteAdd;
        let mut fe = two();
        // a sprite's picture replaced: its records take the minted image
        fe.image_over.insert((0x1234, 2), 9);
        let list = fe.draw_screen(&[0], &mut vec![], &Fixed);
        assert_eq!(list.sprites[1].tex, Tex { resource: 9, frame: 0, script: IMAGE_SCRIPT });
        assert_eq!(list.sprites[0].tex.resource, 7);
        // an added sprite under object id 1 (at 10, 20): drawn at its place from it, with its image and order
        let add =
            SpriteAdd { id: 77, image: 5, pos: [30.0, 40.0], size: [0; 2], parent: 1, order: 60, animate_like: 0 };
        fe.additions.insert(0x1234, vec![add.clone()]);
        fe.sync_additions();
        let list = fe.draw_screen(&[0], &mut vec![], &Fixed);
        let s = list.sprites.iter().find(|s| s.id == 77).expect("added");
        assert_eq!((s.pos[0], s.pos[1], s.order), (40.0, 60.0, 60));
        assert_eq!(s.tex, Tex { resource: 5, frame: 0, script: IMAGE_SCRIPT });
        // its Adjusts move it like any object's
        fe.adjust.insert((0x1234, 77), Adjust { offset: [1.0, 0.0], ..Default::default() });
        let list = fe.draw_screen(&[0], &mut vec![], &Fixed);
        assert_eq!(list.sprites.iter().find(|s| s.id == 77).unwrap().pos[0], 41.0);
        // moved to a root with a size of its own; then taken out
        fe.additions.insert(0x1234, vec![SpriteAdd { parent: 0, size: [100, 8], ..add }]);
        fe.sync_additions();
        let list = fe.draw_screen(&[0], &mut vec![], &Fixed);
        let s = list.sprites.iter().find(|s| s.id == 77).unwrap();
        assert_eq!((s.pos[0], s.size), (31.0, [100.0, 8.0]));
        fe.additions.clear();
        fe.sync_additions();
        assert!(!fe.draw_screen(&[0], &mut vec![], &Fixed).sprites.iter().any(|s| s.id == 77));
    }

    #[test]
    fn an_added_animated_sprite_runs_its_models_animation() {
        use crate::frontend::SpriteAdd;
        let mut fe = two();
        // the model: 3 frames, "show frame 1 then stop", alpha material, unit scales
        let mut m = Object::new(Kind::AnimatedSprite, 0);
        m.id = 5;
        m.set_slot(0xc0, 7);
        m.set_slot(0xc4, 3);
        m.set_slot(0xed, 2);
        m.set_slot(0x108, 1.0f32.to_bits());
        m.set_slot(0x10c, 1.0f32.to_bits());
        m.program = vec![0x0101_0000, 0];
        fe.objects.push(m);
        let mi = fe.objects.len() - 1;
        fe.layers[0].roots.push(mi);
        fe.layers[0].objects.push(mi);
        let add = SpriteAdd { id: 88, image: 9, pos: [1.0, 2.0], size: [0; 2], parent: 0, order: 50, animate_like: 5 };
        fe.additions.insert(0x1234, vec![add]);
        fe.sync_additions();
        let k = (fe.layers[0].objects.iter().copied()).find(|&k| fe.objects[k].id == 88).unwrap();
        assert_eq!(fe.objects[k].kind, Kind::AnimatedSprite);
        assert_eq!(fe.objects[k].program, fe.objects[mi].program);
        // its own copy of the frame script runs: frame 1 of the minted image, with the model's material
        crate::layer::animate(&mut fe.objects[k]);
        let list = fe.draw_screen(&[0], &mut vec![], &Fixed);
        let s = list.sprites.iter().find(|s| s.id == 88).unwrap();
        assert_eq!(s.tex, Tex { resource: 9, frame: 1, script: IMAGE_SCRIPT });
        assert_eq!(s.material[0], 2);
        // a replaced picture on a live animated sprite keeps the live frame
        fe.objects[mi].set_slot(0xc8, 2);
        fe.image_over.insert((0x1234, 5), 11);
        let list = fe.draw_screen(&[0], &mut vec![], &Fixed);
        let s = list.sprites.iter().find(|s| s.id == 5).unwrap();
        assert_eq!(s.tex, Tex { resource: 11, frame: 2, script: IMAGE_SCRIPT });
    }

    fn sprite(rot: f32, pivot: [f32; 2]) -> Sprite {
        Sprite {
            id: 1,
            object: 0,
            pos: [100.0, 50.0, 0.5, 0.0],
            rot: [0.0, 0.0, rot],
            colour: [1.0, 0.5, 0.25, 1.0],
            order: 0,
            tex: Tex { resource: 1, frame: 0, script: 0 },
            size: [64.0, 32.0],
            pivot,
            flags: 0,
            scroll: [0, 0],
            region: ROOT,
            layer: 9,
            material: [2, 1, 0, 0, 0],
        }
    }

    #[test]
    fn lists_blend_records_by_id() {
        let before = sprite(0.0, [0.0; 2]);
        // this tick: moved, in a region that moved by 10, faded; and a new sprite with no record before
        let region = Region { x: 10, ..ROOT };
        let now = Sprite { pos: [130.0, 50.0, 0.5, 0.0], colour: [1.0, 0.5, 0.25, 0.0], region, ..before };
        let new = Sprite { id: 2, ..before };
        let a = DrawList { sprites: vec![before], ..Default::default() };
        let b = DrawList { sprites: vec![now, new], ..Default::default() };
        let h = b.blend(&a, 0.5);
        // relative to their regions: 100 then 120, halfway 110, in the region at 10
        assert_eq!((h.sprites[0].pos[0], h.sprites[0].colour[3]), (120.0, 0.5));
        assert_eq!(h.sprites[1], new);
        // the blend is clamped: past the tick it is this tick's list
        assert_eq!(b.blend(&a, 3.0), b);
    }

    #[test]
    fn puppets_blend_where_they_stand_and_their_poses() {
        let mut pose = qwc_anim::pose::Pose::new(vec![[0.0; 3]], vec![[1.0; 3]], vec![0]);
        pose.rotation[0] = Some([0.0; 3]);
        let quarter =
            crate::spline::matrix([0.0, std::f32::consts::FRAC_1_SQRT_2, 0.0, std::f32::consts::FRAC_1_SQRT_2]);
        let mut stands = crate::place::IDENTITY;
        stands[12] = 2.0;
        let before = Puppet {
            id: 1,
            object: 3,
            relative: false,
            pos: [0.0; 4],
            rotation: [0.0, 0.0, 0.0, 1.0],
            region: ROOT,
            stands: Some(stands),
            pose: Some(pose.clone()),
        };
        let mut now = before.clone();
        let mut turned = quarter;
        turned[12] = 4.0;
        now.stands = Some(turned);
        now.pose.as_mut().unwrap().rotation[0] = Some([1.0, 0.0, 0.0]);
        let a = DrawList { puppets: vec![before], ..Default::default() };
        let b = DrawList { puppets: vec![now], ..Default::default() };
        let h = b.blend(&a, 0.5);
        let p = &h.puppets[0];
        // half way along and an eighth of a turn round; the pose's angle half way
        let s = p.stands.unwrap();
        let eighth = std::f32::consts::FRAC_PI_4.cos();
        assert!((s[12] - 3.0).abs() < 1e-6 && (s[0] - eighth).abs() < 1e-5, "{s:?}");
        assert!((p.pose.as_ref().unwrap().rotation[0].unwrap()[0] - 0.5).abs() < 1e-6);
        assert_eq!(b.blend(&a, 1.0), b);
    }

    #[test]
    fn quads_inset_the_texture_and_pack_the_colour() {
        let q = sprite_quad(&sprite(0.0, [0.0; 2]), [64, 32], None);
        let pos: Vec<[f32; 2]> = q.iter().map(|v| [v.pos[0], v.pos[1]]).collect();
        assert_eq!(pos, [[100.0, 82.0], [164.0, 82.0], [100.0, 50.0], [164.0, 50.0]]);
        // half a texel: 2048 / 64 = 32 across, 2048 / 32 = 64 down
        let uv: Vec<[i16; 2]> = q.iter().map(|v| v.uv).collect();
        assert_eq!(uv, [[32, 4032], [4064, 4032], [32, 64], [4064, 64]]);
        // 128 per unit, the alpha doubled
        assert_eq!(q[0].argb, 0xff80_4020);
        // a quarter turn about the centre
        let q = sprite_quad(&sprite(std::f32::consts::FRAC_PI_2, [32.0, 16.0]), [64, 32], None);
        let (cx, cy) = (132.0, 66.0);
        let p0 = q[2].pos;
        assert!((p0[0] - (cx + 16.0)).abs() < 1e-4 && (p0[1] - (cy - 32.0)).abs() < 1e-4, "{p0:?}");
    }

    #[test]
    fn wipes_fade_the_corners_of_their_layer() {
        let mut mask = [0u8; 1024];
        mask.iter_mut().for_each(|m| *m = 128);
        assert_eq!(wipe_alpha(&mask, 0.3, 0.7), 0.5);
        let mut s = sprite(0.0, [0.0; 2]);
        s.flags = 0x4000;
        let w = Wipe { layer: 9, mask: &mask };
        let q = sprite_quad(&s, [64, 32], Some(&w));
        assert_eq!(q[0].argb >> 24, 0x80);
        // another layer's wipe leaves it alone
        let w = Wipe { layer: 8, mask: &mask };
        assert_eq!(sprite_quad(&s, [64, 32], Some(&w))[0].argb >> 24, 0xff);
    }
}
