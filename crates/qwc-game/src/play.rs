//! `qwc-game play [TEAM0 TEAM1] [--stadium NAME] [--watch] [--time SECS] [--out FILE.png]`: a match in a window,
//! the simulation (`qwc_sim::world::Match`) at the game's 24 Hz and the 3D world (`crate::arena`) blended between
//! ticks. The teams are folders of `teams/` (default `England` against `France`), each with its `team_1.cli`
//! (**[I]** the difficulty's file, DataStore `0xed986418`, is not traced); the stadium a folder of `stadium/`
//! (default `england`). You play the first team (`qwc_sim::human`, on the keys of `crate::controls`, the sticks
//! turned by the camera's heading) unless `--watch`. With `--out` the match runs `--time` seconds (default 10)
//! without a window, both teams the AI's, and that moment is drawn into the PNG; without `--out`, `--time` closes the
//! window then. In the window: P pauses, F12 saves the frame (to `--out` when given), Escape quits.

use std::sync::Arc;
use std::time::{Duration, Instant};

use qwc_assets::Vfs;
use qwc_render::overlay::{Overlay, OverlayTarget, OVERLAY_FORMAT};
use qwc_render::{wgpu, Gpu, Renderer};
use qwc_sim::Match;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::arena::{Arena, MatchSpec};
use crate::controls::{self, Controls};
use crate::fe::write_png;
use crate::{fail, Args};

/// The game's tick.
const TICK: Duration = Duration::from_nanos(1_000_000_000 / 24);

/// The teams and the stadium `play` was given (or the online match's, `crate::online`).
pub struct Setup {
    spec: MatchSpec,
    /// The remastered textures (`--textures`) the world draws, and how it is lit (`--look`).
    textures: Option<qwc_assets::remaster::Remastered>,
    look: qwc_render::Look,
}

/// Team `name`'s folder as the disc spells it (`teams/<folder>/`, any case), if there is one.
pub(crate) fn team_folder(vfs: &Vfs, name: &str) -> Option<String> {
    let want = format!("teams/{}/", name.to_ascii_lowercase());
    vfs.with_extension("fpm")
        .find(|p| p.to_ascii_lowercase().starts_with(&want))
        .map(|p| p.split('/').nth(1).unwrap_or(name).to_string())
}

impl Setup {
    fn from(args: &Args, vfs: &Vfs) -> Setup {
        let names: Vec<&str> = args.models.iter().map(String::as_str).collect();
        let teams = match names[..] {
            [] => ["England", "France"],
            [a, b] => [a, b],
            _ => crate::usage(),
        };
        let teams = teams.map(|t| team_folder(vfs, t).unwrap_or_else(|| fail(format!("no team folder teams/{t}"))));
        let stadium = args.stadium.clone().unwrap_or_else(|| "england".into());
        Setup::new(args, vfs, teams, stadium)
    }

    /// `teams` (folders as the disc spells them) at `stadium`.
    pub fn new(args: &Args, vfs: &Vfs, teams: [String; 2], stadium: String) -> Setup {
        let music = crate::audio::match_score(&stadium).into();
        Setup {
            spec: MatchSpec {
                stadium,
                hoops: "hoopnorm".into(),
                teams,
                skill: 1,
                music,
                sounds: None,
                desc: "quidd".into(),
            },
            textures: crate::fe::remastered_textures(args, vfs),
            look: args.look,
        }
    }

    /// The match, `rand()` from `seed`, with the moves a finished save has: the dodge, the Bludger and the special
    /// move (combos and the uber move stay off, as for a quick match; the front end's matches read the save).
    pub fn start(&self, vfs: &Vfs, seed: u32) -> Match {
        let mut m = self.spec.start(vfs, seed);
        m.power.unlocked = [true, true, true, false];
        m
    }

    fn title(&self, m: &Match) -> String {
        title(&self.spec, m)
    }
}

/// The window's title: the teams and the score.
pub(crate) fn title(spec: &MatchSpec, m: &Match) -> String {
    let (a, b) = (&spec.teams[0], &spec.teams[1]);
    let (sa, sb) = (m.teams[0].score, m.teams[1].score);
    let end = if m.finished { " (full time)" } else { "" };
    format!("Quidditch World Cup: {a} {sa} - {sb} {b}{end}")
}

