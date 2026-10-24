//! The front-end screen manager (`docs/FRONTEND_FLOW.md`, "The runtime"): the game flow over `game.flo`
//! (`GameFlow`, `0x4fb9c0`), the flow helper that keeps the current, next and previous screens (`0x4fcd20`), and the
//! screens themselves (`CFECK_Screen`, vtable `0x5ab518`) with their states, layers and transitions.
//!
//! The original's three parts talk through message channels; the port calls the listeners directly, in the order the
//! messages go out. **[T]** unless marked **[I]**. Departures, all **[I]**:
//! - every script is loaded into the VM before the flow starts, so a screen's loading steps take one frame (the
//!   original requests each layer's script and resources and waits for them over several frames);
//! - load sets are only recorded ([`Manager::loads`]); nothing loads asynchronously, so the "loaded" message is sent
//!   at once;
//! - an exit index or node hash that leads nowhere is refused (the original reads past the table or crashes).

use std::collections::HashMap;

use qwc_formats::flow::{name_hash, Flow, Screen as ScreenDef};

use crate::frontend::{FrontEnd, Request};
use crate::layer::event;
use crate::vm::Vm;

/// A screen's state (`screen +0x24`).
pub mod state {
    /// Made; its scripts and resources are loading.
    pub const LOADING: u8 = 1;
    /// Its layers are built.
    pub const BUILT: u8 = 2;
    /// Current: constructed and running.
    pub const ACTIVE: u8 = 3;
    /// Leaving: the pre-transition runs until the scripts (or `finish`) say it is done.
    pub const PRE_TRANSITION: u8 = 4;
    /// The pre-transition is done; waiting for the next screen.
    pub const PRE_DONE: u8 = 5;
    /// The next screen is built: the post-transition runs until the screen is ready to go.
    pub const POST_TRANSITION: u8 = 6;
    /// Gone.
    pub const LEFT: u8 = 7;
}

/// The DataStore key `ReadyToExit` writes the screen's node hash to.
pub const PREVIOUS_SCREEN_ID: u32 = 0xb301_029b;

/// One screen (`CFECK_Screen`).
#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    /// Index in `Screens.lst`.
    pub def: usize,
    /// `+0x04`
    pub node: u32,
    /// `+0x24`
    pub state: u8,
    /// `+0x3c`-`+0x50`: the front end's layers.
    pub layers: Vec<usize>,
    /// `+0x64`: finish the transitions without the scripts.
    pub finish: bool,
    /// The load-set slots (`+0x10`): the fixed ones, then each `@Key`'s DataStore value.
    pub load_sets: Vec<u32>,
}

/// Something that happened, for tests and tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The flow moved from node `from` to node `to` (indices in `game.flo`).
    Change { from: usize, to: usize },
    /// The flow settled on a node (it is current and the old screen is gone).
    Entered(usize),
    /// A screen's state changed.
    State { node: u32, state: u8 },
    /// Load sets a new screen needs that the last one did not have, and the ones it no longer needs.
    LoadSets { load: Vec<u32>, release: Vec<u32> },
    /// A request was refused (why).
    Refused(String),
    /// A script stopped with an error (what).
    Error(String),
}

/// What an exit request names (`{flags, value, kind}` sent on `0x1043904`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Exit(u32),
    Node(u32),
}

