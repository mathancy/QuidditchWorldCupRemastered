//! `qwc-game frontend [NODE|boot] [--size WxH] [--keys SECS:KEY,...] [--time SECS] [--out FILE.png]`: the front
//! end live in a window. F12 saves the frame shown (to `--out`, numbered, or `frontend-N.png`). For runs without
//! anyone at the keys, `--keys` presses keys at those times (seconds since the window opened, movies included;
//! each held for a tick: `6:Enter,8.5:Down`, names as in [`key_by_name`], `F12` saves the frame) and `--time`
//! closes the window then, saving the last frame when `--out` is given.
//!
//! The flow runs at the game's 24 Hz: each drawn frame runs the ticks due since the last, at most three, and when
//! still behind it lets the time go (the main loop `0x52f7f0` does both). Each tick fills pad 0's words from the
//! keys held, runs the screen manager's frame and builds the draw list; every drawn frame paints the last list
//! ([`crate::fe::Painter`]). Between ticks the frame drawn is the last two lists blended (`DrawList::blend`,
//! `0x4e8310`; the camera as the render camera blends it, `0x4bed60`). **[I]** the time it is drawn at is a tick
//! behind the flow, so the blend runs from the last list but one to the last (the lists' time stamps are not
//! traced). **[I]** the keys (the PC's own mapping through `PadManager` is not traced): the arrows move, Enter
//! chooses, Escape or Backspace cancels, Tab is button 4 (the Inventory prompts) and Space button 8.
//!
//! Movies: `General_PlayMovie` plays its movie to the end inside the native (`0x4d3fd0`, the player `0x4d23c0`),
//! so the screens stop. Here the native's request ends the tick; the window then shows the movie (pictures by
//! the clock at 25 a second, the sound through the mixer) and runs no ticks until it ends. The script's flag lets
//! a newly pressed button stop it (the player's callback `0x4d1e80` answers the controllers' `+0xe9`, the edge of
//! the first word of `[0x1043770]`): the EA and WB movies pass 0, the intro 1. **[I]** any key but F12 is that
//! button, and keys pressed during a movie do not reach the pads.
//!
//! The remastered movies (the packs' `movies/*.mp4`, `--remastered DIR`) play in place of the disc's when there is
//! one by the same name, keeping their 16:9 shape ([`qwc_video`]); `--original-movies` plays the disc's, and
//! `--openh264` decodes them with OpenH264 where the system has its own decoder.

use std::sync::Arc;
use std::time::{Duration, Instant};

use qwc_assets::Bytes;
use qwc_assets::Vfs;
use qwc_feck::draw::DrawList;
use qwc_feck::frontend::EngineEvent;
use qwc_feck::pad::PadWord;
use qwc_render::{wgpu, Gpu};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::fe::{font_files, report, start_flow, write_png, Disc, Flow, MatchView, MoviePicture, Painter};
use crate::fematch::MatchRun;
use crate::{fail, Args};
use qwc_formats::tqi::{Movie, FPS};
use qwc_video::HdMovie;

/// The game's tick.
const TICK: Duration = Duration::from_nanos(1_000_000_000 / 24);

/// The pad word a key holds down.
fn word(key: KeyCode) -> Option<PadWord> {
    Some(match key {
        KeyCode::ArrowUp => PadWord::Up,
        KeyCode::ArrowDown => PadWord::Down,
        KeyCode::ArrowLeft => PadWord::Left,
        KeyCode::ArrowRight => PadWord::Right,
        KeyCode::Enter | KeyCode::NumpadEnter => PadWord::Choose,
        KeyCode::Escape | KeyCode::Backspace => PadWord::Cancel,
        KeyCode::Tab => PadWord::Button4,
        KeyCode::Space => PadWord::Button8,
        // **[I]** front-end button `0x200`, the match's pause (`Match.vso`), as a pad's Start
        KeyCode::KeyP => PadWord::Button200,
        _ => return None,
    })
}

