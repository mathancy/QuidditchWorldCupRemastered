//! `qwc-game online host [TEAM [OPPONENT]] [--stadium NAME] [--port N] [--delay FRAMES]` and
//! `qwc-game online join ADDRESS [TEAM]`: a match against another player over the network, with no server
//! (`qwc-net`). The host listens on a UDP port (20030 unless `--port`) and plays team 0; the joiner connects to the
//! host's address (`ip`, `ip:port` or a name) and plays team 1, the team it asks for unless the host's is the same
//! (then the host's OPPONENT, England or France by default). The host picks the stadium (`--stadium`, default
//! `england`) and the seed. Both machines run the same match from the same data in lockstep: each tick only the
//! players' input crosses the network ([`pack`]), and each side checks the other's checksum of the match.
//!
//! Not the original's (it had no online play): the remaster's own. The match is `play`'s (`crate::play`), each
//! machine showing its own player's view (`Arena::viewer`), not the split screen.
//!
//! In the window: P (or the pad's Start) pauses both players' games, Escape leaves. With `--time SECS --out FILE`
//! the side plays headless instead, its player's hands scripted ([`bot`]), and writes the result to FILE when the
//! match reaches SECS (for checks: run a host and a joiner and compare their checksums).
//!
//! Getting through the routers (`docs/ONLINE.md`): the host asks its router to forward the port (UPnP, unless
//! `--no-upnp`), finds its public address from public STUN servers, listens on IPv6 too, and lets in the joiners
//! whose public addresses the launcher passes on (`punch ADDRESS` lines on standard input: it sends to them, which
//! opens its router to their answers). The joiner finds its own public address, to give to the host.
//!
//! Status lines go to standard output for the launcher's Online page, each `@online WORD ...`: `hosting PORT`,
//! `address IP:PORT` (this machine's on its network), `address6 [IP]:PORT`, `upnp [IP:PORT]` (the router forwards
//! the port) or `upnp-failed REASON`, `public IP:PORT [symmetric]` (this side's address on the internet; symmetric:
//! a router no punched hole gets through), `letting-in ADDRESS`, `other-build ADDRESS BUILD` (a joiner turned away),
//! `joining ADDRESS`, `joined ADDRESS`, `connected ADDRESS`, `start DELAY RTT_MS`, `end SCORE0 SCORE1`,
//! `over REASON` and `error MESSAGE`.

use std::io::Write as _;
use std::time::{Duration, Instant};

use qwc_assets::Vfs;
use qwc_net::{Joining, Session, Welcomed};
use qwc_render::overlay::{Overlay, OverlayQuad, OverlayVertex};
use qwc_render::Gpu;
use qwc_sim::human::Input;
use qwc_sim::race::SeekerPad;
use qwc_sim::Match;
use serde::{Deserialize, Serialize};

use crate::controls::Controls;
use crate::{fail, Args};

/// What the two builds must share to play: the remaster's version (`docs/RELEASING.md`) and the frame's layout.
pub const BUILD: &str = concat!(env!("CARGO_PKG_VERSION"), "/1");

/// An input frame's bytes: the control bits, the three sticks across the ground as `f32`s, the buttons, the Seeker
/// game's stick and buttons.
pub const FRAME_LEN: usize = 1 + 6 * 4 + 2 + 2 * 4 + 1;
/// A control bit: the player asks to pause (or to play on).
const PAUSE: u8 = 1;

/// The match the host chose, sent to the joiner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Setup {
    /// Team 0's (the host's) and team 1's folders, and the stadium's.
    pub teams: [String; 2],
    pub stadium: String,
    /// The match's `rand()` seed, and the remaster's formotion momentum (the host's setting: both must play alike).
    pub seed: u32,
    pub momentum: f64,
}

impl Setup {
    pub fn to_bytes(&self) -> Vec<u8> {
        ron::to_string(self).unwrap_or_default().into_bytes()
    }

    pub fn from_bytes(b: &[u8]) -> Result<Setup, String> {
        ron::from_str(std::str::from_utf8(b).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }
}

/// One player's input in a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameInput {
    pub input: Input,
    pub seeker: SeekerPad,
    pub control: u8,
}

