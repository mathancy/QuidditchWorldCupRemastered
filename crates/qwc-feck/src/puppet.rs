//! Puppets (`PUPT`, `docs/ANIMATION.md`): front-end objects that are animated characters. A puppet holds a
//! `qwc_anim::control::Character` made from its animation resource (`0x4d8ff0` -> `0x4dc200`), the
//! `LayerObj_Puppet_*` natives play its clips, and each frame the characters run (`0x469770`), their clip ends
//! calling the scripts back through the puppet's 8 callback records (`+0x134`, 0x34 bytes, class `0x5abb8c`).

use qwc_anim::control::{Character, Event, Mode, SetClip};
use qwc_anim::pose::Pose;

use crate::frontend::FrontEnd;
use crate::layer;
use crate::memory::Memory;
use crate::vm::{Result, Vm};

/// What a puppet's animation resource gives (`0x4dc200` loads `"%s.skl"` and `"%s.fas"` of the puppet's first
/// resource): the clips, and with the skeleton, the nodes the pose is evaluated into.
#[derive(Debug, Clone)]
pub struct PuppetSet {
    pub clips: Vec<SetClip>,
    pub pose: Option<Pose>,
}

/// The host's source of clip sets: a puppet's animation resource (the layer script's hash, and the record: its id,
/// group and name) to `name.fas` and `name.skl`.
pub type SetLoader = Box<dyn FnMut(u32, &qwc_formats::vso::Resource) -> Option<PuppetSet>>;

/// A callback record (`+0x134 + 0x34 k`).
#[derive(Debug, Clone, PartialEq)]
pub struct Callback {
    /// The clip it waits for (`+8`, the handle's clip bits; `u32::MAX` none).
    pub clip: u32,
    /// Ends to wait for (`+0xc`, the play's count).
    pub count: i32,
    /// The script function (`+0x10`; 0 none).
    pub function: u32,
    /// The clip's name (`+0x14`), and where the port keeps it in VM memory for the script.
    pub name: String,
    pub name_addr: u32,
}

impl Default for Callback {
    fn default() -> Callback {
        Callback { clip: u32::MAX, count: 0, function: 0, name: String::new(), name_addr: 0 }
    }
}

/// A puppet's animated character and its callbacks.
#[derive(Debug, Clone)]
pub struct Puppet {
    pub character: Character,
    pub callbacks: [Callback; 8],
    /// `LayerObj_Puppet_LookAtPosition`'s target (`0x468250`). **[I]** the head's tracking is not ported.
    pub look_at: Option<[f32; 3]>,
}

/// `0x4dc610`'s flag choice: -1 loops unless the count is 1; 0 once; 1 holds; 2 loops; anything else passes the
/// `fallback` word on as flags (the original's slip: the slot, or the object for `PlayAnimFlags`).
pub fn play_flags(select: u32, count: u32, fallback: u32) -> u32 {
    match select as i32 {
        -1 => (count != 1) as u32,
        0 => 0,
        1 => 4,
        2 => 1,
        _ => fallback,
    }
}

impl FrontEnd {
    /// The puppet of object `i` with its clips (`PUPT` field 0 names the animation resource in the layer's
    /// `_ResourceData`). Without a loader, or without the clips, there is none: the natives then do nothing, as
    /// for a character not loaded (`+0x124` bit 1).
    pub(crate) fn make_puppet(&mut self, i: usize, resources: &[qwc_formats::vso::Resource]) {
        let id = self.objects[i].field(0);
        let Some(r) = resources.iter().find(|r| r.id == id) else { return };
        let Some(load) = self.anim_sets.as_mut() else { return };
        let Some(set) = load(self.objects[i].script, r) else { return };
        let mut character = Character::default();
        character.control.sets.push(set.clips);
        character.control.pose = set.pose;
        self.puppets.insert(i, Puppet { character, callbacks: Default::default(), look_at: None });
    }

    /// `0x468580` for a puppet, then (with a function, on slot 0) callback record `sub` waits for the clip
    /// (`0x4d64c0`, `0x4dc610`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn puppet_play(
        &mut self,
        mem: &mut Memory,
        obj: usize,
        name: &str,
        count: u32,
        mode: u32,
        flags: u32,
        function: u32,
        slot: u32,
        sub: u32,
    ) -> Result<()> {
        let Some(p) = self.puppets.get_mut(&obj) else { return Ok(()) };
        let waits = function != 0 && slot == 0 && (sub as usize) < 8;
        let cb = waits.then_some((sub, sub as usize));
        let handle = p.character.control.play(name, flags, Mode::from_word(mode), slot as usize, cb, 1.0, 0.0);
        if handle == qwc_anim::control::NONE {
            *self.missing_clips.entry(name.to_string()).or_default() += 1;
            return Ok(());
        }
        if waits {
            let c = &mut p.callbacks[sub as usize];
            c.clip = handle >> 7 & 0x7ff;
            c.count = count as i32;
            c.function = function;
            if c.name != name || c.name_addr == 0 {
                // **[I]** the original copies the name into a 32-byte field without a bound
                c.name = name.to_string();
                c.name_addr = mem.alloc(name.len() + 1);
                let mut b = name.as_bytes().to_vec();
                b.push(0);
                mem.write(c.name_addr, &b)?;
            }
        }
        Ok(())
    }

    /// One frame of the animated objects (`0x469770`, each character's `+0x8c` with the frame's milliseconds),
    /// then the callbacks their clip ends call (`0x4dc6e0`): the record's count goes down, and at 0 its function
    /// runs once in the puppet's layer with `(puppet, 0, name)`. **[I]** its place in the frame is not traced.
    pub fn anim_frame(&mut self, vm: &mut Vm) -> Result<()> {
        let mut calls = Vec::new();
        let keys: Vec<usize> = self.puppets.keys().copied().collect();
        for i in keys {
            if !self.layers.get(self.objects[i].layer).is_some_and(|l| l.alive) {
                continue;
            }
            let mut events = Vec::new();
            let p = self.puppets.get_mut(&i).unwrap();
            p.character.update(self.frame_ms, &mut self.crt_rand, &mut events);
            for e in events {
                let Event::End { token, handle, .. } = e else { continue };
                let c = &mut p.callbacks[token as usize];
                if c.clip != handle >> 7 & 0x7ff {
                    continue;
                }
                c.count -= 1;
                if c.count > 0 {
                    continue;
                }
                c.count = 0;
                let f = std::mem::take(&mut c.function);
                c.clip = u32::MAX;
                if f != 0 {
                    calls.push((self.objects[i].layer, f, [layer::object_handle(i), 0, c.name_addr]));
                }
            }
        }
        for (l, f, args) in calls {
            self.puppet_callbacks += 1;
            let script = self.layers[l].script;
            vm.execute(self, script, f, &args)?;
        }
        Ok(())
    }
}
