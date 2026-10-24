//! Layers and layer objects (`docs/FECK_VM.md`, "Layers"): a screen's `.vso` holds a `_LayerData` tree that
//! `FUN_004d9760` builds into objects. Every object shares a base (position, colour, rotation, size, flags, id,
//! event handlers, user data, links to its parent, siblings, children and its layer) and keeps its type's own
//! fields, parsed by the type table at `0x5ef8b0`.

use qwc_formats::vso::Layer as Node;

/// Object types (the table at `0x5ef8b0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Null,
    /// `BMNU`, a button menu.
    Menu,
    /// `SPRT`
    Sprite,
    /// `TEXT`
    Text,
    /// `CMRA`
    Camera,
    /// `MODL`
    Model,
    /// `PUPT`, a puppet (an animated character model).
    Puppet,
    /// `ASPT`, an animated sprite.
    AnimatedSprite,
    /// `PTCL`, particles.
    Particles,
    /// `QDHC`, a Quidditch card.
    Card,
    /// `SLDR`, a slider.
    Slider,
    /// `PMNU`, a page menu.
    PageMenu,
}

impl Kind {
    pub fn from_tag(tag: &str) -> Option<Kind> {
        Some(match tag {
            "NULL" => Kind::Null,
            "BMNU" => Kind::Menu,
            "SPRT" => Kind::Sprite,
            "TEXT" => Kind::Text,
            "CMRA" => Kind::Camera,
            "MODL" => Kind::Model,
            "PUPT" => Kind::Puppet,
            "ASPT" => Kind::AnimatedSprite,
            "PTCL" => Kind::Particles,
            "QDHC" => Kind::Card,
            "SLDR" => Kind::Slider,
            "PMNU" => Kind::PageMenu,
            _ => return None,
        })
    }
}

/// Event numbers of `OVNT` (`FUN_004de700`) and the handler slot each fills.
pub mod event {
    pub const CONSTRUCT: usize = 1;
    pub const DESTRUCT: usize = 2;
    pub const HIGHLIGHT: usize = 3;
    pub const UNHIGHLIGHT: usize = 4;
    pub const CHOOSE: usize = 5;
    pub const CANCEL: usize = 6;
    pub const PRE_TRANSITION: usize = 7;
    pub const POST_TRANSITION: usize = 8;
    pub const INPUT: usize = 9;
    pub const PROCESS: usize = 10;
    pub const GENERAL: usize = 11;
}

/// Flag bits (`+0x5c`) the traced code tests.
pub mod flag {
    /// Skipped by the event dispatch (`0x4d9840`, `0x4da880`).
    pub const DISABLED: u32 = 0x08;
    /// Takes the highlight (`0x4da510`).
    pub const HIGHLIGHTABLE: u32 = 0x10;
    /// Highlighted.
    pub const HIGHLIGHTED: u32 = 0x20;
    /// Bits `0x80 << pad`: which controllers it takes input from (`0x4da6a0`).
    pub const INPUT_ANY: u32 = 0x1f80;
}

/// The script's opaque handles for objects and layers (the original hands out C++ pointers).
pub const OBJECT_BASE: u32 = 0x2000_0000;
pub const LAYER_BASE: u32 = 0x3000_0000;
pub const HANDLE_STRIDE: u32 = 0x40;

pub fn object_handle(i: usize) -> u32 {
    OBJECT_BASE + i as u32 * HANDLE_STRIDE
}

pub fn layer_handle(i: usize) -> u32 {
    LAYER_BASE + i as u32 * HANDLE_STRIDE
}

pub fn object_index(h: u32) -> Option<usize> {
    ((OBJECT_BASE..LAYER_BASE).contains(&h) && (h - OBJECT_BASE).is_multiple_of(HANDLE_STRIDE))
        .then(|| ((h - OBJECT_BASE) / HANDLE_STRIDE) as usize)
}

pub fn layer_index(h: u32) -> Option<usize> {
    (h >= LAYER_BASE && (h - LAYER_BASE).is_multiple_of(HANDLE_STRIDE))
        .then(|| ((h - LAYER_BASE) / HANDLE_STRIDE) as usize)
}