/// A key by the name `--keys` uses.
fn key_by_name(name: &str) -> Option<KeyCode> {
    Some(match name.to_ascii_lowercase().as_str() {
        "up" => KeyCode::ArrowUp,
        "down" => KeyCode::ArrowDown,
        "left" => KeyCode::ArrowLeft,
        "right" => KeyCode::ArrowRight,
        "enter" => KeyCode::Enter,
        "escape" | "esc" => KeyCode::Escape,
        "backspace" => KeyCode::Backspace,
        "tab" => KeyCode::Tab,
        "space" => KeyCode::Space,
        "j" => KeyCode::KeyJ,
        "k" => KeyCode::KeyK,
        "h" => KeyCode::KeyH,
        "u" => KeyCode::KeyU,
        "w" => KeyCode::KeyW,
        "a" => KeyCode::KeyA,
        "s" => KeyCode::KeyS,
        "d" => KeyCode::KeyD,
        "f12" => KeyCode::F12,
        _ => return None,
    })
}

/// `--keys`: (seconds, key), in time order.
fn parse_keys(s: &str) -> Vec<(f64, KeyCode)> {
    let mut keys: Vec<(f64, KeyCode)> = s
        .split(',')
        .filter(|x| !x.trim().is_empty())
        .map(|x| {
            let (t, k) = x.split_once(':').unwrap_or_else(|| fail(format!("--keys: {x} is not SECS:KEY")));
            let t = t.trim().parse().unwrap_or_else(|_| fail(format!("--keys: bad time {t}")));
            (t, key_by_name(k.trim()).unwrap_or_else(|| fail(format!("--keys: unknown key {k}"))))
        })
        .collect();
    keys.sort_by(|a, b| a.0.total_cmp(&b.0));
    keys
}

struct Surface {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    painter: Painter,
    depth: wgpu::TextureView,
}

/// A movie's pictures: the disc's, or the remastered file standing in for it.
enum Source {
    Disc {
        movie: Movie<Bytes>,
        /// The picture on screen.
        shown: Option<usize>,
    },
    Remastered(HdMovie),
}

/// A movie being played.
struct Playing {
    name: String,
    source: Source,
    started: Instant,
    /// A newly pressed button stops it.
    skippable: bool,
    voice: Option<qwc_audio::VoiceId>,
}

struct Live<'a> {
    flow: Flow,
    /// Shared with the front end's natives (`FrontEnd::assets`).
    disc: std::rc::Rc<Disc<'a>>,
    size: (u32, u32),
    /// The window's mode and vsync (the player's settings).
    window_mode: qwc_settings::WindowMode,
    vsync: bool,
    /// The sound's levels: the settings', changed by the in-game options (`crate::options`).
    sound: qwc_settings::Sound,
    /// The packs' big pictures compressed (`crate::compression`).
    compress: Option<Option<std::path::PathBuf>>,
    /// Keys held, by pad word.
    held: [bool; 16],
    /// `--keys` still to press, and the one held for the next tick.
    script: Vec<(f64, KeyCode)>,
    scripted: Option<PadWord>,
    /// When the window opened: `--keys` and `--time` count from it.
    opened: Instant,
    /// Ticks run.
    ticks: u64,
    /// `--time`: quit after this many seconds.
    quit_at: Option<f64>,
    /// Where frames are saved, and how many were.
    out: Option<std::path::PathBuf>,
    shots: u32,
    shoot: bool,
    list: DrawList,
    /// The tick before's list.
    previous: DrawList,
    /// Steps of the flow already printed.
    reported: usize,
    last: Instant,
    behind: Duration,
    surface: Option<Surface>,
    /// The engine events already looked at (`FrontEnd::engine`).
    engine_seen: usize,
    playing: Option<Playing>,
    /// A key pressed since the last redraw (for skipping a movie).
    pressed: bool,
    /// The sound (`crate::audio`), without an output device none.
    audio: Option<crate::audio::Audio>,
    /// Where the remastered movies are, the first with a movie winning (none: `--original-movies`), and the decoder
    /// for them.
    remastered: Vec<std::path::PathBuf>,
    decoder: qwc_video::Decoder,
    /// How the matches are lit (`--look`), and the remastered textures their models draw (`--textures`).
    world: (qwc_render::Look, Option<qwc_assets::remaster::Remastered>),
    /// The match the front end started (`crate::fematch`), while it runs.
    run: Option<MatchRun>,
    /// The scripts' log lines already printed (`QWC_FE_LOG`).
    logged: usize,
    /// A cutscene plays (`crate::cutscene`, drawn by the painter), whether the match asked for it, and a key asked
    /// to skip it.
    cutscene: bool,
    match_cutscene: bool,
    skip_cutscene: bool,
    /// The remaster's layout changes to the screens (`qwc_frontend::layouts`), read again when a file changes.
    layouts: qwc_frontend::Layouts,
    /// The gamepads (`crate::gamepad`): the first is player 1's, the second player 2's.
    pads: crate::gamepad::Gamepads,
}