/// The frame's bytes for `input` and `seeker` with `control` bits. The sticks' `y` is always 0 (`Input::from_pad`);
/// the values become `f32`s, which both machines then use alike (the local player's input is the one that comes
/// back from [`unpack`], not the one packed).
pub fn pack(input: &Input, seeker: &SeekerPad, control: u8) -> Vec<u8> {
    let mut b = Vec::with_capacity(FRAME_LEN);
    b.push(control);
    for v in [input.stick[0], input.stick[2], input.stick2[0], input.stick2[2], input.raw[0], input.raw[2]] {
        b.extend_from_slice(&(v as f32).to_le_bytes());
    }
    let buttons = [
        input.action_pressed,
        input.action_held,
        input.shoot_pressed,
        input.shoot_held,
        input.special,
        input.boost_pressed,
        input.boost_held,
        input.dodge_pressed,
        input.special_pressed,
        input.special_held,
        input.combo_left_pressed,
        input.combo_left_held,
        input.combo_right_pressed,
        input.combo_right_held,
        input.uber_pressed,
    ];
    let bits = buttons.iter().enumerate().fold(0u16, |a, (k, &on)| a | u16::from(on) << k);
    b.extend_from_slice(&bits.to_le_bytes());
    for v in seeker.stick {
        b.extend_from_slice(&(v as f32).to_le_bytes());
    }
    b.push(u8::from(seeker.boost_held) | u8::from(seeker.boost_pressed) << 1 | u8::from(seeker.grab_held) << 2);
    debug_assert_eq!(b.len(), FRAME_LEN);
    b
}

/// A frame's bytes read back ([`pack`]); a frame of another length reads as no input.
pub fn unpack(b: &[u8]) -> FrameInput {
    if b.len() != FRAME_LEN {
        return FrameInput::default();
    }
    let f = |k: usize| f64::from(f32::from_le_bytes(b[1 + 4 * k..5 + 4 * k].try_into().unwrap()));
    let bits = u16::from_le_bytes([b[25], b[26]]);
    let on = |k: u32| bits & 1 << k != 0;
    let input = Input {
        stick: [f(0), 0.0, f(1)],
        stick2: [f(2), 0.0, f(3)],
        raw: [f(4), 0.0, f(5)],
        action_pressed: on(0),
        action_held: on(1),
        shoot_pressed: on(2),
        shoot_held: on(3),
        special: on(4),
        boost_pressed: on(5),
        boost_held: on(6),
        dodge_pressed: on(7),
        special_pressed: on(8),
        special_held: on(9),
        combo_left_pressed: on(10),
        combo_left_held: on(11),
        combo_right_pressed: on(12),
        combo_right_held: on(13),
        uber_pressed: on(14),
    };
    let g = |k: usize| f64::from(f32::from_le_bytes(b[27 + 4 * k..31 + 4 * k].try_into().unwrap()));
    let s = b[35];
    let seeker =
        SeekerPad { stick: [g(0), g(1)], boost_held: s & 1 != 0, boost_pressed: s & 2 != 0, grab_held: s & 4 != 0 };
    FrameInput { input, seeker, control: b[0] }
}

/// A status line for the launcher (standard output, flushed at once).
pub fn status(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "@online {line}");
    let _ = out.flush();
}

/// A seed from the clock (the host's: the joiner takes it from the setup).
fn seed() -> u32 {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    (t.as_nanos() as u32 ^ (t.as_secs() as u32).rotate_left(16)).max(1)
}

/// The handshake's last step, once the window has loaded the match: the host's start or the joiner's ready.
pub enum Pending {
    Host { welcomed: Welcomed, start_sum: u64, delay: Option<u32> },
    Join { joining: Joining, start_sum: u64 },
}

impl Pending {
    pub fn finish(self) -> Result<Session, qwc_net::Error> {
        let s = match self {
            Pending::Host { welcomed, start_sum, delay } => welcomed.start(start_sum, FRAME_LEN, delay)?,
            Pending::Join { joining, start_sum } => joining.ready(start_sum, FRAME_LEN)?,
        };
        let rtt = s.rtt().map_or(0, |r| r.as_millis());
        status(&format!("start {} {rtt}", s.delay()));
        eprintln!("online: playing {} (input {} frames late, round trip {rtt} ms)", s.peer(), s.delay());
        Ok(s)
    }
}

/// Both players' input for a frame to run, in team order, as the frames' bytes came.
pub struct Frame {
    pub inputs: [FrameInput; 2],
    bytes: [Vec<u8>; 2],
}

/// What running a frame did ([`Online::run`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ran {
    /// Both games are paused (nothing ran); the match stepped (not held after a goal); the restart's cue.
    pub paused: bool,
    pub stepped: bool,
    pub cue: Option<crate::restart::Cue>,
}