/// A timed change the scripts add with `LayerObj_AddAction_*` (`0x4d5850` and its siblings): it runs for
/// `duration` frames, calling its update with the fraction done each frame (`0x4da880`).
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub kind: ActionKind,
    /// The script's id for it (`LayerObj_KillAction` finds it by this).
    pub id: u32,
    pub elapsed: u32,
    pub duration: u32,
    /// Start and end values (the action's own meaning: alpha, colour, position or size; a timer keeps its
    /// repeat count in `from[0]`).
    pub from: [f32; 4],
    pub to: [f32; 4],
    /// The script function run with the object when the action ends (0 = none).
    pub callback: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Alpha,
    Colour,
    Position,
    Size,
    Timer,
    /// Along the keys at `keys` (VM memory, `count` of them; `qwc_feck::spline`). A looping one (sprites only)
    /// never ends.
    Spline {
        spline: crate::spline::Spline,
        keys: u32,
        count: u32,
        looping: bool,
    },
    /// A sprite through the frames of its resource (`LayerObj_Sprite_AddAction_Animate`, vtable `0x5abeb8`):
    /// `count` frames on from the one bound as it began (`+0x2c`; `base`, the original's resource entry `+0x28`),
    /// `dir` 1 forwards or -1 backwards (`+0x30`), the frame last shown (`+0x38`, -999 at first), back and forth
    /// for ever with `ping_pong` (`+0x34`).
    Animate {
        count: u32,
        dir: i32,
        ping_pong: bool,
        base: Option<u8>,
        last: i32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub kind: Kind,
    pub layer: usize,
    /// Its layer script's hash (`+0x64` -> `+0x54`): its resource ids are that script's.
    pub script: u32,
    /// `+0x60`
    pub id: u32,
    /// `+0x5c`
    pub flags: u32,
    /// `+0x08`-`+0x14`
    pub pos: [f32; 4],
    /// `+0x18`-`+0x24`, red, green, blue, alpha (0 to 1).
    pub colour: [f32; 4],
    /// `+0x28`-`+0x30`, radians (`HEAD` sets the y rotation from degrees).
    pub rot: [f32; 3],
    /// `+0x34`: the type's second field for sprites and the third for text: the material layer its textures
    /// register in (`0x502ab0`; the engine draws by layer, inferred).
    pub layer_order: u32,
    /// `+0x38`, `+0x3a`
    pub size: [u16; 2],
    /// Script function hashes by event number (`OVNT`); 0 = none.
    pub events: [u32; 12],
    /// `UDTA` blocks: (key, VM address of the words, word count).
    pub user: Vec<(u32, u32, u32)>,
    /// The type's own fields as stored.
    pub fields: Vec<u32>,
    /// The type part of the object by its offset in the original (`+0xbc` onwards), seeded by the type parsers
    /// and read and written by the type natives.
    pub slots: std::collections::BTreeMap<u16, u32>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub actions: Vec<Action>,
    /// A text's tab stops (`+0xd8`, 8 `u16`, `0xffff` unused), added by `TEXt` properties.
    pub tabs: [u16; 8],
    /// An animated sprite's frame script (`ASPD`, up to and including its end tag). The original keeps pointers
    /// into it (`+0xcc` the start, `+0xd0` the next command, `+0xd8` the labels, `+0xe8` the loop); the port keeps
    /// word indices in those slots.
    pub program: Vec<u32>,
}

impl Object {
    pub fn new(kind: Kind, layer: usize) -> Object {
        Object {
            kind,
            layer,
            script: 0,
            id: 0,
            flags: 0,
            pos: [0.0; 4],
            colour: [1.0; 4],
            rot: [0.0; 3],
            layer_order: 0,
            size: [0; 2],
            events: [0; 12],
            user: Vec::new(),
            fields: Vec::new(),
            slots: Default::default(),
            parent: None,
            children: Vec::new(),
            actions: Vec::new(),
            tabs: [0xffff; 8],
            program: Vec::new(),
        }
    }

    /// The type's field `i` as stored in the layer data.
    pub fn field(&self, i: usize) -> u32 {
        self.fields.get(i).copied().unwrap_or(0)
    }

    /// The type part at offset `off` (0 when never set).
    pub fn slot(&self, off: u16) -> u32 {
        self.slots.get(&off).copied().unwrap_or(0)
    }

    pub fn set_slot(&mut self, off: u16, v: u32) {
        self.slots.insert(off, v);
    }