pub fn play(args: &Args, vfs: &'static Vfs) {
    let setup = Setup::from(args, vfs);
    let spec = &setup.spec;
    eprintln!("{} against {} at {}", spec.teams[0], spec.teams[1], spec.stadium);
    if let Some(out) = &args.out {
        // headless: the match `--time` seconds on, drawn once
        let secs = if args.time > 0.0 { args.time } else { 10.0 };
        let mut m = setup.start(vfs, 1);
        if std::env::var_os("QWC_SEEKER_GAME").is_some() {
            m.start_seeker_game();
        }
        // `QWC_TWO_PLAYERS`: both teams a human's (idle), the split screen (checks)
        if std::env::var_os("QWC_TWO_PLAYERS").is_some() {
            m.set_human(0, true);
            m.set_human(1, true);
        }
        let gpu = Gpu::headless().unwrap_or_else(|e| fail(e));
        let mut renderer = Renderer::new(&gpu.device, &gpu.queue, OVERLAY_FORMAT);
        renderer.shaders = crate::packs::shaders();
        renderer.remastered = setup.textures.clone();
        crate::stadium::set_look(&mut renderer, setup.look, &setup.spec.stadium);
        let mut arena = Arena::load(&gpu, &mut renderer, vfs, &setup.spec, &m);
        let ticks = (secs * 24.0) as u32;
        let mut restart = crate::restart::Restart::default();
        for _ in 0..ticks {
            if !restart.holds() {
                m.step();
                arena.ticked(&m, m.tick as f64 / 24.0);
            }
            restart.tick(&m, false);
        }
        let camera = arena.frame(&gpu, &renderer, &m, 1.0, ticks as f64 / 24.0);
        let target = OverlayTarget::new(&gpu.device, args.size);
        let depth = qwc_render::depth_view(&gpu.device, args.size);
        arena.draw_world(&gpu, &mut renderer, &target.view, &depth, args.size, &m, &camera, 1.0);
        let mut overlay = Overlay::new(&gpu.device, OVERLAY_FORMAT);
        overlay.upload(&gpu, 0, 1, 1, &[255; 4]);
        let view = crate::hud::View { teams: [&spec.teams[0], &spec.teams[1]], rings: &[], challenge: None };
        let fit = qwc_feck::draw::Fit { span: overlay.span() };
        let mut quads: Vec<_> = crate::restart::black(fit, restart.darkness()).into_iter().collect();
        let mut hud = crate::hud::Hud::default().quads(&mut overlay, &gpu, vfs, &m, &view, args.size, fit);
        crate::restart::fade(&mut hud, restart.hud());
        quads.extend(hud);
        overlay.draw(&gpu, &target.view, None, &quads);
        write_png(out, args.size, target.read_rgba(&gpu).unwrap_or_else(|e| fail(e)));
        eprintln!("{}; wrote {}", setup.title(&m), out.display());
        return;
    }
    let mut m = setup.start(vfs, 1);
    if !args.watch {
        m.set_human(0, true);
    }
    if std::env::var_os("QWC_SEEKER_GAME").is_some() {
        m.start_seeker_game();
    }
    run(args, vfs, setup, m, None);
}

/// The match `m` in a window; `online`, against a player on another machine (`crate::online`), this machine's
/// player on its seat's team.
pub(crate) fn run(args: &Args, vfs: &'static Vfs, setup: Setup, m: Match, online: Option<crate::online::Online>) {
    let viewer = online.as_ref().map_or(0, |o| o.seat);
    let heading = [m.teams[viewer].attack_dir(), 0.0];
    let mut audio = crate::audio::Audio::open(vfs);
    if let Some(a) = &mut audio {
        a.set_volumes(&args.sound);
        let folder = crate::matchaudio::stadium_folder(vfs, &setup.spec.stadium);
        a.match_started(&setup.spec.music, folder);
        // the commentary, with an exhibition's crowd cutscene record for its colour commentator and referee
        let spec = &setup.spec;
        let tables = crate::ccs::Tables::load(vfs);
        let codes = [crate::ccs::team_code(&spec.teams[0]), crate::ccs::team_code(&spec.teams[1])];
        let location = crate::ccs::location_of_folder(&spec.stadium);
        let record = tables.as_ref().and_then(|t| Some((t, &t.records[t.find("E", codes, location)?])));
        let human = [m.teams[0].human, m.teams[1].human];
        a.commentary_started(crate::commentary::setup(&spec.teams, &spec.stadium, record, human));
        if online.is_some() {
            a.set_viewer(viewer);
        }
    }
    let look_file = crate::looks::Watch::new(&setup.spec.stadium);
    let mut app = Play {
        vfs,
        setup,
        m,
        size: args.size,
        window_mode: args.window,
        vsync: args.vsync,
        compress: crate::compression(args),
        window: None,
        last: Instant::now(),
        behind: Duration::ZERO,
        time: 0.0,
        paused: false,
        shoot: false,
        out: None,
        controls: Controls::saved(),
        pads: crate::gamepad::Gamepads::new(),
        start_held: false,
        heading,
        quit_at: (args.time > 0.0).then(|| Instant::now() + Duration::from_secs_f64(args.time)),
        audio,
        music: Default::default(),
        look_file,
        restart: Default::default(),
        online,
        still: false,
    };
    let el = EventLoop::new().unwrap_or_else(|e| fail(e));
    el.run_app(&mut app).unwrap_or_else(|e| fail(e));
    if let Some(o) = &mut app.online {
        o.leave();
    }
}