pub struct Manager {
    pub flow: Flow,
    pub defs: Vec<ScreenDef>,
    /// Scripts by the hash of their name without the extension (as `Screens.lst` names them): VM index and bytes.
    scripts: HashMap<u32, (usize, Vec<u8>)>,
    /// The flow's node (`+0x38`), an index in `flow.nodes`.
    pub node: usize,
    from: Option<usize>,
    to: Option<usize>,
    starting: bool,
    start_load: bool,
    /// `+0x48` sets in, `+0x49` screen in, `+0x4a` unloaded, `+0x4b` pre-transition done, `+0x4c` ready to go.
    sets_in: bool,
    screen_in: bool,
    unloaded: bool,
    pre_done: bool,
    ready: bool,
    /// `+0x3c`: the old screen is still unloading.
    busy: bool,
    pending: Option<Target>,
    /// The helper's screens (`+0x34`, `+0x38`, `+0x3c`), indices in `defs`.
    pub current: Option<usize>,
    pub next: Option<usize>,
    pub previous: Option<usize>,
    /// The screens by `defs` index.
    pub screens: Vec<Option<Screen>>,
    /// The load sets of the last committed screen (`0x4fa4b0`).
    pub loads: Vec<u32>,
    pub steps: Vec<Step>,
}

impl Manager {
    /// The manager over `game.flo` and `Screens.lst`; `scripts` are every `.vso` already loaded into the VM, as
    /// (VM index, file bytes).
    pub fn new(flow: Flow, defs: Vec<ScreenDef>, vm: &Vm, scripts: Vec<(usize, Vec<u8>)>) -> Manager {
        let scripts = scripts
            .into_iter()
            .filter_map(|(i, b)| {
                let hash = qwc_formats::vso::Vso::parse(&b).ok()?.hash;
                (i < vm.scripts.len()).then_some((hash, (i, b)))
            })
            .collect();
        let n = defs.len();
        Manager {
            flow,
            defs,
            scripts,
            node: 0,
            from: None,
            to: None,
            starting: false,
            start_load: false,
            sets_in: false,
            screen_in: false,
            unloaded: false,
            pre_done: false,
            ready: false,
            busy: false,
            pending: None,
            current: None,
            next: None,
            previous: None,
            screens: vec![None; n],
            loads: Vec::new(),
            steps: Vec::new(),
        }
    }

    /// Start the flow at a node (`0x1043908` → `0x4fb9a0`); the boot starts it at the top group's entry.
    pub fn start(&mut self, node: usize) {
        self.node = node;
        self.starting = true;
        self.start_load = true;
    }

    /// The node the flow starts at: the first root group's entry (`MainGroup` → `PreFlow_XboxSkip`).
    pub fn boot_node(&self) -> Option<usize> {
        let g = *self.flow.roots.first()? as usize;
        Some(self.flow.groups.get(g)?.entry as usize)
    }

    /// The screen showing a node, by hash (`0x4fd340`).
    fn def_of(&self, hash: u32) -> Option<usize> {
        self.defs.iter().position(|d| d.hash == hash)
    }

    /// The current screen, when there is one.
    pub fn current_screen(&self) -> Option<&Screen> {
        self.screens[self.current?].as_ref()
    }

    fn set_state(&mut self, d: usize, state: u8) {
        if let Some(s) = self.screens[d].as_mut() {
            s.state = state;
            let node = s.node;
            self.steps.push(Step::State { node, state });
        }
    }

    /// One frame: the event queue, the screens' updates (`0x4fcd20`), the requests the scripts made, then the flow
    /// (`0x4fb9c0`).
    pub fn frame(&mut self, fe: &mut FrontEnd, vm: &mut Vm) {
        // `0x4e2950`: the controllers (`0x4d7700`), then the event pump (`0x4e2300`)
        fe.step_controllers();
        if let Err(e) = fe.pump(vm) {
            fe.queue.clear();
            self.steps.push(Step::Error(format!("events: {e}")));
        }
        // **[I]** the wipes that ended in the last draw (the original runs their callbacks in the draw, which it does
        // with the update)
        if let Err(e) = fe.run_wipe_callbacks(vm) {
            self.steps.push(Step::Error(format!("wipe callback: {e}")));
        }
        let mut processed = Vec::new();
        for d in [self.current, self.previous, self.next].into_iter().flatten() {
            self.update(fe, vm, d, &mut processed);
        }
        // the memory card's step ends the front-end manager's update (`0x4e2950`). **[I]** its place against the
        // flow's update is not traced
        fe.memcard_frame(vm);
        if let Err(e) = fe.anim_frame(vm) {
            self.steps.push(Step::Error(format!("puppets: {e}")));
        }
        self.take_requests(fe, vm);
        self.flow_update(fe, vm);
        self.take_requests(fe, vm);
    }