/// The online side of a match in the window: the handshake still to finish, then the session, the shared pause,
/// how long the other player has been waited for, and the banner that says so.
pub struct Online {
    /// 0 when hosting (team 0), 1 when joined (team 1).
    pub seat: usize,
    pub pending: Option<Pending>,
    pub session: Option<Session>,
    /// Both games paused (either player's P), from the frame both saw it.
    pub paused: bool,
    pause_asked: bool,
    /// Since when the next frame has waited for the other player's input.
    waiting: Option<Instant>,
    /// The end already reported (on standard output), and the match's end.
    over_told: bool,
    full_time_told: bool,
    pub banner: Banner,
    /// Every frame run: its number, both inputs' bytes, the pause and the checksum after it, written out when the
    /// games go out of step ([`Online::write_log`]) to compare the two sides'.
    log: Vec<String>,
}

impl Online {
    pub fn new(seat: usize, pending: Pending) -> Online {
        Online {
            seat,
            pending: Some(pending),
            session: None,
            paused: false,
            pause_asked: false,
            waiting: None,
            over_told: false,
            full_time_told: false,
            banner: Banner::default(),
            log: Vec::new(),
        }
    }

    /// The handshake's end (blocking: the other side may still be loading); `Err` ends the game.
    pub fn connect(&mut self) -> Result<(), String> {
        let Some(p) = self.pending.take() else { return Ok(()) };
        let s = p.finish().map_err(|e| e.to_string())?;
        self.session = Some(s);
        Ok(())
    }

    /// Reads what came; reports the session's end once.
    pub fn poll(&mut self) {
        let Some(s) = &mut self.session else { return };
        s.poll();
        if let Some(end) = s.end().cloned() {
            if !self.over_told {
                self.over_told = true;
                status(&format!("over {end}"));
                eprintln!("online: {end}");
                if matches!(end, qwc_net::End::Desync(_)) || std::env::var_os("QWC_ONLINE_LOG").is_some() {
                    self.write_log();
                }
            }
        }
    }