struct Surface {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    renderer: Renderer,
    depth: wgpu::TextureView,
    arena: Arena,
    /// The HUD (`crate::hud`) over the match.
    overlay: Overlay,
    hud: crate::hud::Hud,
}

struct Play<'a> {
    vfs: &'a Vfs,
    setup: Setup,
    m: Match,
    size: (u32, u32),
    /// The window's mode and vsync (the player's settings).
    window_mode: qwc_settings::WindowMode,
    vsync: bool,
    /// The packs' big pictures compressed (`crate::compression`).
    compress: Option<Option<std::path::PathBuf>>,
    window: Option<Surface>,
    last: Instant,
    behind: Duration,
    /// Match time: ticks run over 24.
    time: f64,
    paused: bool,
    shoot: bool,
    out: Option<std::path::PathBuf>,
    /// The keys held, and the camera's heading across the ground (the sticks turn by it, team `+0xfc`).
    controls: Controls,
    /// The gamepads (`crate::gamepad`): the first is the player's, its Start pauses as P does.
    pads: crate::gamepad::Gamepads,
    start_held: bool,
    heading: [f64; 2],
    /// `--time` without `--out`: the window closes then.
    quit_at: Option<Instant>,
    /// The sound (`crate::audio`), and the match's events already given to the music.
    audio: Option<crate::audio::Audio>,
    music: crate::audio::MusicRouter,
    /// The stadium's look file (`crate::looks`), read again when it is saved.
    look_file: crate::looks::Watch,
    /// The restart after a goal (`crate::restart`: the fades, the HUD's, the hold).
    restart: crate::restart::Restart,
    /// Online (`crate::online`): the other player's machine runs the same match, each frame once both inputs are in.
    online: Option<crate::online::Online>,
    /// The last frame run left the match where it was (both online games paused, or the restart holding play): the
    /// picture stays on the last tick instead of blending towards it again each frame.
    still: bool,
}