    /// The frame's draw list. The helper's update (`0x4fcd20`) goes through the current, previous and next screens.
    /// A screen in states 3 to 6 walks its layers in its state method (`0x4fcd90`: `+0x2c`, `+0x20`, `+0x24`,
    /// `+0x28` of vtable `0x5ab518`; all of them call `0x4e2f00`), so each such screen draws. A layer draws once a
    /// frame even when several screens hold it. **[I]** The original processes and draws in the same walk; the port
    /// processes in [`Manager::frame`] and draws here, after it. Text objects that hold no string yet are formatted
    /// first, as their draw (`0x4dd7a0`) does.
    pub fn draw(&self, fe: &mut FrontEnd, vm: &Vm, assets: &dyn crate::draw::Assets) -> crate::draw::DrawList {
        let mut list = crate::draw::DrawList::default();
        let mut drawn = Vec::new();
        for d in [self.current, self.previous, self.next].into_iter().flatten() {
            let Some(s) = self.screens[d].as_ref() else { continue };
            if (state::ACTIVE..=state::POST_TRANSITION).contains(&s.state) {
                for &l in &s.layers {
                    if fe.layers[l].alive && !drawn.contains(&l) {
                        fe.format_texts(&vm.mem, l);
                    }
                }
                list.append(fe.draw_screen(&s.layers, &mut drawn, assets));
            }
        }
        // `0x4fe530`: the frame's camera as the frame ends
        list.view = list.camera.map(|c| fe.camera_view(c));
        // the renderer's particle pass (`0x4e42c0`); **[I]** once a tick
        let emitters = list.particles.clone();
        list.sparks = fe.particles_frame(&emitters);
        list
    }

    /// `0x4fcd90`: a screen's update by state.
    fn update(&mut self, fe: &mut FrontEnd, vm: &mut Vm, d: usize, processed: &mut Vec<usize>) {
        let Some(s) = self.screens[d].clone() else {
            return;
        };
        match s.state {
            state::LOADING => {
                // the loading steps are done at once (**[I]**, see the module notes); "in" (`0x4fd2b0`), and "sets
                // in" as nothing loads asynchronously
                self.screen_in = true;
                self.sets_in = true;
            }
            state::ACTIVE..=state::POST_TRANSITION => {
                // `0x4e2f00`: each layer processes once per frame
                for &l in &s.layers {
                    if fe.layers[l].alive && !processed.contains(&l) {
                        processed.push(l);
                        if let Err(e) = fe.process(vm, l) {
                            self.steps.push(Step::Error(format!("{}: process: {e}", fe.layers[l].name)));
                        }
                    }
                }
                if s.finish && s.state == state::PRE_TRANSITION {
                    self.pre_transition_done(d);
                }
                if s.finish && s.state == state::POST_TRANSITION {
                    self.ready = true;
                }
            }
            _ => {}
        }
    }

    /// `0x4e2e70`: from state 4 only.
    fn pre_transition_done(&mut self, d: usize) {
        if self.screens[d].as_ref().is_some_and(|s| s.state == state::PRE_TRANSITION) {
            self.set_state(d, state::PRE_DONE);
            self.pre_done = true;
        }
    }

    /// The screen that built a layer, when it still exists.
    fn screen_of_layer(&self, fe: &FrontEnd, layer: usize) -> Option<usize> {
        let d = fe.layers.get(layer)?.screen?;
        self.screens[d].as_ref().map(|_| d)
    }