    /// The frames run, to `online-frames-<seat>.txt` in the library's `logs` (where the launcher's `online.log` is;
    /// else the working folder): after a desync, the two sides' files show the first frame whose inputs or checksum
    /// differ.
    pub fn write_log(&self) {
        let dir = qwc_packs::library_dir().map(|d| d.join("logs")).unwrap_or_default();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("online-frames-{}.txt", self.seat));
        let text =
            format!("# frame, team 0's input, team 1's input, paused, checksum after\n{}\n", self.log.join("\n"));
        match std::fs::write(&path, text) {
            Ok(()) => eprintln!("online: the frames run are in {}", path.display()),
            Err(e) => eprintln!("online: {}: {e}", path.display()),
        }
    }

    /// The local player asks to pause or play on (sent with the next input; both games toggle on the frame).
    pub fn ask_pause(&mut self) {
        self.pause_asked = true;
    }

    /// Our tick's length ([`Session::tick_length`]: a little longer while we run ahead).
    pub fn tick_length(&self) -> Duration {
        self.session.as_ref().map_or(qwc_net::TICK, Session::tick_length)
    }

    /// A tick: the local player's input from `controls` (turned by the camera's `heading`) given for the frame
    /// `delay` on, then the frame due, once both players' inputs are here; `None` while waiting for the other's.
    pub(crate) fn frame(&mut self, controls: &mut Controls, heading: [f64; 2], combos: bool) -> Option<Frame> {
        self.take(|control| {
            controls.tick();
            let input = Input::from_pad(&controls.pad, heading, combos);
            pack(&input, &SeekerPad::from_pad(&controls.pad), control)
        })
    }

    /// The tick's core, the window's and the headless side's alike: this side's input frame from `give` (handed the
    /// control bits to send, the pause asked for) when one is due, then the frame to run once both inputs are in.
    /// Either player's pause bit toggles the pause, on the same frame on both machines.
    pub fn take(&mut self, give: impl FnOnce(u8) -> Vec<u8>) -> Option<Frame> {
        self.poll();
        let s = self.session.as_mut()?;
        if s.wants_input() {
            let control = if std::mem::take(&mut self.pause_asked) { PAUSE } else { 0 };
            s.input(give(control));
        }
        let Some(bytes) = s.ready() else {
            self.waiting.get_or_insert_with(Instant::now);
            return None;
        };
        self.waiting = None;
        let inputs = [unpack(&bytes[0]), unpack(&bytes[1])];
        if (inputs[0].control | inputs[1].control) & PAUSE != 0 {
            self.paused = !self.paused;
        }
        Some(Frame { inputs, bytes })
    }

    /// Runs `frame` on `m` as both machines must: both players' inputs, then (unless paused) the step (unless the
    /// restart after a goal holds play) and the restart's tick; the checksum to the session, the frame to the log,
    /// the match's end reported once.
    pub fn run(&mut self, m: &mut Match, restart: &mut crate::restart::Restart, frame: &Frame) -> Ran {
        for (t, f) in frame.inputs.iter().enumerate() {
            m.set_input(t, f.input);
            m.set_seeker_pad(t, f.seeker);
        }
        let mut ran = Ran { paused: self.paused, stepped: false, cue: None };
        if !self.paused {
            if !restart.holds() {
                m.step();
                ran.stepped = true;
            }
            ran.cue = restart.tick(m, false);
        }
        let sum = m.checksum();
        if let Some(s) = &mut self.session {
            let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
            self.log.push(format!(
                "{} {} {} {} {sum:016x}",
                s.frame(),
                hex(&frame.bytes[0]),
                hex(&frame.bytes[1]),
                u8::from(self.paused)
            ));
            s.ran(sum);
        }
        if m.finished && !self.full_time_told {
            self.full_time_told = true;
            status(&format!("end {} {}", m.teams[0].score, m.teams[1].score));
        }
        ran
    }

    /// Leaves the game, telling the other player.
    pub fn leave(&mut self) {
        if let Some(s) = &mut self.session {
            s.close("left the game");
        }
    }

    /// The round trip, for the title.
    pub fn rtt(&self) -> Option<Duration> {
        self.session.as_ref().and_then(Session::rtt)
    }

    /// What the banner says now: the session's end, a long wait for the other player, full time, the pause; none in
    /// play.
    pub fn message(&self) -> Option<Vec<String>> {
        let s = self.session.as_ref()?;
        if let Some(end) = s.end() {
            let first = match end {
                qwc_net::End::Left(why) => format!("The other player left ({why})."),
                qwc_net::End::Lost => "The connection to the other player was lost.".to_string(),
                qwc_net::End::Desync(n) => format!("The two games went out of step (frame {n})."),
                qwc_net::End::Closed => "You left the game.".to_string(),
            };
            return Some(vec![first, "Escape to leave".into()]);
        }
        if self.waiting.is_some_and(|w| w.elapsed() > Duration::from_millis(400)) {
            let quiet = s.silence().as_secs();
            let second =
                if quiet >= 2 { format!("nothing heard for {quiet} s") } else { "the connection is slow".into() };
            return Some(vec!["Waiting for the other player...".into(), second]);
        }
        if self.full_time_told {
            return Some(vec!["Full time".into(), "Escape to leave".into()]);
        }
        self.paused.then(|| vec!["Paused".into(), "P to play on".into()])
    }
}

/// The online messages over the match: a dimmed band across the middle of the screen and lines of text in the
/// remaster's vector fonts (the front end's large heading's font: the packs' list, else the stock
/// `remastered/fonts.txt` shipped with the launcher, since the banner is the remaster's own and shows with no pack
/// on), each glyph its own overlay texture.
#[derive(Default)]
pub struct Banner {
    /// The font, once looked for (none without a fonts list or the family in it).
    font: Option<Option<qwc_text::TextFont>>,
    faces: qwc_text::Faces,
    text: qwc_text::Renderer,
    /// The glyph textures uploaded, and the screen they were drawn for.
    glyphs: std::collections::HashSet<u64>,
    screen: (u32, u32),
}

impl Banner {
    fn load_font(&mut self) {
        if self.font.is_none() {
            let stock = || qwc_text::FontList::read(std::path::Path::new("remastered/fonts.txt")).ok();
            let list = crate::fe::pack_fonts().or_else(stock);
            let font = list.and_then(|l| qwc_text::TextFont::new(&l, &mut self.faces, "TiepoloBlkITCTT24i_ds.ffn", 26));
            self.font = Some(font.and_then(Result::ok));
        }
    }