impl Live<'_> {
    /// The pads' events: a button skips a movie or a cutscene as a key does.
    fn poll_pads(&mut self) {
        self.pads.poll();
        if self.pads.take_pressed() {
            if self.playing.is_some() {
                self.pressed = true;
            } else if self.cutscene {
                self.skip_cutscene = true;
            }
        }
    }

    /// One 24 Hz tick.
    fn tick(&mut self) {
        self.ticks += 1;
        let pad_words = [self.pads.menu_words(0), self.pads.menu_words(1)];
        // read every tick, so that no press latched in the menus reaches the match
        let states = [self.pads.state(0), self.pads.state(1)];
        if let Some(run) = &mut self.run {
            run.controls.gamepad = states[0];
            run.controls2.gamepad = states[1];
        }
        let f = &mut self.flow;
        if self.layouts.refresh() {
            // an image file written over is read again
            for img in self.layouts.layers().flat_map(|l| {
                let dir = self.layouts.images_dir();
                let objs = l.objects.iter().filter_map(|o| o.image.clone());
                objs.chain(l.adds.iter().map(|a| a.image.clone()))
                    .map(move |i| qwc_frontend::layouts::image_spec(&dir, &i))
                    .collect::<Vec<_>>()
            }) {
                self.disc.drop_texture(&img);
            }
            self.layouts.apply(&mut f.fe, &self.disc);
        }
        for (i, (w, &down)) in f.fe.pads.words[0].iter_mut().zip(&self.held).enumerate() {
            *w = (down || pad_words[0][i] || self.scripted.is_some_and(|s| s as usize == i)) as u16;
        }
        // the second pad is player 2's menus
        for (w, &down) in f.fe.pads.words[1].iter_mut().zip(&pad_words[1]) {
            *w = down as u16;
        }
        // a scripted key is held for one tick
        self.scripted = None;
        f.m.frame(&mut f.fe, &mut f.vm);
        self.disc.set_loaded(&f.m, &f.sets);
        // the text is laid out for the screen's place in the window it is drawn on, the pins for its margins
        let size = self.surface.as_ref().map_or(self.size, |s| (s.config.width, s.config.height));
        let fit = crate::fe::screen_fit(size);
        self.disc.set_screen(fit.area(size));
        f.fe.margins = fit.margins();
        self.previous = std::mem::replace(&mut self.list, f.draw(&*self.disc));
        report(f, self.reported);
        self.reported = f.m.steps.len();
        crate::fe::log_lines(&f.fe, self.ticks as usize, &mut self.logged);
        let events = &f.fe.engine[self.engine_seen.min(f.fe.engine.len())..];
        let movie = events.iter().rev().find_map(|e| match e {
            EngineEvent::PlayMovie { name, flag } => Some((name.clone(), *flag)),
            _ => None,
        });
        // the match screen's `General_InitMatch` starts the match (its flag, `CurrentChallengeNum >= 0`, says a
        // challenge; **[I]** either way a match), `General_DeInitMatch` ends it
        let (init, deinit) = (
            events.iter().any(|e| matches!(e, EngineEvent::InitMatch(_))),
            events.iter().any(|e| matches!(e, EngineEvent::DeInitMatch)),
        );
        let cutscene = events.iter().any(|e| matches!(e, EngineEvent::PlayCutScene(_)));
        // the menus' speech (`crate::fespeech`)
        let speech: Vec<EngineEvent> = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    EngineEvent::FrontendSpeech(_) | EngineEvent::ChallengeSpeech(_) | EngineEvent::ResultsSpeech(_)
                )
            })
            .cloned()
            .collect();
        let volumes = crate::options::volume_events(events);
        // the Video options' resolution: the window changed, and kept as the settings' resolution
        let display = events.iter().rev().find_map(|e| match e {
            EngineEvent::ChangeDisplay(i) => f.fe.display_modes.get(*i as usize).copied(),
            _ => None,
        });
        self.engine_seen = f.fe.engine.len();
        if let Some((w, h, _)) = display {
            if let Some(s) = &self.surface {
                crate::display::apply(&s.window, (w, h), self.window_mode);
            }
            self.size = (w, h);
            let mut s = qwc_settings::Settings::load();
            (s.display.width, s.display.height) = (w, h);
            if let Err(e) = s.save() {
                eprintln!("settings: {e}");
            }
        }
        if !volumes.is_empty() {
            if crate::options::take(&volumes, &mut self.sound, &f.fe, &mut f.vm.mem) {
                // the player's slider: kept in the settings (the launcher's page shows it), the rest of the file as
                // it is now
                let mut s = qwc_settings::Settings::load();
                s.sound = qwc_settings::Sound { master: s.sound.master, ..self.sound };
                if let Err(e) = s.save() {
                    eprintln!("settings: {e}");
                }
            }
            if let Some(a) = &self.audio {
                a.set_volumes(&self.sound);
            }
        }
        let sent = std::mem::take(&mut f.fe.audio_events);
        if let Some(a) = &mut self.audio {
            for e in &speech {
                match e {
                    EngineEvent::FrontendSpeech(args) => a.frontend_speech(*args),
                    EngineEvent::ChallengeSpeech(args) => a.challenge_speech(*args),
                    EngineEvent::ResultsSpeech(args) => a.results_speech(*args),
                    _ => {}
                }
            }
        }
        // the cutscene `General_PlayCutScene` queues (`FEIntro`) plays over the stadium; its end reaches the screen's
        // layers as general event 2 (`eFE_CUTSCENE_FINISH`). Without a window it ends at once. **[I]** a key skips
        // it, though its script marks it not interruptible (the remaster's choice: it runs nearly three minutes)
        let mut ended = false;
        if cutscene {
            // **[I]** the front end's cutscenes are the House Cup's: Gryffindor and Slytherin
            let (teams, cast) = (["Gryff", "Slyth"], crate::cutscene::Cast::default());
            let started = match &mut self.surface {
                Some(s) => {
                    let place = crate::cutscene::Place::AsMade;
                    s.painter.start_cutscene(&s.gpu, self.disc.vfs, "feintro", teams, &cast, place)
                }
                None => false,
            };
            self.cutscene = started;
            self.match_cutscene = false;
            self.skip_cutscene = false;
            ended = !started;
        }
        // the match's next cutscene (`crate::fematch`), once none plays; one that cannot play is over at once
        if let (false, Some(run)) = (self.cutscene, &mut self.run) {
            if let Some(req) = run.next_cutscene() {
                let teams = run.cutscene_teams(req.team).map(str::to_string);
                let mut cast = run.cast();
                cast.own = req.own.clone();
                cast.other_own = req.other_own.clone();
                let started = match &mut self.surface {
                    Some(s) => {
                        let teams = [teams[0].as_str(), teams[1].as_str()];
                        s.painter.start_cutscene(&s.gpu, self.disc.vfs, &req.name, teams, &cast, req.place)
                    }
                    None => false,
                };
                if started {
                    (self.cutscene, self.match_cutscene, self.skip_cutscene) = (true, true, false);
                    if let Some(a) = &mut self.audio {
                        a.cutscene_started(&req.name, &teams[0], &run.m);
                    }
                } else {
                    run.cutscene_ended();
                }
            }
        }
        if self.cutscene {
            // its triggers: the speech lines, and the subtitles (`CS_SUBT_<name>` shows string `ENG_CS_<name>`
            // until `CS_SUBT_<name>_END`: general event 5 to the screen's layers with the string's id and whether
            // it shows; the screen's script draws it, `OnSubtitleGeneral`: with the option `SubTitles` on and the
            // language one without the speech dubbed, Czech, Korean, Thai or Chinese)
            let events = self.surface.as_mut().map(|s| s.painter.take_cutscene_events()).unwrap_or_default();
            for e in &events {
                if let Some(a) = &mut self.audio {
                    a.cutscene_event(e);
                    if let Some(run) = &self.run {
                        a.commentary_trigger(e, &run.m);
                    }
                }
                if let Some(name) = e.strip_prefix("CS_SUBT_") {
                    let (name, shown) = match name.strip_suffix("_END") {
                        Some(n) => (n, 0),
                        None => (name, 1),
                    };
                    let id = qwc_formats::flow::name_hash(&format!("ENG_CS_{name}"));
                    let f = &mut self.flow;
                    let layers = f.m.current_screen().map(|s| s.layers.clone()).unwrap_or_default();
                    for l in layers {
                        if let Err(e) = f.fe.general_with(&mut f.vm, l, &[5, 0, id, shown]) {
                            eprintln!("subtitle: {e}");
                        }
                    }
                }
            }
            let over = self.surface.as_ref().is_none_or(|s| s.painter.cutscene_over());
            if over || std::mem::take(&mut self.skip_cutscene) || (deinit && self.match_cutscene) {
                if let Some(s) = &mut self.surface {
                    s.painter.end_cutscene();
                }
                self.cutscene = false;
                match (self.match_cutscene, &mut self.run) {
                    (true, Some(run)) => run.cutscene_ended(),
                    (true, None) => {}
                    (false, _) => ended = true,
                }
            }
        }
        if ended {
            let f = &mut self.flow;
            let layers = f.m.current_screen().map(|s| s.layers.clone()).unwrap_or_default();
            for l in layers {
                if let Err(e) = f.fe.general(&mut f.vm, l, 2) {
                    eprintln!("cutscene end: {e}");
                }
            }
        }
        if deinit {
            self.run = None;
            if let Some(s) = &mut self.surface {
                s.painter.end_match();
            }
            if let Some(a) = &mut self.audio {
                a.match_ended();
            }
        }
        if init {
            let run = MatchRun::start(&self.flow, self.disc.vfs, self.ticks as u32);
            if let Some(a) = &mut self.audio {
                let sounds = run.spec.sounds.clone();
                let folder =
                    sounds.unwrap_or_else(|| crate::matchaudio::stadium_folder(self.disc.vfs, &run.spec.stadium));
                if let Some(ch) = &run.challenge {
                    a.challenge_started(folder, ch.kind);
                } else {
                    a.match_started(&run.spec.music, folder);
                    a.commentary_started(run.commentary());
                }
            }
            self.run = Some(run);
        }
        if let Some(run) = &mut self.run {
            let heading = self.surface.as_ref().map_or([-1.0, 0.0], |s| s.painter.heading);
            if run.tick(&mut self.flow, self.ticks, heading) {
                if let Some(s) = &mut self.surface {
                    s.painter.hold_camera(run.camera_held());
                    s.painter.match_ticked(&run.m, run.time);
                    s.window.set_title(&crate::play::title(&run.spec, &run.m));
                }
            }
            if let Some(a) = &mut self.audio {
                for e in run.music() {
                    a.match_event(e);
                }
                if run.cue.take() == Some(crate::restart::Cue::HudStart) {
                    a.hud_start();
                }
                if !self.flow.fe.paused {
                    a.match_tick(&run.m, run.input.as_ref());
                }
                if let Some(num) = run.briefing.take() {
                    a.challenge_begins(num, crate::commentary::team_code(&run.spec.teams[0]));
                }
                if run.challenge_events.contains(&qwc_sim::challenge::Event::Countdown(0)) {
                    a.countdown_over();
                }
            }
        }
        if let Some(a) = &mut self.audio {
            a.tick(&sent, self.disc.package_loaded(crate::audio::FE_PACKAGE));
        }
        if let Some((name, flag)) = movie {
            self.start_movie(&name, flag != 0);
        }
    }

    /// The remastered movie standing in for `name`, when there is one that opens.
    fn remastered_movie(&self, name: &str) -> Option<(HdMovie, String)> {
        let path = self.remastered.iter().find_map(|d| qwc_video::find(d, name))?;
        let screen = self.surface.as_ref().map_or(self.size, |s| (s.config.width, s.config.height));
        match qwc_video::open(&path, screen, self.decoder) {
            Ok(m) => {
                let (w, h) = m.decoded;
                let from = format!("{} through {} at {w}x{h}", path.display(), m.decoder);
                Some((m, from))
            }
            Err(e) => {
                eprintln!("{e}: the disc's movie plays");
                None
            }
        }
    }

    /// Start `name` (as the script names it, a path on the disc).
    fn start_movie(&mut self, name: &str, skippable: bool) {
        let (source, sound, seconds, from) = match self.remastered_movie(name) {
            Some((mut m, from)) => {
                let seconds = m.seconds();
                let sound = m.sound.take().ok_or_else(|| "no sound".to_string());
                (Source::Remastered(m), sound, seconds, from)
            }
            None => {
                let path = name.replace('\\', "/");
                let read = self.disc.vfs.read(&path).map_err(|e| e.to_string());
                match read.and_then(|b| Movie::parse(b).map_err(|e| e.to_string())) {
                    Ok(movie) => {
                        let sound = movie.audio().map_err(|e| e.to_string());
                        let seconds = movie.seconds();
                        (Source::Disc { movie, shown: None }, sound, seconds, path)
                    }
                    Err(e) => {
                        eprintln!("movie {name}: {e}");
                        return;
                    }
                }
            }
        };
        if let Some(a) = &mut self.audio {
            a.hush(true);
        }
        let voice = match (&self.audio, sound) {
            (Some(a), Ok(pcm)) => {
                let params = qwc_audio::Params { bus: qwc_audio::Bus::Music, ..Default::default() };
                Some(a.mixer().lock().unwrap().play(Arc::new(pcm), params))
            }
            (_, Err(e)) => {
                eprintln!("movie {name}: sound: {e}");
                None
            }
            _ => None,
        };
        eprintln!("playing {name}: {from} ({seconds:.1} s{})", if skippable { ", any key skips" } else { "" });
        self.pressed = false;
        self.playing = Some(Playing { name: name.into(), source, started: Instant::now(), skippable, voice });
    }

    /// One frame of the movie playing; answers whether it is still playing.
    fn movie_frame(&mut self) -> bool {
        let Some(p) = &mut self.playing else { return false };
        let t = p.started.elapsed().as_secs_f64();
        let ended = match &p.source {
            Source::Disc { movie, .. } => (t * FPS as f64) as usize >= movie.frames.len(),
            Source::Remastered(m) => m.ended(t),
        };
        let skipped = p.skippable && std::mem::take(&mut self.pressed);
        if ended || skipped {
            if let Some(a) = &mut self.audio {
                if let Some(v) = p.voice {
                    a.mixer().lock().unwrap().stop(v);
                }
                a.hush(false);
            }
            eprintln!("{} {}", p.name, if skipped { "skipped" } else { "ended" });
            self.playing = None;
            // the flow takes up from now: no ticks to catch up
            self.last = Instant::now();
            self.behind = Duration::ZERO;
            return false;
        }
        let Some(s) = &mut self.surface else { return true };
        let frame = match s.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => {
                s.surface.configure(&s.gpu.device, &s.config);
                s.window.request_redraw();
                return true;
            }
        };
        // the new picture: (pixels, size, stored B G R X)
        let (picture, keep) = match &mut p.source {
            Source::Disc { movie, shown } => {
                let k = (t * FPS as f64) as usize;
                let picture = if *shown != Some(k) {
                    match movie.frame(k) {
                        Ok(f) => Some((f.to_rgba(), (f.width as u32, f.height as u32), false)),
                        Err(e) => {
                            eprintln!("{} frame {k}: {e}", p.name);
                            None
                        }
                    }
                } else {
                    None
                };
                *shown = Some(k);
                (picture, None)
            }
            Source::Remastered(m) => (m.picture(t).map(|x| (x.bgra, m.decoded, true)), Some(m.size)),
        };
        let view = frame.texture.create_view(&Default::default());
        let screen = (s.config.width, s.config.height);
        let picture = picture.as_ref().map(|(px, size, bgra)| MoviePicture { pixels: px, size: *size, bgra: *bgra });
        s.painter.movie(&s.gpu, &view, screen, keep, picture);
        self.save_shot(&frame.texture);
        let s = self.surface.as_mut().unwrap();
        s.gpu.queue.present(frame);
        s.window.request_redraw();
        true
    }

    /// F12 or `--out`: the frame in `texture` saved as a PNG, when one was asked for.
    fn save_shot(&mut self, texture: &wgpu::Texture) {
        if !std::mem::take(&mut self.shoot) {
            return;
        }
        let Some(s) = &self.surface else { return };
        let size = (s.config.width, s.config.height);
        match qwc_render::read_texture(&s.gpu, texture, size) {
            Ok(mut px) => {
                if matches!(s.config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb) {
                    px.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p.swap(0, 2));
                }
                let path = self.shot_path();
                write_png(&path, size, px);
                eprintln!("saved {}", path.display());
            }
            Err(e) => eprintln!("screenshot: {e}"),
        }
    }

    /// Where the next saved frame goes.
    fn shot_path(&mut self) -> std::path::PathBuf {
        self.shots += 1;
        match &self.out {
            Some(p) if self.shots == 1 => p.clone(),
            Some(p) => {
                let stem = p.file_stem().map_or("frontend".into(), |s| s.to_string_lossy().into_owned());
                p.with_file_name(format!("{stem}-{}.png", self.shots))
            }
            None => format!("frontend-{}.png", self.shots).into(),
        }
    }

    fn redraw(&mut self, el: &ActiveEventLoop) {
        self.poll_pads();
        let wall = self.opened.elapsed().as_secs_f64();
        while self.script.first().is_some_and(|&(t, _)| t <= wall) {
            let (_, k) = self.script.remove(0);
            if k == KeyCode::F12 {
                self.shoot = true;
            } else if self.playing.is_some() {
                self.pressed = true;
            } else if self.cutscene {
                self.skip_cutscene = true;
            } else {
                self.scripted = word(k);
                // a match takes it as its controls too, held for a third of a second
                if let (Some(run), Some(id)) = (&mut self.run, crate::controls::key_id(k)) {
                    run.controls.tap(id, 8);
                }
            }
        }
        let quitting = self.quit_at.is_some_and(|t| wall >= t);
        if quitting && self.out.is_some() {
            self.shoot = true;
        }
        if self.movie_frame() {
            if quitting {
                el.exit();
            }
            return;
        }
        let now = Instant::now();
        self.behind += now - self.last;
        self.last = now;
        let mut ran = 0;
        while self.behind >= TICK && ran < 3 && self.playing.is_none() {
            self.tick();
            self.behind -= TICK;
            ran += 1;
        }
        if self.behind >= TICK {
            self.behind = Duration::ZERO;
        }
        let Some(s) = &mut self.surface else { return };
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
        let list = self.list.blend(&self.previous, self.behind.as_secs_f32() / TICK.as_secs_f32());
        let (time, store) =
            (self.opened.elapsed().as_secs_f64(), crate::fe::flag_store(&self.flow.fe, &self.flow.vm.mem));
        let alpha = self.behind.as_secs_f64() / TICK.as_secs_f64();
        let game = self.run.as_ref().map(|r| MatchView {
            m: &r.m,
            spec: &r.spec,
            // paused, or held (a cutscene, the restart after a goal), the models stay where the last tick put them
            alpha: if self.flow.fe.paused || r.holding() || r.halted() { 1.0 } else { alpha },
            time: r.time + alpha / 24.0,
            challenge: r.challenge.as_ref().map(|c| (c, r.outcomes.as_slice())),
            dark: r.darkness(),
            hud: r.restart.hud(),
        });
        s.painter.paint(&s.gpu, &view, &s.depth, size, &list, &self.flow.fe, &self.disc, time, &store, game.as_ref());
        self.save_shot(&frame.texture);
        let s = self.surface.as_mut().unwrap();
        s.gpu.queue.present(frame);
        if quitting {
            el.exit();
            return;
        }
        s.window.request_redraw();
    }
}