    /// The flow natives' requests (`0x4d31e0`-`0x4d32b0`), in the order the scripts made them.
    fn take_requests(&mut self, fe: &mut FrontEnd, vm: &mut Vm) {
        for r in std::mem::take(&mut fe.requests) {
            let (layer, target, finish) = match r {
                Request::Exit { layer, exit, finish } => (layer, Target::Exit(exit), finish),
                Request::ExitToNode { layer, hash, finish } => {
                    // `0x4faa50`: the exit leading to that node or group
                    match self.flow.exit_to(self.node, hash) {
                        Some(e) => (layer, Target::Exit(e as u32), finish),
                        None => {
                            self.steps.push(Step::Refused(format!("{hash:08x} is not linked to the current node")));
                            continue;
                        }
                    }
                }
                Request::JumpToNode { layer, hash, finish } => (layer, Target::Node(hash), finish),
                Request::PreTransitionDone { layer } => {
                    if let Some(d) = self.screen_of_layer(fe, layer) {
                        self.pre_transition_done(d);
                    }
                    continue;
                }
                Request::ReadyToExit { layer } => {
                    if let Some(d) = self.screen_of_layer(fe, layer) {
                        let s = self.screens[d].as_ref().unwrap();
                        fe.set_datastore_u32(&mut vm.mem, PREVIOUS_SCREEN_ID, s.node);
                        if s.state == state::POST_TRANSITION {
                            self.ready = true;
                        }
                    }
                    continue;
                }
            };
            // only an active screen leaves; its layers stop taking input (`0x4e3040`). **[I]** the event of type 7
            // the original posts here has no listener the port knows of.
            let Some(d) = self.screen_of_layer(fe, layer) else {
                continue;
            };
            let s = self.screens[d].as_mut().unwrap();
            if s.state != state::ACTIVE {
                continue;
            }
            s.finish = finish;
            for &l in &s.layers {
                fe.input_blocked[l] = true;
            }
            self.exit(fe, vm, target);
        }
    }

    /// `0x4fbc10` / `0x4fbc70`.
    fn exit(&mut self, fe: &mut FrontEnd, vm: &mut Vm, target: Target) {
        if self.busy {
            self.pending = Some(target);
            return;
        }
        let to = match target {
            Target::Exit(n) => self.flow.take_exit(self.node, n as usize),
            Target::Node(h) => self.flow.node_by_hash(h),
        };
        let Some(to) = to else {
            self.steps.push(Step::Refused(format!("{target:?} from node {} leads nowhere", self.node)));
            return;
        };
        let from = self.node;
        self.from = Some(from);
        self.to = Some(to);
        let (old, new) = (self.flow.nodes[from].hash, self.flow.nodes[to].hash);
        if old == new {
            return;
        }
        (self.sets_in, self.screen_in, self.pre_done, self.ready) = (false, false, false, false);
        self.steps.push(Step::Change { from, to });
        self.changing(fe, vm, old);
        self.load(fe, vm, new);
    }

    /// `0x4fb9c0`.
    fn flow_update(&mut self, fe: &mut FrontEnd, vm: &mut Vm) {
        let node = self.flow.nodes[self.node].hash;
        if self.starting {
            if self.start_load {
                // the loader is idle (`+0x4f`): load the start node's screen
                self.load(fe, vm, node);
                (self.sets_in, self.screen_in, self.start_load) = (false, false, false);
            } else if self.sets_in && self.screen_in {
                self.build(fe, vm, node);
                self.activate(fe, vm, node);
                self.starting = false;
                self.steps.push(Step::Entered(self.node));
            }
            return;
        }
        let (Some(from), Some(to)) = (self.from, self.to) else {
            return;
        };
        let (old, new) = (self.flow.nodes[from].hash, self.flow.nodes[to].hash);
        if self.sets_in && self.screen_in && self.pre_done {
            (self.sets_in, self.screen_in, self.pre_done) = (false, false, false);
            self.build(fe, vm, new);
            self.post_transition(fe, vm, old);
        } else if self.ready {
            self.ready = false;
            self.activate(fe, vm, new);
            self.node = to;
            self.unloaded = false;
            self.busy = true;
            self.leave(fe, vm, old);
            // `+0x60` is never set in the front end: the flow declares the old node unloaded itself
            self.unloaded = true;
        } else if self.unloaded {
            self.unloaded = false;
            self.busy = false;
            self.from = None;
            self.steps.push(Step::Entered(self.node));
            if let Some(t) = self.pending.take() {
                self.exit(fe, vm, t);
            }
        }
    }