    /// The quads for `lines` on a window of `size`: the band, then the text centred on it.
    pub fn quads(
        &mut self,
        overlay: &mut Overlay,
        gpu: &Gpu,
        lines: &[String],
        size: (u32, u32),
        fit: qwc_feck::draw::Fit,
    ) -> Vec<OverlayQuad> {
        let [l, _, r, _] = fit.span;
        let (top, bottom) = (226.0, 226.0 + 30.0 * lines.len() as f32 + 12.0);
        let band = |x, y| OverlayVertex { pos: [x, y], uv: [0.0, 0.0], colour: [0.0, 0.0, 0.0, 0.6] };
        let mut quads = vec![OverlayQuad {
            texture: 0,
            blend: 2,
            layer: 0,
            corners: [band(l, bottom), band(r, bottom), band(l, top), band(r, top)],
        }];
        if self.screen != size {
            self.glyphs.iter().for_each(|&k| overlay.forget(k));
            self.glyphs.clear();
            self.text.clear();
            self.screen = size;
        }
        self.load_font();
        let Some(Some(font)) = &self.font else { return quads };
        font.set_screen(size);
        let (kx, ky) = (size.0.max(1) as f32 / 512.0, size.1.max(1) as f32 / 512.0);
        for (k, line) in lines.iter().enumerate() {
            let chars: Vec<u16> = line.encode_utf16().collect();
            let width: i32 = chars.iter().map(|&c| font.advance(c)).sum();
            let x = 256.0 - width as f32 / 2.0;
            let y = top + 8.0 + 30.0 * k as f32;
            for p in self.text.run(font, &chars, [x, y], size) {
                let key = 1 << 62 | (p.key.hash64() >> 2);
                if !self.glyphs.contains(&key) {
                    let Some(im) = self.text.image(&p.key) else { continue };
                    overlay.upload(gpu, key, im.width, im.height, &im.rgba);
                    self.glyphs.insert(key);
                }
                let (x0, y0) = (p.x / kx, p.y / ky);
                let (x1, y1) = ((p.x + p.width as f32) / kx, (p.y + p.height as f32) / ky);
                let v = |x, y, u, t| OverlayVertex { pos: [x, y], uv: [u, t], colour: [1.0; 4] };
                let corners = [v(x0, y1, 0.0, 1.0), v(x1, y1, 1.0, 1.0), v(x0, y0, 0.0, 0.0), v(x1, y0, 1.0, 0.0)];
                quads.push(OverlayQuad { texture: key, blend: 2, layer: 1, corners });
            }
        }
        quads
    }
}

/// A scripted player for the headless checks: the stick swung round, the buttons pressed now and then, from its own
/// generator seeded by the side (never the match's).
pub struct Bot(u32);

impl Bot {
    pub fn new(seat: usize) -> Bot {
        Bot(0x9e37_79b9 ^ seat as u32)
    }

    pub fn frame(&mut self, tick: u32, control: u8) -> Vec<u8> {
        let mut next = || {
            self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12345);
            self.0 >> 8
        };
        let angle = f64::from(tick % 240) / 240.0 * std::f64::consts::TAU + f64::from(next() % 100) / 400.0;
        let (s, c) = qwc_sim::fmath::sin_cos(angle);
        let mut press = |odds: u32| next() % odds == 0;
        let (action, shoot, boost) = (press(30), press(50), press(8));
        let input = Input {
            stick: [c, 0.0, s],
            raw: [c, 0.0, -s],
            action_pressed: action,
            action_held: action,
            shoot_pressed: shoot,
            shoot_held: shoot,
            boost_pressed: boost,
            boost_held: boost,
            ..Input::default()
        };
        let seeker = SeekerPad { stick: [s, c], boost_held: boost, boost_pressed: boost, grab_held: press(20) };
        pack(&input, &seeker, control)
    }
}

/// `online host|join ...`.
pub fn online(args: &Args, vfs: &'static Vfs) {
    let r = match args.models.first().map(String::as_str) {
        Some("host") => host(args, vfs),
        Some("join") => join(args, vfs),
        _ => crate::usage(),
    };
    if let Err(e) = r {
        status(&format!("error {e}"));
        fail(e);
    }
}

/// The team folder as the disc spells it, if there is one.
fn team(vfs: &Vfs, name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty()).then(|| crate::play::team_folder(vfs, name)).flatten()
}