impl ApplicationHandler for Live<'_> {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.surface.is_some() {
            return;
        }
        let attrs = crate::display::attributes(el, "Quidditch World Cup: front end", self.size, self.window_mode);
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
        // the overlay works in the stored values, as the original's 8-bit screen did: a format without sRGB
        // conversion
        let linear = surface.get_capabilities(&gpu.adapter).formats.into_iter().find(|f| !f.is_srgb());
        config.format = linear.unwrap_or(config.format);
        // the alpha channel is the masks' working space: the window is opaque
        if surface.get_capabilities(&gpu.adapter).alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            config.alpha_mode = wgpu::CompositeAlphaMode::Opaque;
        }
        // F12 reads the frame back
        if surface.get_capabilities(&gpu.adapter).usages.contains(wgpu::TextureUsages::COPY_SRC) {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        config.present_mode = crate::display::present_mode(self.vsync, &surface.get_capabilities(&gpu.adapter));
        surface.configure(&gpu.device, &config);
        let mut painter = Painter::new(&gpu, config.format);
        painter.set_compression(self.compress.clone());
        painter.set_world(self.world.0, self.world.1.clone());
        let depth = qwc_render::depth_view(&gpu.device, (config.width, config.height));
        // the in-game Video options' modes: the monitor's, the window's own the current one
        let current = (config.width, config.height);
        let fe = &mut self.flow.fe;
        fe.display_modes = crate::display::modes(&window, self.window_mode, current);
        let at = fe.display_modes.iter().position(|&(w, h, d)| (w, h) == current && d == 32);
        fe.display_mode = at.map_or(u32::MAX, |i| i as u32);
        fe.note_display_modes(&mut self.flow.vm.mem);
        self.surface = Some(Surface { window, surface, config, gpu, painter, depth });
        self.last = Instant::now();
        self.opened = self.last;
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => {
                if let Some(s) = &mut self.surface {
                    s.config.width = size.width.max(1);
                    s.config.height = size.height.max(1);
                    s.surface.configure(&s.gpu.device, &s.config);
                    s.depth = qwc_render::depth_view(&s.gpu.device, (s.config.width, s.config.height));
                }
            }
            WindowEvent::RedrawRequested => self.redraw(el),
            WindowEvent::KeyboardInput { event, is_synthetic, .. } => {
                let down = event.state == ElementState::Pressed;
                if event.physical_key == PhysicalKey::Code(KeyCode::F12) && down {
                    self.shoot = true;
                } else if self.playing.is_some() {
                    if down && !event.repeat && !is_synthetic {
                        if self.playing.as_ref().is_some_and(|p| p.skippable) && !self.pressed {
                            eprintln!("key {:?}", event.physical_key);
                        }
                        self.pressed = true;
                    }
                } else if self.cutscene {
                    if down && !event.repeat && !is_synthetic {
                        self.skip_cutscene = true;
                    }
                } else if let PhysicalKey::Code(code) = event.physical_key {
                    if let Some(w) = word(code) {
                        self.held[w as usize] = event.state == ElementState::Pressed;
                    }
                    // a match also takes the keys as the game's controls
                    if let (Some(run), Some(id)) = (&mut self.run, crate::controls::key_id(code)) {
                        run.controls.set(id, down);
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let (Some(run), Some(id)) = (&mut self.run, crate::controls::mouse_id(button)) {
                    run.controls.set(id, state == ElementState::Pressed);
                }
            }
            _ => {}
        }
    }
}