    /// "Load" (`0x4fcde0`): make the screen (`0x4fd6f0`) and enter state 1; its load sets are committed against the
    /// last screen's (`0x4fa4b0`).
    fn load(&mut self, fe: &mut FrontEnd, vm: &mut Vm, hash: u32) {
        if self.next.is_some() {
            return;
        }
        let Some(d) = self.def_of(hash) else {
            self.steps.push(Step::Refused(format!("no screen for node {hash:08x}")));
            return;
        };
        self.next = Some(d);
        if self.screens[d].is_none() {
            let def = &self.defs[d];
            let mut load_sets: Vec<u32> = def.loadsets.iter().map(|&(_, slot)| slot).collect();
            for &(_, key) in &def.dynamic_loadsets {
                load_sets.push(fe.datastore_u32(&vm.mem, key).unwrap_or(0));
            }
            self.screens[d] =
                Some(Screen { def: d, node: hash, state: 0, layers: Vec::new(), finish: false, load_sets });
        }
        self.set_state(d, state::LOADING);
        let sets = self.screens[d].as_ref().unwrap().load_sets.clone();
        let load: Vec<u32> = sets.iter().copied().filter(|s| !self.loads.contains(s)).collect();
        let release: Vec<u32> = self.loads.iter().copied().filter(|s| !sets.contains(s)).collect();
        self.loads = sets;
        self.steps.push(Step::LoadSets { load, release });
    }

    /// "Build" (`0x4e2a60`, state 2): each layer is shared (`NoReload`, a live layer of that name) or built, and a
    /// built one's `__Initialise` runs.
    fn build(&mut self, fe: &mut FrontEnd, vm: &mut Vm, hash: u32) {
        let Some(d) = self.def_of(hash).filter(|&d| self.screens[d].is_some()) else {
            return;
        };
        self.set_state(d, state::BUILT);
        // the layers' records register their resources, the records with choices asking their scripts again
        fe.refresh_choices(vm);
        let mut layers = Vec::new();
        for def in self.defs[d].layers.clone() {
            let stem = def.file.rsplit_once('.').map_or(def.file.as_str(), |(s, _)| s);
            let h = name_hash(stem);
            if def.reload == 0 {
                if let Some(l) = fe.layers.iter().position(|l| l.alive && l.hash == h) {
                    fe.layers[l].refs += 1;
                    layers.push(l);
                    continue;
                }
            }
            let Some((script, bytes)) = self.scripts.get(&h) else {
                self.steps.push(Step::Error(format!("{}: no script {}", self.defs[d].name, def.file)));
                continue;
            };
            let script = *script;
            let l = match fe.add_layer(vm, script, bytes) {
                Ok(l) => l,
                Err(e) => {
                    self.steps.push(Step::Error(format!("{}: {e}", def.file)));
                    continue;
                }
            };
            fe.layers[l].screen = Some(d);
            layers.push(l);
            let init = name_hash("__Initialise");
            if vm.scripts[script].export(init).is_some() {
                if let Err(e) = vm.execute(fe, script, init, &[]) {
                    self.steps.push(Step::Error(format!("{}: __Initialise: {e}", def.file)));
                }
            }
        }
        self.screens[d].as_mut().unwrap().layers = layers;
    }