fn host(args: &Args, vfs: &'static Vfs) -> Result<(), String> {
    let mine = match args.models.get(1) {
        Some(t) => team(vfs, t).ok_or_else(|| format!("no team folder teams/{t}"))?,
        None => team(vfs, "England").ok_or("no team folder teams/England")?,
    };
    let stadium = args.stadium.clone().unwrap_or_else(|| "england".into());
    let folder = format!("stadium/{}/", stadium.to_ascii_lowercase());
    if !vfs.with_extension("fpm").any(|p| p.to_ascii_lowercase().starts_with(&folder)) {
        return Err(format!("no stadium folder stadium/{stadium}"));
    }
    let port = args.port.unwrap_or(qwc_net::DEFAULT_PORT);
    let mut listener = qwc_net::Host::bind(port, BUILD).map_err(|e| format!("port {port}: {e}"))?;
    // what a joiner's check is told (the launcher's Join panel)
    listener.set_about(format!("team {mine}\nstadium {stadium}").as_bytes());
    let port = listener.port();
    status(&format!("hosting {port}"));
    eprintln!("online: hosting {mine} at {stadium} on UDP port {port}");
    let lan = qwc_net::lan_ipv4();
    if let Some(ip) = lan {
        status(&format!("address {ip}:{port}"));
    }
    if let Some(ip) = qwc_net::global_ipv6().filter(|_| listener.ipv6()) {
        status(&format!("address6 [{ip}]:{port}"));
    }
    // the router asked to forward the port (kept until the game closes: dropping it asks the router to stop), and
    // the public address from STUN
    let forwarded = match lan.filter(|_| !args.no_upnp) {
        Some(ip) => match qwc_net::upnp::forward(ip, port, Duration::from_secs(2)) {
            Ok(f) => {
                let ext = f.external_ip.map_or_else(String::new, |ip| format!("{ip}:{port}"));
                status(&format!("upnp {ext}"));
                eprintln!("online: the router forwards UDP port {port} ({ext})");
                Some(f)
            }
            Err(e) => {
                status(&format!("upnp-failed {e}"));
                eprintln!("online: UPnP: {e}");
                None
            }
        },
        None => None,
    };
    let mapping = listener.public_address(Duration::from_millis(1500));
    if let Some(public) = mapping.public {
        // with the port forwarded, the router's port is the game's own
        let shown = if forwarded.is_some() { std::net::SocketAddr::new(public.ip(), port) } else { public };
        status(&format!("public {shown}{}", if mapping.symmetric { " symmetric" } else { "" }));
    }
    // the joiners to let in (`punch ADDRESS` lines from the launcher on standard input), each for two minutes
    let punches = punch_requests();
    let mut letting_in: Vec<(std::net::SocketAddr, Instant)> = Vec::new();
    let request = loop {
        while let Ok(to) = punches.try_recv() {
            if to.port() == 0 {
                eprintln!("online: the launcher closed while waiting for a joiner");
                std::process::exit(0);
            }
            status(&format!("letting-in {to}"));
            letting_in.push((to, Instant::now() + Duration::from_secs(120)));
        }
        letting_in.retain(|(_, until)| Instant::now() < *until);
        for (to, _) in &letting_in {
            listener.punch(*to);
        }
        match listener.poll(Duration::from_millis(250)).map_err(|e| e.to_string())? {
            Some(qwc_net::Knock::Join(r)) => break r,
            Some(qwc_net::Knock::OtherBuild { from, build }) => {
                status(&format!("other-build {from} {build}"));
                eprintln!("online: {from} plays version {build}, this is {BUILD}: turned away");
            }
            None => {}
        }
    };
    status(&format!("joined {}", request.from));
    // the joiner's team, unless it is the host's (or none): then the host's choice of opponent
    let fallback =
        args.models.get(2).cloned().unwrap_or_else(|| if mine == "England" { "France" } else { "England" }.into());
    let asked = std::str::from_utf8(&request.ask).ok().and_then(|t| team(vfs, t)).filter(|t| *t != mine);
    let theirs = asked.or_else(|| team(vfs, &fallback)).ok_or_else(|| format!("no team folder teams/{fallback}"))?;
    let momentum = if qwc_settings::Settings::load().gameplay.momentum { qwc_sim::formation::MOMENTUM } else { 0.0 };
    let setup = Setup { teams: [mine, theirs], stadium, seed: seed(), momentum };
    eprintln!("online: {} against {} at {} (seed {})", setup.teams[0], setup.teams[1], setup.stadium, setup.seed);
    let welcomed = listener.welcome(request, &setup.to_bytes()).map_err(|e| e.to_string())?;
    let (play_setup, m) = build(args, vfs, &setup);
    let pending = Pending::Host { welcomed, start_sum: m.checksum(), delay: args.delay };
    run(args, vfs, play_setup, m, Online::new(0, pending));
    drop(forwarded);
    Ok(())
}