pub fn frontend(args: &Args, vfs: &'static Vfs) {
    let mut flow = start_flow(args, vfs);
    // the front end in a window saves where the original does (`crate::record`), unless `QWC_NO_SAVE`
    if std::env::var_os("QWC_NO_SAVE").is_none() {
        if let Some(card) = crate::record::save_card() {
            flow.fe.memcard.io = Box::new(card);
        }
        // and its key bindings beside the save, read as start-up reads them
        if let Some(file) = crate::record::keys_file() {
            flow.fe.keys = qwc_feck::keys::Keys::with_file(file);
        }
    }
    let font_bytes = font_files(vfs, &flow.resources);
    let disc = Disc::shared(
        vfs,
        std::mem::take(&mut flow.resources),
        font_bytes,
        crate::fe::remastered_textures(args, vfs),
        crate::fe::remastered_fonts(args),
    );
    flow.fe.assets = Some(disc.clone());
    let layouts = qwc_frontend::Layouts::default();
    layouts.apply(&mut flow.fe, &disc);
    // 16:9 for the remaster (1280 x 720 unless `--size`): the stadium widens with the window, the remastered movies
    // fill it, and the front end's 2D screen (4:3 in the original, 512 x 512 over 640 x 480) is stretched over it
    // until its art is remastered
    let size = args.size;
    let mut app = Live {
        flow,
        disc,
        size,
        window_mode: args.window,
        vsync: args.vsync,
        sound: args.sound,
        compress: crate::compression(args),
        held: [false; 16],
        script: args.keys.as_deref().map(parse_keys).unwrap_or_default(),
        scripted: None,
        opened: Instant::now(),
        ticks: 0,
        quit_at: (args.time > 0.0).then_some(args.time),
        out: args.out.clone(),
        shots: 0,
        shoot: false,
        list: DrawList::default(),
        previous: DrawList::default(),
        reported: 0,
        last: Instant::now(),
        behind: Duration::ZERO,
        surface: None,
        engine_seen: 0,
        playing: None,
        pressed: false,
        audio: crate::audio::Audio::open(vfs).inspect(|a| a.set_volumes(&args.sound)),
        remastered: if args.original_movies { Vec::new() } else { args.movies.clone() },
        decoder: if args.openh264 { qwc_video::Decoder::OpenH264 } else { qwc_video::Decoder::Any },
        world: (args.look, crate::fe::remastered_textures(args, vfs)),
        run: None,
        logged: 0,
        cutscene: false,
        match_cutscene: false,
        skip_cutscene: false,
        layouts,
        pads: crate::gamepad::Gamepads::new(),
    };
    let el = EventLoop::new().unwrap_or_else(|e| fail(e));
    el.run_app(&mut app).unwrap_or_else(|e| fail(e));
}