impl Play<'_> {
    /// The window's title: the score, and online the round trip.
    fn title(&self) -> String {
        let mut t = self.setup.title(&self.m);
        if let Some(o) = &self.online {
            let rtt = o.rtt().map_or_else(String::new, |r| format!(", {} ms", r.as_millis()));
            t += &format!("  (online{rtt})");
        }
        t
    }

    /// P or the pad's Start: online, both games pause from the frame both see it; else this one.
    fn toggle_pause(&mut self) {
        match &mut self.online {
            Some(o) => o.ask_pause(),
            None => self.paused = !self.paused,
        }
    }

    /// The pads, the pause, and the ticks due since the last call (online, the frames whose inputs are in);
    /// answers how many ran.
    fn advance(&mut self) -> usize {
        self.pads.poll();
        self.pads.take_pressed();
        let start = self.pads.menu_words(0)[qwc_feck::pad::PadWord::Button200 as usize];
        if start && !std::mem::replace(&mut self.start_held, start) {
            self.toggle_pause();
        }
        self.start_held = start;
        if let Some(o) = &mut self.online {
            o.poll();
        }
        let now = Instant::now();
        if !self.paused {
            self.behind += now - self.last;
        }
        self.last = now;
        let Some(s) = &mut self.window else { return 0 };
        let mut ran = 0;
        let tick = self.online.as_ref().map_or(TICK, |o| o.tick_length());
        while self.behind >= tick && ran < 3 {
            let mut input = None;
            // once a tick: the presses latched since the last one
            self.controls.gamepad = self.pads.state(0);
            let done = if let Some(o) = &mut self.online {
                // online: this player's input goes to the session, and the frame runs once both players' are in, as
                // on the other machine (`Online::run`)
                let Some(frame) = o.frame(&mut self.controls, self.heading, self.m.combo_enabled) else {
                    // no debt piles up while waiting
                    self.behind = self.behind.min(tick);
                    break;
                };
                input = Some(frame.inputs[o.seat].input);
                o.run(&mut self.m, &mut self.restart, &frame)
            } else {
                if self.m.teams[0].human {
                    self.controls.tick();
                    let i = qwc_sim::human::Input::from_pad(&self.controls.pad, self.heading, self.m.combo_enabled);
                    self.m.set_input(0, i);
                    self.m.set_seeker_pad(0, qwc_sim::race::SeekerPad::from_pad(&self.controls.pad));
                    input = Some(i);
                }
                let stepped = !self.restart.holds();
                if stepped {
                    self.m.step();
                }
                let cue = self.restart.tick(&self.m, false);
                crate::online::Ran { paused: false, stepped, cue }
            };
            self.still = !done.stepped;
            if done.stepped {
                self.time = self.m.tick as f64 / 24.0;
                s.arena.ticked(&self.m, self.time);
            }
            if !done.paused {
                let cue = done.cue;
                if let Some(a) = &mut self.audio {
                    for e in self.music.tick(&self.m) {
                        a.match_event(e);
                    }
                    if cue == Some(crate::restart::Cue::HudStart) {
                        a.hud_start();
                    }
                    a.match_tick(&self.m, input.as_ref());
                }
            }
            self.behind -= tick;
            ran += 1;
        }
        if self.behind >= tick {
            self.behind = Duration::ZERO;
        }
        // the score keeps its next segment queued, paused or not
        if let Some(a) = &mut self.audio {
            a.tick(&[], false);
        }
        if ran > 0 {
            let title = self.title();
            if let Some(s) = &mut self.window {
                s.window.set_title(&title);
            }
        }
        ran
    }

    fn redraw(&mut self) {
        self.advance();
        let Some(s) = &mut self.window else { return };
        if let Some(look) = self.look_file.poll() {
            s.renderer.settings = look;
            crate::stadium::apply_env(&mut s.renderer);
            s.arena.bake_probes(&s.gpu, &mut s.renderer, &self.m);
            eprintln!("the stadium's look read again");
        }
        let frame = match s.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => {
                s.surface.configure(&s.gpu.device, &s.config);
                s.window.request_redraw();
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let size = (s.config.width, s.config.height);
        let alpha = self.behind.as_secs_f64() / TICK.as_secs_f64();
        // while play holds, or both online games are paused (frames still run, the match does not move), the models
        // and the camera stay where the last tick put them: blending again from the tick before each frame run would
        // flick between the two 24 times a second
        let alpha = if self.restart.holds() || self.still { 1.0 } else { alpha };
        let camera = s.arena.frame(&s.gpu, &s.renderer, &self.m, alpha, self.time + alpha / 24.0);
        // the sticks turn by the follow camera's heading, whatever shot shows
        let ahead = s.arena.ahead().unwrap_or(camera.target - camera.eye);
        let flat = qwc_render::glam::Vec2::new(ahead.x, ahead.z).normalize_or_zero();
        if flat != qwc_render::glam::Vec2::ZERO {
            self.heading = [f64::from(flat.x), f64::from(flat.y)];
        }
        s.arena.draw_world(&s.gpu, &mut s.renderer, &view, &s.depth, size, &self.m, &camera, alpha);
        let teams = [self.setup.spec.teams[0].as_str(), self.setup.spec.teams[1].as_str()];
        let hud = crate::hud::View { teams, rings: &[], challenge: None };
        let fit = qwc_feck::draw::Fit { span: s.overlay.span() };
        let mut quads: Vec<_> = crate::restart::black(fit, self.restart.darkness()).into_iter().collect();
        let mut hud = s.hud.quads(&mut s.overlay, &s.gpu, self.vfs, &self.m, &hud, size, fit);
        crate::restart::fade(&mut hud, self.restart.hud());
        quads.extend(hud);
        // online: the other player waited for, the pause, the session's end
        if let Some(lines) = self.online.as_ref().and_then(|o| o.message()) {
            let o = self.online.as_mut().unwrap();
            quads.extend(o.banner.quads(&mut s.overlay, &s.gpu, &lines, size, fit));
        }
        s.overlay.draw(&s.gpu, &view, None, &quads);
        if std::mem::take(&mut self.shoot) {
            match qwc_render::read_texture(&s.gpu, &frame.texture, size) {
                Ok(mut px) => {
                    if matches!(s.config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb)
                    {
                        px.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p.swap(0, 2));
                    }
                    let path = self.out.clone().unwrap_or_else(|| "match.png".into());
                    write_png(&path, size, px);
                    eprintln!("saved {}", path.display());
                }
                Err(e) => eprintln!("screenshot: {e}"),
            }
        }
        s.gpu.queue.present(frame);
        s.window.request_redraw();
    }
}