/// The addresses the launcher asks the host to let in: `punch ADDRESS` lines on standard input, read on a thread.
/// Started by the launcher (`QWC_FROM_LAUNCHER`), the host leaves while still waiting when its standard input ends:
/// the launcher has closed, and nobody is left to stop it.
fn punch_requests() -> std::sync::mpsc::Receiver<std::net::SocketAddr> {
    let (tx, rx) = std::sync::mpsc::channel();
    let launched = std::env::var_os("QWC_FROM_LAUNCHER").is_some();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines().map_while(Result::ok) {
            let Some(to) = line.trim().strip_prefix("punch ") else { continue };
            match qwc_net::resolve(to, qwc_net::DEFAULT_PORT) {
                Ok(a) => {
                    if tx.send(a).is_err() {
                        return;
                    }
                }
                Err(e) => status(&format!("letting-in-failed {e}")),
            }
        }
        if launched {
            // the channel dropped means a joiner came: the match goes on
            let _ = tx.send(std::net::SocketAddr::from(([0, 0, 0, 0], 0)));
        }
    });
    rx
}

fn join(args: &Args, vfs: &'static Vfs) -> Result<(), String> {
    let to = args.models.get(1).ok_or("join: the host's address")?;
    let ask = args.models.get(2).cloned().unwrap_or_default();
    status(&format!("joining {to}"));
    let joining = Joining::bind(to, qwc_net::DEFAULT_PORT).map_err(|e| e.to_string())?;
    // this side's public address, for the host to let in when its router drops the hello
    if !joining.peer().ip().is_loopback() {
        let mapping = joining.public_address(Duration::from_millis(1500));
        if let Some(public) = mapping.public {
            status(&format!("public {public}{}", if mapping.symmetric { " symmetric" } else { "" }));
        }
    }
    let bytes = joining.hello(BUILD, ask.as_bytes(), qwc_net::JOIN_WAIT).map_err(|e| e.to_string())?;
    let setup = Setup::from_bytes(&bytes).map_err(|e| format!("the host's setup: {e}"))?;
    status(&format!("connected {}", joining.peer()));
    eprintln!("online: {} against {} at {}", setup.teams[0], setup.teams[1], setup.stadium);
    for t in &setup.teams {
        team(vfs, t).ok_or_else(|| format!("the host's team {t} is not in this game's files"))?;
    }
    let (play_setup, m) = build(args, vfs, &setup);
    let pending = Pending::Join { joining, start_sum: m.checksum() };
    run(args, vfs, play_setup, m, Online::new(1, pending));
    Ok(())
}

/// `play`'s setup for the online `setup`, and its match: both teams humans', the host's seed and momentum.
fn build(args: &Args, vfs: &Vfs, setup: &Setup) -> (crate::play::Setup, Match) {
    let s = crate::play::Setup::new(args, vfs, setup.teams.clone(), setup.stadium.clone());
    let mut m = s.start(vfs, setup.seed);
    m.momentum = setup.momentum;
    m.set_human(0, true);
    m.set_human(1, true);
    (s, m)
}

/// The match in the window, or headless with `--out`.
fn run(args: &Args, vfs: &'static Vfs, setup: crate::play::Setup, m: Match, online: Online) {
    match &args.out {
        Some(out) => headless(args, m, online, out),
        None => crate::play::run(args, vfs, setup, m, Some(online)),
    }
}