    /// The layers only this screen holds (`+0x4c` = 1): the per-object passes run on those.
    fn own_layers(&self, fe: &FrontEnd, d: usize) -> Vec<usize> {
        let s = self.screens[d].as_ref().unwrap();
        s.layers.iter().copied().filter(|&l| fe.layers[l].alive && fe.layers[l].refs == 1).collect()
    }

    /// "Current" (`0x4fcef0`, state 3, `0x4e2d10`): the helper's screens move on and the screen constructs.
    fn activate(&mut self, fe: &mut FrontEnd, vm: &mut Vm, hash: u32) {
        let Some(d) = self.def_of(hash).filter(|&d| self.screens[d].is_some()) else {
            return;
        };
        self.previous = self.current;
        self.current = self.next.take();
        self.set_state(d, state::ACTIVE);
        for l in self.own_layers(fe, d) {
            if let Err(e) = fe.construct(vm, l) {
                self.steps.push(Step::Error(format!("{}: construct: {e}", fe.layers[l].name)));
            }
        }
    }

    /// Run event `e`'s handlers on the screen's own layers, stopping at an error (`0x4d9840`, `0x4d9900`).
    fn handlers(&mut self, fe: &mut FrontEnd, vm: &mut Vm, d: usize, e: usize, what: &str) {
        for l in self.own_layers(fe, d) {
            if let Err(err) = fe.dispatch(vm, l, e) {
                self.steps.push(Step::Error(format!("{}: {what}: {err}", fe.layers[l].name)));
                return;
            }
        }
    }

    /// "Node changing" (`0x4fcf40`, state 4, `0x4e2d70`): the old screen's destruct handlers, then its
    /// pre-transition handlers. **[I]** Where the original runs the objects' destruct handlers (`+0x88`) is not found
    /// (the layers' teardowns, `0x4e3090` and `0x4d9e70`, call the objects' own destroy methods). They cannot run at
    /// the leave, after the next screen's construct: the save screens turn the memory card off in theirs
    /// (`Memcard_EnableMemoryCards(0)`) and on in their constructs, so a save step's construct would be undone by the
    /// last step's destruct and the save never run (as the port's did, 2026-10-24). Here they run as the screen
    /// starts to leave.
    fn changing(&mut self, fe: &mut FrontEnd, vm: &mut Vm, hash: u32) {
        let Some(d) = self.def_of(hash).filter(|&d| self.screens[d].is_some()) else {
            return;
        };
        self.set_state(d, state::PRE_TRANSITION);
        self.handlers(fe, vm, d, event::DESTRUCT, "destruct");
        self.handlers(fe, vm, d, event::PRE_TRANSITION, "pre-transition");
    }

    /// "Post-transition" (`0x4fcf70`, state 6, `0x4e2dc0`).
    fn post_transition(&mut self, fe: &mut FrontEnd, vm: &mut Vm, hash: u32) {
        let Some(d) = self.def_of(hash).filter(|&d| self.screens[d].is_some()) else {
            return;
        };
        self.set_state(d, state::POST_TRANSITION);
        self.handlers(fe, vm, d, event::POST_TRANSITION, "post-transition");
    }