impl ApplicationHandler for Play<'_> {
    /// Online, the match goes on while the window is not drawn (minimised): its frames still run and its input
    /// still goes out, so the other player is neither held up for long nor cut off.
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.online.is_some() && self.window.is_some() {
            self.advance();
            el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(5)));
        }
    }

    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = crate::display::attributes(el, &self.title(), self.size, self.window_mode);
        let window = Arc::new(el.create_window(attrs).unwrap_or_else(|e| fail(e)));
        let instance = wgpu::Instance::new(qwc_render::primary_backends(
            wgpu::InstanceDescriptor::new_with_display_handle(Box::new(el.owned_display_handle())),
        ));
        let surface = instance.create_surface(window.clone()).unwrap_or_else(|e| fail(e));
        let gpu = Gpu::new(instance, Some(&surface)).unwrap_or_else(|e| fail(e));
        eprintln!("adapter: {}", gpu.adapter.get_info().name);
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .unwrap_or_else(|| fail("surface not supported by the adapter"));
        // the stored values, as the original's 8-bit screen and the front end's window
        let linear = surface.get_capabilities(&gpu.adapter).formats.into_iter().find(|f| !f.is_srgb());
        config.format = linear.unwrap_or(config.format);
        if surface.get_capabilities(&gpu.adapter).alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            config.alpha_mode = wgpu::CompositeAlphaMode::Opaque;
        }
        if surface.get_capabilities(&gpu.adapter).usages.contains(wgpu::TextureUsages::COPY_SRC) {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        config.present_mode = crate::display::present_mode(self.vsync, &surface.get_capabilities(&gpu.adapter));
        surface.configure(&gpu.device, &config);
        let mut renderer = Renderer::new(&gpu.device, &gpu.queue, config.format);
        renderer.shaders = crate::packs::shaders();
        renderer.compress = self.compress.clone();
        renderer.remastered = self.setup.textures.clone();
        crate::stadium::set_look(&mut renderer, self.setup.look, &self.setup.spec.stadium);
        let mut arena = Arena::load(&gpu, &mut renderer, self.vfs, &self.setup.spec, &self.m);
        if let Some(o) = &mut self.online {
            // the other machine shows the other player's view
            arena.viewer = Some(o.seat);
            // the handshake's end, now this side has loaded: the other may still be loading
            window.set_title("Quidditch World Cup: waiting for the other player...");
            if let Err(e) = o.connect() {
                crate::online::status(&format!("error {e}"));
                eprintln!("online: {e}");
                el.exit();
                return;
            }
            window.set_title(&self.title());
        }
        let depth = qwc_render::depth_view(&gpu.device, (config.width, config.height));
        let mut overlay = Overlay::new(&gpu.device, config.format);
        // a white texel for the restart's black
        overlay.upload(&gpu, 0, 1, 1, &[255; 4]);
        let hud = crate::hud::Hud::default();
        self.window = Some(Surface { window, surface, config, gpu, renderer, depth, arena, overlay, hud });
        self.last = Instant::now();
        self.behind = Duration::ZERO;
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                if let Some(o) = &mut self.online {
                    o.leave();
                }
                el.exit()
            }
            WindowEvent::Resized(size) => {
                if let Some(s) = &mut self.window {
                    s.config.width = size.width.max(1);
                    s.config.height = size.height.max(1);
                    s.surface.configure(&s.gpu.device, &s.config);
                    s.depth = qwc_render::depth_view(&s.gpu.device, (s.config.width, s.config.height));
                }
            }
            WindowEvent::RedrawRequested => {
                if self.quit_at.is_some_and(|t| Instant::now() >= t) {
                    eprintln!("{}", self.title());
                    el.exit();
                    return;
                }
                self.redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let down = event.state == ElementState::Pressed;
                let PhysicalKey::Code(code) = event.physical_key else { return };
                match code {
                    KeyCode::Escape if down => {
                        if let Some(o) = &mut self.online {
                            o.leave();
                        }
                        el.exit()
                    }
                    KeyCode::KeyP if down && !event.repeat => self.toggle_pause(),
                    KeyCode::F12 if down => self.shoot = true,
                    _ => {
                        if let Some(id) = controls::key_id(code) {
                            self.controls.set(id, down);
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(id) = controls::mouse_id(button) {
                    self.controls.set(id, state == ElementState::Pressed);
                }
            }
            _ => {}
        }
    }
}