/// Plays without a window to `--time` seconds of match time (default 60), both sides' hands scripted; writes the
/// score, the frames, the checksum and the round trip to `out`.
fn headless(args: &Args, mut m: Match, mut online: Online, out: &std::path::Path) {
    if let Err(e) = online.connect() {
        status(&format!("error {e}"));
        fail(e);
    }
    let secs = if args.time > 0.0 { args.time } else { 60.0 };
    let ticks = (secs * 24.0) as u32;
    let seat = online.seat;
    let mut bot = Bot::new(seat);
    let mut restart = crate::restart::Restart::default();
    let mut next = Instant::now();
    let mut frames = 0u32;
    // `QWC_BOT_PAUSE=F1,F2,...`: the scripted player asks for the pause on those frames (checks)
    let pauses: Vec<u32> =
        std::env::var("QWC_BOT_PAUSE").unwrap_or_default().split(',').filter_map(|v| v.trim().parse().ok()).collect();
    // the match time to play to, or as many frames (paused frames run no match time)
    while m.tick < ticks && frames < ticks * 4 && !m.finished {
        online.poll();
        // the other player leaving ends the match as it stands (its result written); anything else is a failure
        match online.session.as_ref().and_then(Session::end) {
            Some(qwc_net::End::Left(_)) => break,
            Some(end) => fail(format!("online: {end}")),
            None => {}
        }
        if Instant::now() < next {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        let delay = online.session.as_ref().map_or(0, Session::delay);
        if pauses.contains(&frames) {
            online.ask_pause();
        }
        let Some(frame) = online.take(|control| bot.frame(frames + delay, control)) else {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
        next = next.max(Instant::now() - qwc_net::TICK) + online.tick_length();
        online.run(&mut m, &mut restart, &frame);
        frames += 1;
    }
    // a moment more, so the other side has our last frames and checksums
    let s = online.session.as_mut().unwrap();
    let until = Instant::now() + Duration::from_millis(500);
    while Instant::now() < until {
        s.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    let end = s.end().map(|e| e.to_string());
    let rtt = s.rtt().map_or(0, |r| r.as_millis());
    let report = format!(
        "seat {seat}\nframes {frames}\ntick {}\nscore {} {}\nchecksum {:016x}\nrtt_ms {rtt}\ndelay {}\nend {}\n",
        m.tick,
        m.teams[0].score,
        m.teams[1].score,
        m.checksum(),
        s.delay(),
        end.as_deref().unwrap_or("none"),
    );
    online.leave();
    std::fs::write(out, &report).unwrap_or_else(|e| fail(format!("{}: {e}", out.display())));
    status(&format!("end {} {}", m.teams[0].score, m.teams[1].score));
    eprint!("{report}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_reads_back_as_the_f32s_both_sides_use() {
        let input = Input {
            stick: [0.25, 0.0, -0.1],
            stick2: [-1.0, 0.0, 0.5],
            raw: [0.3, 0.0, 0.7],
            action_pressed: true,
            shoot_held: true,
            special: true,
            dodge_pressed: true,
            combo_right_held: true,
            uber_pressed: true,
            ..Input::default()
        };
        let seeker = SeekerPad { stick: [0.5, -0.75], boost_held: true, boost_pressed: false, grab_held: true };
        let b = pack(&input, &seeker, PAUSE);
        assert_eq!(b.len(), FRAME_LEN);
        let f = unpack(&b);
        assert_eq!(f.control, PAUSE);
        assert_eq!(f.seeker, seeker);
        // -0.1 and 0.3 are not f32s: what comes back is the f32 nearest, the same on both machines
        assert_eq!(f.input.stick, [0.25, 0.0, f64::from(-0.1f32)]);
        assert_eq!(f.input.raw, [f64::from(0.3f32), 0.0, f64::from(0.7f32)]);
        assert_eq!(f.input, Input { stick: f.input.stick, raw: f.input.raw, ..input });
        // packing what came back gives the same bytes
        assert_eq!(pack(&f.input, &f.seeker, f.control), b);
        assert_eq!(unpack(&b[1..]), FrameInput::default());
    }

    #[test]
    fn every_button_has_its_own_bit() {
        let all = Input {
            action_pressed: true,
            action_held: true,
            shoot_pressed: true,
            shoot_held: true,
            special: true,
            boost_pressed: true,
            boost_held: true,
            dodge_pressed: true,
            special_pressed: true,
            special_held: true,
            combo_left_pressed: true,
            combo_left_held: true,
            combo_right_pressed: true,
            combo_right_held: true,
            uber_pressed: true,
            ..Input::default()
        };
        assert_eq!(unpack(&pack(&all, &SeekerPad::default(), 0)).input, all);
        let b = pack(&all, &SeekerPad::default(), 0);
        assert_eq!(u16::from_le_bytes([b[25], b[26]]), 0x7fff);
    }

    #[test]
    fn the_setup_reads_back() {
        let s = Setup { teams: ["England".into(), "Japan".into()], stadium: "japan".into(), seed: 99, momentum: 0.0 };
        assert_eq!(Setup::from_bytes(&s.to_bytes()), Ok(s));
        assert!(Setup::from_bytes(b"(nonsense").is_err());
    }
}