    /// Where the type parsers put the stored fields (`0x4d8960`, `0x4d93f0`, `0x4d9550`, `0x4d8e10`; the camera's
    /// floats and object links are kept in `fields`).
    fn seed_slots(&mut self) {
        let f = |i: usize| self.field(i);
        let seeds: Vec<(u16, u32)> = match self.kind {
            Kind::Menu => vec![(0xbc, f(0))],
            Kind::Sprite => vec![
                (0xbc, f(0)),
                (0xc8, f(2) & 0xff),
                (0xc9, f(3) & 0xff),
                (0xca, f(4) & 0xff),
                (0xcb, (f(5) != 0) as u32),
                (0xcc, (f(6) != 0) as u32),
                (0xd2, f(7) & 0xffff),
                (0xd4, f(8) & 0xffff),
                (0xd8, f(9) as u16 as i16 as i32 as u32),
            ],
            Kind::Text => vec![(0xbc, f(0)), (0xc0, f(1)), (0xc8, f(3))],
            Kind::Model => vec![(0xbc, f(0)), (0xc0, f(1))],
            // `0x4d8f50`: an emitter from the start (`+0xbc`), no description yet (`+0x110`)
            Kind::Particles => vec![(0xbc, 1), (0x110, 0)],
            // `0x4d8ab0`, every float from 16.16: the forward vector `+0xc4` and up `+0xd4` (their w 0), then
            // `+0xe4`, the field of view `+0xe8`, the near plane `+0xec` (at least 0.25), the far plane `+0xf0`, the
            // focus distance `+0xf4` and `+0xf8`. The interest and focus objects (fields 12, 13) are found by id
            // as the layer builds (`build`)
            Kind::Camera => {
                let v = |i: usize| fx(f(i)).to_bits();
                let mut s = vec![(0xd0, 0), (0xe0, 0), (0xec, fx(f(8)).max(0.25).to_bits())];
                for (k, off) in [0xc4u16, 0xc8, 0xcc, 0xd4, 0xd8, 0xdc, 0xe4, 0xe8].into_iter().enumerate() {
                    s.push((off, v(k)));
                }
                for (k, off) in [(9, 0xf0u16), (10, 0xf4), (11, 0xf8)] {
                    s.push((off, v(k)));
                }
                s
            }
            // `0x4d87a0` (the constructor `0x4dab90` zeroes the rest and sets both scales to 1): the resource id, the
            // frame count, three colour bytes, two flags, the offset flags (bit 1 centres the frame, `0x4db440`) and
            // the x and y offsets
            Kind::AnimatedSprite => vec![
                (0xc0, f(0)),
                (0xc4, f(1)),
                (0xed, f(3) & 0xff),
                (0xee, f(4) & 0xff),
                (0xef, f(5) & 0xff),
                (0xf0, (f(6) != 0) as u32),
                (0xf1, (f(7) != 0) as u32),
                (0xfc, f(8)),
                (0xf6, f(9) & 0xffff),
                (0xf8, f(10) & 0xffff),
                (0x108, 1f32.to_bits()),
                (0x10c, 1f32.to_bits()),
            ],
            // `0x4d9280`: minimum and maximum (16.16 to floats), the step count, and the start value as a step
            // (`0x4dcfe0` clamps it to 0..count-1; the slider has no handlers yet, so none runs)
            Kind::Slider => {
                let (min, max, count) = (fx(f(0)), fx(f(1)), f(2) as i32);
                let at = ftol((fx(f(3)) - min) / (max - min) * count.wrapping_sub(1) as f32);
                vec![
                    (0xbc, min.to_bits()),
                    (0xc0, max.to_bits()),
                    (0xc4, count as u32),
                    (0xc8, slider_clamp(at, count)),
                ]
            }
            _ => vec![],
        };
        self.slots.extend(seeds);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// The `.vso` this layer runs (index in the VM).
    pub script: usize,
    pub name: String,
    /// `+0x54`: the script's object hash (the name hash of its mixed-case name, `.vso` header `0x08`).
    pub hash: u32,
    /// `+0x4c`: how many screens hold the layer (a `NoReload` layer is shared by consecutive screens).
    pub refs: u32,
    /// False once released by its last screen.
    pub alive: bool,
    /// `+0x5c`: the screen that built the layer (`qwc_feck::screens`), whatever screens share it later.
    pub screen: Option<usize>,
    /// Top-level objects in order, and every object in creation order (`+0x6c` list).
    pub roots: Vec<usize>,
    pub objects: Vec<usize>,
    /// `+0x20`: its wipe (`qwc_feck::wipe`).
    pub wipe: crate::wipe::LayerWipe,
}

/// 16.16 fixed point.
fn fx(w: u32) -> f32 {
    w as i32 as f32 / 65536.0
}

/// MSVC's `_ftol` (`0x581d24`): truncation, with `0x80000000` for anything out of range (NaN included).
pub fn ftol(x: f32) -> i32 {
    if x.is_finite() && x > -2147483649.0 && x < 2147483648.0 {
        x.trunc() as i32
    } else {
        i32::MIN
    }
}

/// A slider's step, clamped to `0..count` as `0x4dcfe0` and `0x4d6a50` do (signed compares).
pub fn slider_clamp(at: i32, count: i32) -> u32 {
    let at = at.max(0);
    (if at >= count { count.wrapping_sub(1) } else { at }) as u32
}

/// Milliseconds an animated sprite's wait loses per frame (`0x4dac40`).
pub const WAIT_PER_FRAME: i16 = 0x29;

/// An animated sprite's frame step (`0x4dac40`, its `+0x1c`, run after the object's actions each frame): count
/// down a wait, then run the frame script until it waits or stops. Each command is a word: the top byte the
/// opcode, the next two bytes `a` (bits 16-23) and `b` (bits 8-15), with `a | b << 8` their 16-bit value `ab`.
///
/// | Op | Does |
/// |---|---|
/// | 0 | stop here (the next frame runs it again) |
/// | 1 | show frame `a` (`+0xc8`) |
/// | 2 | wait `ab` milliseconds (`+0xd4`) |
/// | 3 | label `a` is the next command (`+0xd8 + 4a`) |
/// | 4 | go to label `a` |
/// | 5 | a loop starts after this, run `a` more times (`+0xec`, `+0xe8`); `0xff`: once per frame left |
/// | 6 | back to the loop's start while its count lasts |
/// | 7, 8 | add `a` (signed) to the x or y offset (`+0x100`, `+0x104`) |
/// | 9 | move `a` (signed) frames, clamped to the frames there are |
/// | 10, 11 | add `ab` to `+0xf2`, `+0xf4` |
/// | 12, 13 | set the x or y offset to `ab` |
/// | 14, 15 | set the origin `+0xf6`, `+0xf8` to `ab` |
/// | 16, 17 | set the x or y scale (`+0x108`, `+0x10c`) to `a` (signed) / 100 |
/// | 18, 19 | add `a` / 100 to it |
/// | 20 | set the rotation (`+0x110`) to `ab` (signed) degrees |
/// | 21 | add `ab` degrees, wrapped into 0 to 2π |
///
/// Other opcodes do nothing (the end tag among them: the original reads on past it). **[I]** the port stops at
/// the end of the script, and after 65536 commands in one step (a loop with no wait hangs the original).
pub fn animate(o: &mut Object) {
    let wait = o.slot(0xd4) as u16 as i16;
    if wait > 0 {
        let left = wait.wrapping_sub(WAIT_PER_FRAME);
        o.set_slot(0xd4, left as u16 as u32);
        if left > 0 {
            return;
        }
        o.set_slot(0xd4, 0);
    }
    let byte = |o: &Object, off: u16| o.slot(off) as u8;
    let add_f = |o: &mut Object, off: u16, d: f32| o.set_slot(off, (f32::from_bits(o.slot(off)) + d).to_bits());
    // the original's x87 keeps the products wide until the store
    let degrees = |ab: u32| ab as u16 as i16 as f64 * std::f32::consts::PI as f64 * (1.0f32 / 180.0) as f64;
    for _ in 0..0x10000 {
        let at = o.slot(0xd0);
        let Some(&w) = o.program.get(at as usize) else { return };
        let next = at + 1;
        o.set_slot(0xd0, next);
        let (a, b) = ((w >> 16) & 0xff, (w >> 8) & 0xff);
        let ab = a | b << 8;
        let label = 0xd8 + 4 * a as u16;
        match w >> 24 {
            0 => {
                o.set_slot(0xd0, at);
                return;
            }
            1 => o.set_slot(0xc8, a),
            2 => {
                o.set_slot(0xd4, ab);
                return;
            }
            3 => o.set_slot(label, next),
            4 => o.set_slot(0xd0, o.slot(label)),
            5 => {
                let n = if a == 0xff { (o.slot(0xc4) as u8).wrapping_sub(byte(o, 0xc8)) } else { a as u8 };
                o.set_slot(0xec, n as u32);
                o.set_slot(0xe8, next);
            }
            6 => {
                let n = byte(o, 0xec);
                if n != 0 {
                    o.set_slot(0xec, (n - 1) as u32);
                    o.set_slot(0xd0, o.slot(0xe8));
                }
            }
            7 | 8 => {
                let off = if w >> 24 == 7 { 0x100 } else { 0x104 };
                o.set_slot(off, o.slot(off).wrapping_add(a as u8 as i8 as u32));
            }
            9 => {
                let f = byte(o, 0xc8).wrapping_add(a as u8) as i8;
                let count = o.slot(0xc4) as i32;
                let f = if f < 0 {
                    0
                } else if count <= f as i32 {
                    (o.slot(0xc4) as u8).wrapping_sub(1)
                } else {
                    f as u8
                };
                o.set_slot(0xc8, f as u32);
            }
            10 | 11 => {
                let off = if w >> 24 == 10 { 0xf2 } else { 0xf4 };
                o.set_slot(off, (o.slot(off) as u16).wrapping_add(ab as u16) as u32);
            }
            12 => o.set_slot(0x100, ab),
            13 => o.set_slot(0x104, ab),
            14 => o.set_slot(0xf6, ab),
            15 => o.set_slot(0xf8, ab),
            16 | 17 => o.set_slot(if w >> 24 == 16 { 0x108 } else { 0x10c }, (a as u8 as i8 as f32 * 0.01).to_bits()),
            18 | 19 => add_f(o, if w >> 24 == 18 { 0x108 } else { 0x10c }, a as u8 as i8 as f32 * 0.01),
            20 => o.set_slot(0x110, (degrees(ab) as f32).to_bits()),
            21 => {
                let tau = std::f32::consts::TAU;
                let mut r = (degrees(ab) + f32::from_bits(o.slot(0x110)) as f64) as f32;
                if r > tau {
                    r -= tau;
                }
                if r < 0.0 {
                    r += tau;
                }
                o.set_slot(0x110, r.to_bits());
            }
            _ => {}
        }
    }
}

/// Build `nodes` into `objects` under `parent`, for layer `layer`. `user` allocates a block of words in VM memory
/// and returns its address.
pub fn build(
    nodes: &[Node],
    layer: usize,
    parent: Option<usize>,
    objects: &mut Vec<Object>,
    order: &mut Vec<usize>,
    user: &mut dyn FnMut(&[u32]) -> u32,
) -> Vec<usize> {
    let mut made = Vec::new();
    for n in nodes {
        let Node::Object { kind, fields, id, flags, children } = n else { continue };
        let Some(k) = Kind::from_tag(kind) else { continue };
        let idx = objects.len();
        let mut o = Object::new(k, layer);
        o.id = *id;
        o.flags = *flags;
        o.fields = fields.clone();
        o.parent = parent;
        o.seed_slots();
        // the type parsers' writes into the base (0x4d93f0, 0x4d9550)
        match k {
            Kind::Sprite => o.layer_order = o.field(1),
            Kind::AnimatedSprite => o.layer_order = o.field(2),
            Kind::Text => o.layer_order = o.field(2),
            _ => {}
        }
        objects.push(o);
        order.push(idx);
        made.push(idx);
        if k == Kind::Camera {
            // `0x4d8ab0`: the interest (`+0xbc`) and focus (`+0xc0`) by id, among the layer's objects made so far
            // (this one included: it joins the list first); none found leaves 0
            for (field, off) in [(12, 0xbcu16), (13, 0xc0)] {
                let want = objects[idx].field(field);
                let found = (want != 0).then(|| order.iter().find(|&&j| objects[j].id == want)).flatten();
                objects[idx].set_slot(off, found.map_or(0, |&j| object_handle(j)));
            }
        }
        // `0x4d8d00` walks the records in order and hands each parser this object and the object made last at
        // this level (this one until a child object appears, then that child). Every property parser writes the
        // last-made object except `OVNT` (`0x4de700`), which writes this one: events never move to a child.
        let mut target = idx;
        let mut kids = Vec::new();
        for c in children {
            match c {
                Node::Property { name, words } => {
                    let t = if *name == "OVNT" { idx } else { target };
                    apply(&mut objects[t], name, words, user)
                }
                Node::Object { .. } => {
                    let made_child = build(std::slice::from_ref(c), layer, Some(idx), objects, order, user);
                    if let Some(&k) = made_child.first() {
                        target = k;
                        kids.push(k);
                    }
                }
            }
        }
        objects[idx].children = kids;
    }
    made
}

/// The property parsers (`0x4de960`-`0x4dea30`).
fn apply(o: &mut Object, name: &str, w: &[u32], user: &mut dyn FnMut(&[u32]) -> u32) {
    let at = |i: usize| w.get(i).copied().unwrap_or(0);
    match name {
        "POS" => {
            o.pos = [fx(at(0)), fx(at(1)), fx(at(2)), 0.0];
        }
        "POSX" => o.pos[0] = fx(at(0)),
        "POSY" => o.pos[1] = fx(at(0)),
        "POSZ" => o.pos[2] = fx(at(0)),
        "HEAD" => o.rot[1] = fx(at(0)).to_radians(),
        "OVNT" => {
            let e = at(0) as usize;
            if (1..=11).contains(&e) {
                o.events[e] = at(1);
            }
        }
        "COL" => match at(0) {
            k @ 0..=3 => o.colour[k as usize] = fx(at(1)),
            4 => o.colour = [fx(at(1)), fx(at(2)), fx(at(3)), fx(at(4))],
            _ => {}
        },
        // the low halves of two words
        "SIZE" => o.size = [at(0) as u16, at(1) as u16],
        // a menu's handlers (`0x4d8a00`): 0 wrap past the start `+0xd0`, 1 wrap past the end `+0xd4`, 2 moved
        // forward `+0xdc`, 3 moved back `+0xd8`
        // a slider's handlers (`0x4d9370`): 0-4 into `+0xd4`, `+0xd8`, `+0xe0`, `+0xdc`, `+0xe4` (the last runs
        // when the value is set)
        "SLDE" => {
            if let Some(off) = [0xd4u16, 0xd8, 0xe0, 0xdc, 0xe4].get(at(0) as usize) {
                o.set_slot(*off, at(1));
            }
        }
        "BMNE" => {
            if let Some(off) = [0xd0u16, 0xd4, 0xdc, 0xd8].get(at(0) as usize) {
                o.set_slot(*off, at(1));
            }
        }
        // `0x4d8a70`: 0 the DataStore key remembering the choice `+0xc8`, 1 the start index `+0xc4`
        "BMNP" => match at(0) {
            0 => o.set_slot(0xc8, at(1)),
            1 => o.set_slot(0xc4, at(1) & 0xffff),
            _ => {}
        },
        // `0x4d9610` / `0x4de6c0`: the word's low half goes into the first unused tab stop; with all 8 used it is
        // dropped (**[I]** on other kinds the original writes into whatever lies at `+0xd8`; the port ignores it)
        "TEXt" if o.kind == Kind::Text => {
            if let Some(t) = o.tabs.iter_mut().find(|t| **t == 0xffff) {
                *t = at(0) as u16;
            }
        }
        // `0x4d8920`: the frame script starts here (`+0xcc`, `+0xd0`), whatever the object
        "ASPD" => {
            o.program = w.to_vec();
            o.set_slot(0xcc, 0);
            o.set_slot(0xd0, 0);
        }
        "UDTA" => {
            let (n, key) = (at(0) as usize, at(1));
            let words: Vec<u32> = (0..n).map(|i| at(2 + i)).collect();
            let addr = user(&words);
            o.user.push((key, addr, n as u32 * 4));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_objects_with_properties() {
        let tree = vec![Node::Object {
            kind: "SPRT",
            fields: vec![7, 3, 0, 0, 0, 0, 0, 0, 0, 0],
            id: 0x100,
            flags: 0x10,
            children: vec![
                Node::Property { name: "POS", words: vec![0x10000, 0x28000, 0xffff0000] },
                Node::Property { name: "COL", words: vec![3, 0x8000] },
                Node::Property { name: "OVNT", words: vec![1, 0xabcd] },
                Node::Property { name: "UDTA", words: vec![2, 0x55, 9, 10] },
                Node::Object { kind: "TEXT", fields: vec![11, 2, 5, 0], id: 0x101, flags: 0, children: vec![] },
                // after a child object, a property applies to that child (`0x4d8d00`), except an event
                Node::Property { name: "POSX", words: vec![0x30000] },
                Node::Property { name: "OVNT", words: vec![2, 0xbeef] },
            ],
        }];
        let (mut objs, mut order) = (Vec::new(), Vec::new());
        let mut blocks = Vec::new();
        let roots = build(&tree, 0, None, &mut objs, &mut order, &mut |w: &[u32]| {
            blocks.push(w.to_vec());
            0x900
        });
        assert_eq!(roots, [0]);
        assert_eq!(order, [0, 1]);
        let s = &objs[0];
        assert_eq!((s.kind, s.id, s.flags, s.layer_order), (Kind::Sprite, 0x100, 0x10, 3));
        assert_eq!(s.pos, [1.0, 2.5, -1.0, 0.0]);
        assert_eq!(s.colour, [1.0, 1.0, 1.0, 0.5]);
        assert_eq!(s.events[event::CONSTRUCT], 0xabcd);
        assert_eq!((s.user.clone(), blocks), (vec![(0x55, 0x900, 8)], vec![vec![9, 10]]));
        assert_eq!((s.children.clone(), objs[1].parent, objs[1].layer_order), (vec![1], Some(0), 5));
        assert_eq!((objs[0].pos[0], objs[1].pos[0]), (1.0, 3.0));
        assert_eq!((objs[0].events[2], objs[1].events[2]), (0xbeef, 0), "OVNT stays on its list's object");
        assert_eq!(object_index(object_handle(5)), Some(5));
        assert_eq!(layer_index(layer_handle(2)), Some(2));
        assert_eq!(object_index(layer_handle(2)), None);
    }

    #[test]
    fn sliders_take_their_range_and_handlers() {
        // 0..1 in 10 steps starting at 0.5 (the step is trunc(0.5 * 9) = 4)
        let tree = vec![Node::Object {
            kind: "SLDR",
            fields: vec![0, 0x10000, 10, 0x8000],
            id: 1,
            flags: 0,
            children: vec![Node::Property { name: "SLDE", words: vec![4, 0x1234] }],
        }];
        let (mut objs, mut order) = (Vec::new(), Vec::new());
        build(&tree, 0, None, &mut objs, &mut order, &mut |_: &[u32]| 0);
        let o = &objs[0];
        assert_eq!((f32::from_bits(o.slot(0xbc)), f32::from_bits(o.slot(0xc0))), (0.0, 1.0));
        assert_eq!((o.slot(0xc4), o.slot(0xc8), o.slot(0xe4)), (10, 4, 0x1234));
        assert_eq!([slider_clamp(-3, 10), slider_clamp(12, 10), slider_clamp(7, 10)], [0, 9, 7]);
        assert_eq!([ftol(2.9), ftol(-2.9), ftol(f32::NAN), ftol(3e9)], [2, -2, i32::MIN, i32::MIN]);
    }

    #[test]
    fn cameras_take_their_view_and_find_their_objects() {
        // forward x, up y, the field of view 80, a near plane under the minimum, far 1000, focus 4.5 and 1; the
        // interest is the NULL made before it, the focus an id nothing has
        let one = 0x10000;
        let fields = vec![one, 0, 0, 0, one, 0, 0, 80 * one, 0x1000, 1000 * one, 0x48000, one, 5, 0x99];
        let tree = vec![
            Node::Object { kind: "NULL", fields: vec![], id: 5, flags: 0, children: vec![] },
            Node::Object { kind: "CMRA", fields, id: 6, flags: 0, children: vec![] },
        ];
        let (mut objs, mut order) = (Vec::new(), Vec::new());
        build(&tree, 0, None, &mut objs, &mut order, &mut |_: &[u32]| 0);
        let c = &objs[1];
        let f = |off: u16| f32::from_bits(c.slot(off));
        assert_eq!([f(0xc4), f(0xc8), f(0xcc), f(0xd0)], [1.0, 0.0, 0.0, 0.0]);
        assert_eq!([f(0xd4), f(0xd8), f(0xdc), f(0xe0)], [0.0, 1.0, 0.0, 0.0]);
        assert_eq!([f(0xe8), f(0xec), f(0xf0), f(0xf4), f(0xf8)], [80.0, 0.25, 1000.0, 4.5, 1.0]);
        assert_eq!((c.slot(0xbc), c.slot(0xc0)), (object_handle(0), 0));
    }
}