    /// "Leaves" (`0x4fcfa0`, state 7, then `0x4fd8a0`): the screen's own layers are destroyed, every layer released
    /// (their destruct handlers ran as the screen began to change, [`Manager::changing`]).
    fn leave(&mut self, fe: &mut FrontEnd, _vm: &mut Vm, hash: u32) {
        self.previous = None;
        let Some(d) = self.def_of(hash).filter(|&d| self.screens[d].is_some()) else {
            return;
        };
        self.set_state(d, state::LEFT);
        for l in self.screens[d].take().unwrap().layers {
            let layer = &mut fe.layers[l];
            layer.refs = layer.refs.saturating_sub(1);
            if layer.refs == 0 {
                layer.alive = false;
                // its text objects let go of their strings (`0x4e0b20`), so a later screen formats them afresh
                fe.release_texts(l);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwc_formats::flow::{Group, Layer as LayerDef, Link, Node};

    fn node(name: &str, first_link: u32, n_links: u32) -> Node {
        Node {
            hash: name_hash(name),
            str_hashes: [0; 3],
            id: 0,
            first_link,
            n_links,
            name: Some(name.into()),
            strings: None,
            group: 0,
        }
    }

    fn def(name: &str, layers: &[(&str, u8)], sets: &[u32]) -> ScreenDef {
        ScreenDef {
            name: name.into(),
            hash: name_hash(name),
            loadsets: sets.iter().map(|&s| (format!("set{s}"), s)).collect(),
            dynamic_loadsets: vec![],
            layers: layers
                .iter()
                .map(|&(f, reload)| LayerDef { priority: 0, group: "G".into(), file: f.into(), reload })
                .collect(),
            ignored: vec![],
        }
    }

    /// Nodes A -> B, B -> C or A, C a dead end. A and B share `Shared.vso` (`NoReload`).
    fn setup() -> (Manager, FrontEnd, Vm) {
        let flow = Flow {
            version: 1,
            has_names: 1,
            roots: vec![0],
            groups: vec![Group { hash: name_hash("Main"), entry: 0, n_children: 3, name: None, parent: None }],
            nodes: vec![node("A", 0, 1), node("B", 1, 2), node("C", 3, 0)],
            links: vec![Link { target: 1, flags: 0 }, Link { target: 2, flags: 0 }, Link { target: 0, flags: 0 }],
        };
        let defs = vec![
            def("A", &[("Shared.vso", 0), ("A.vso", 1)], &[1, 2]),
            def("B", &[("Shared.vso", 0), ("B.vso", 1)], &[2, 3]),
            def("C", &[("C.vso", 1)], &[]),
        ];
        let (mut vm, fe) = (Vm::new(), FrontEnd::new());
        let mut scripts = Vec::new();
        for name in ["Shared", "A", "B", "C"] {
            let b = crate::tests::empty_script(name_hash(name));
            let i = vm.load(&format!("{name}.vso"), &b).unwrap();
            scripts.push((i, b));
        }
        vm.link(&fe);
        let mut m = Manager::new(flow, defs, &vm, scripts);
        m.start(m.boot_node().unwrap());
        (m, fe, vm)
    }

    fn entered(m: &Manager) -> Vec<usize> {
        m.steps.iter().filter_map(|s| if let Step::Entered(n) = s { Some(*n) } else { None }).collect()
    }

    fn layer(fe: &FrontEnd, name: &str) -> usize {
        fe.layers.iter().rposition(|l| l.name == format!("{name}.vso")).unwrap()
    }

    fn state(m: &Manager, name: &str) -> Option<u8> {
        m.screens[m.def_of(name_hash(name))?].as_ref().map(|s| s.state)
    }

    #[test]
    fn boots_into_the_first_node() {
        let (mut m, mut fe, mut vm) = setup();
        m.frame(&mut fe, &mut vm); // load requested
        assert_eq!(state(&m, "A"), Some(state::LOADING));
        m.frame(&mut fe, &mut vm); // in, then built and current
        assert_eq!((entered(&m), state(&m, "A"), m.current), (vec![0], Some(state::ACTIVE), Some(0)));
        let live: Vec<&str> = fe.layers.iter().filter(|l| l.alive).map(|l| l.name.as_str()).collect();
        assert_eq!(live, ["Shared.vso", "A.vso"]);
        assert!(m.steps.contains(&Step::LoadSets { load: vec![1, 2], release: vec![] }));
    }

    #[test]
    fn a_transition_driven_by_the_scripts() {
        let (mut m, mut fe, mut vm) = setup();
        for _ in 0..2 {
            m.frame(&mut fe, &mut vm);
        }
        let (a, shared) = (layer(&fe, "A"), layer(&fe, "Shared"));
        fe.requests.push(Request::Exit { layer: a, exit: 0, finish: false });
        m.frame(&mut fe, &mut vm);
        // A starts its pre-transition, B is made and loads; A's layers take no input
        assert_eq!((state(&m, "A"), state(&m, "B")), (Some(state::PRE_TRANSITION), Some(state::LOADING)));
        assert!(fe.input_blocked[a] && fe.input_blocked[shared]);
        assert!(m.steps.contains(&Step::LoadSets { load: vec![3], release: vec![1] }));
        // nothing moves until the scripts say the pre-transition is done
        for _ in 0..5 {
            m.frame(&mut fe, &mut vm);
        }
        assert_eq!(state(&m, "B"), Some(state::LOADING));
        fe.requests.push(Request::PreTransitionDone { layer: a });
        m.frame(&mut fe, &mut vm);
        // B is built (sharing A's NoReload layer), A runs its post-transition
        assert_eq!((state(&m, "A"), state(&m, "B")), (Some(state::POST_TRANSITION), Some(state::BUILT)));
        assert_eq!((fe.layers[shared].refs, layer(&fe, "Shared")), (2, shared));
        fe.requests.push(Request::ReadyToExit { layer: a });
        m.frame(&mut fe, &mut vm);
        // B is current, A is gone and its own layer with it; the shared layer stays
        assert_eq!((state(&m, "A"), state(&m, "B"), m.current), (None, Some(state::ACTIVE), Some(1)));
        assert!(!fe.layers[a].alive && fe.layers[shared].alive && fe.layers[shared].refs == 1);
        m.frame(&mut fe, &mut vm);
        assert_eq!(entered(&m), vec![0, 1]);
    }

    #[test]
    fn finish_runs_the_transitions_alone() {
        let (mut m, mut fe, mut vm) = setup();
        for _ in 0..2 {
            m.frame(&mut fe, &mut vm);
        }
        fe.requests.push(Request::Exit { layer: layer(&fe, "A"), exit: 0, finish: true });
        for _ in 0..6 {
            m.frame(&mut fe, &mut vm);
        }
        assert_eq!(entered(&m), vec![0, 1]);
        // then B jumps by hash to C: nothing is shared, so the Shared layer goes
        let shared = layer(&fe, "Shared");
        fe.requests.push(Request::JumpToNode { layer: layer(&fe, "B"), hash: name_hash("C"), finish: true });
        for _ in 0..6 {
            m.frame(&mut fe, &mut vm);
        }
        assert_eq!(entered(&m), vec![0, 1, 2]);
        assert!(!fe.layers[shared].alive);
    }

    #[test]
    fn requests_while_unloading_wait_and_bad_exits_are_refused() {
        let (mut m, mut fe, mut vm) = setup();
        for _ in 0..2 {
            m.frame(&mut fe, &mut vm);
        }
        fe.requests.push(Request::Exit { layer: layer(&fe, "A"), exit: 0, finish: true });
        // run until B is current but the old screen is still unloading (the flow is busy)
        while m.current != Some(1) {
            m.frame(&mut fe, &mut vm);
        }
        assert!(m.busy);
        fe.requests.push(Request::ExitToNode { layer: layer(&fe, "B"), hash: name_hash("A"), finish: true });
        for _ in 0..8 {
            m.frame(&mut fe, &mut vm);
        }
        assert_eq!(entered(&m), vec![0, 1, 0]);
        // only an active screen's requests count, and an exit that leads nowhere is refused
        fe.requests.push(Request::Exit { layer: layer(&fe, "A"), exit: 5, finish: true });
        m.frame(&mut fe, &mut vm);
        assert!(m.steps.iter().any(|s| matches!(s, Step::Refused(_))));
        fe.requests.push(Request::ExitToNode { layer: layer(&fe, "A"), hash: name_hash("C"), finish: true });
        m.frame(&mut fe, &mut vm);
        assert_eq!(m.steps.iter().filter(|s| matches!(s, Step::Refused(_))).count(), 2);
    }
}
