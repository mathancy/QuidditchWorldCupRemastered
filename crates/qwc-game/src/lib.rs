//! `qwc-game`'s modules, as a library: the executable (`main.rs`) and the HUD editor (its exact preview of the front
//! end's screens, `Painter`) use them.

pub mod arena;
pub mod audio;
pub mod ccs;
pub mod commentary;
pub mod controls;
pub mod cutscene;
pub mod display;
pub mod face;
pub mod fe;
pub mod fe3d;
pub mod fematch;
pub mod fespeech;
pub mod flyer;
pub mod fx;
pub mod gamepad;
pub mod hud;
pub mod live;
pub mod looks;
pub mod matchaudio;
pub mod matchcam;
pub mod models;
pub mod movie;
pub mod online;
pub mod options;
pub mod packs;
pub mod play;
pub mod preview;
pub mod record;
pub mod restart;
pub mod scenery;
pub mod seekercam;
pub mod shots;
pub mod sim;
pub mod sound;
pub mod stadium;
pub mod textures;

use std::path::PathBuf;

pub struct Args {
    /// `--data`, else `$QWC_DATA`, else the profile's game data, else `original/cd_files`.
    pub data: PathBuf,
    /// `--profile`: the player's profile (`qwc_packs`), else `$QWC_PROFILE`, else the library's; `--no-packs`: none.
    pub profile: Option<PathBuf>,
    pub no_packs: bool,
    pub mods: Vec<PathBuf>,
    pub command: String,
    pub models: Vec<String>,
    pub anim: Option<String>,
    pub clip: Option<String>,
    pub out: Option<PathBuf>,
    pub stadium: Option<String>,
    pub time: f64,
    /// `--size`, else for a window the player's settings' (`qwc_settings`), else 1280 x 720.
    pub size: (u32, u32),
    /// For a window: windowed, borderless or fullscreen (`--window`, else the settings'), and vsync (`--no-vsync`).
    pub window: qwc_settings::WindowMode,
    pub vsync: bool,
    /// The sound's levels (the settings').
    pub sound: qwc_settings::Sound,
    /// For a window: the packs' big pictures compressed (`qwc_render::Renderer::compress`, the settings').
    pub compress: bool,
    /// `--language NAME`, else the settings' for a window: the language the front end starts in.
    pub language: Option<String>,
    /// `--keys`: scripted key presses for `frontend`.
    pub keys: Option<String>,
    /// `--exe`: a decrypted `QWC.exe` to read the load-set table from.
    pub exe: Option<PathBuf>,
    /// Where the remastered movies are, the first that has a movie wins: `--remastered DIR`, else the packs' `movies`.
    pub movies: Vec<PathBuf>,
    /// `--original-movies`: the disc's movies even where remastered ones are.
    pub original_movies: bool,
    /// `--openh264`: the remastered movies through OpenH264 even where the system has a decoder.
    openh264: bool,
    /// `--textures`: a remastered textures' list instead of the packs', and whether it was given.
    pub textures: PathBuf,
    pub textures_given: bool,
    /// `--disc-textures`: the disc's textures even where remastered ones are.
    pub disc_textures: bool,
    /// `--fonts`: a vector fonts' list instead of the packs' (without either, the disc's bitmap fonts are drawn),
    /// and whether it was given.
    pub fonts: PathBuf,
    pub fonts_given: bool,
    /// `--disc-fonts`: the disc's bitmap fonts even where vector ones are listed.
    pub disc_fonts: bool,
    /// `--watch`: `play` with both teams the AI's.
    pub watch: bool,
    /// `--port`: the UDP port `online host` listens on (`qwc_net::DEFAULT_PORT` when not given); `--delay`: the
    /// frames of input delay online (from the round trip when not given).
    pub port: Option<u16>,
    pub delay: Option<u32>,
    /// `--no-upnp`: `online host` does not ask the router to forward its port.
    pub no_upnp: bool,
    /// `--look`: how the match and the cutscenes are lit (`original` or `remastered`, the default).
    pub look: qwc_render::Look,
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
    pub look_at: Option<[f32; 3]>,
    pub skeleton: String,
    pub tex_from: String,
    pub use_embedded: bool,
}

pub fn usage() -> ! {
    eprintln!("{}", USAGE);
    std::process::exit(2)
}

const USAGE: &str = "usage:
  qwc-game [--data DIR] [--mods DIR] view MODEL... [--anim SET.fas] [--clip NAME|INDEX]
           [--skeleton SKL] [--tex-from MODEL.fpm] [--original-textures]    (edited .glb files; defaults:
           sma_common, Potter, use the textures embedded in the file)
  qwc-game [--data DIR] [--mods DIR] shot MODEL... --out FILE.png [--anim SET.fas] [--clip NAME|INDEX] [--time SECS]
           [--size WxH] [--yaw DEG] [--pitch DEG] [--zoom F]
  qwc-game [--data DIR] [--mods DIR] list EXT
  qwc-game [--data DIR] [--mods DIR] sound event CATEGORY.map ID | bank BANK INDEX | speech LANGUAGE NAME  [--out FILE.wav]
  qwc-game [--data DIR] [--mods DIR] music SCORE [EVENT] [--time SECS] [--out FILE.wav]
  qwc-game [--data DIR] [--mods DIR] sim TEAM0.cli TEAM1.cli [--stadium NAME] [--time SECS] [--out REPLAY.json]
  qwc-game [--data DIR] [--mods DIR] commentary TEAM0 TEAM1 [--stadium NAME] [--time SECS]
  qwc-game [--data DIR] [--mods DIR] play [TEAM0 TEAM1] [--stadium NAME] [--size WxH] [--watch]
           [--time SECS --out FILE.png]   (a match in a window, teams by folder, default England and France at
           england; you play the first unless --watch: arrows or WASD fly, Numpad 5 / J / left mouse is Action (pass,
           or tackle without the ball), Numpad 6 / K / right mouse shoots (held in the goal area), Numpad 4 / H
           boosts, P pauses; with --out, both AI, drawn headless --time seconds in)
           The match and the cutscenes draw the remastered textures listed in --textures as well, and are lit
           as --look says: remastered (the default: the sun with shadows, fog, HDR; QWC_REMASTER=name=value,...
           changes its settings, qwc_render::remaster::Settings) or original.
  qwc-game [--data DIR] [--mods DIR] online host [TEAM [OPPONENT]] [--stadium NAME] [--port N] [--delay FRAMES]
           [--no-upnp]
  qwc-game [--data DIR] [--mods DIR] online join ADDRESS[:PORT] [TEAM]
           (a match against another player over the network, no server: the host listens on UDP port 20030 unless
           --port and plays TEAM (England), the joiner plays the team it asks for (else OPPONENT); both need the same
           version and game files. The host asks its router to forward the port (UPnP) unless --no-upnp, and lets
           in the joiners whose addresses come as `punch ADDRESS` lines on its standard input. P pauses both games,
           Escape leaves. With --time SECS --out FILE: headless, the players scripted, the result written to FILE)
  qwc-game [--data DIR] [--mods DIR] textures [--stadium NAME] [--out DIR]   (a stadium's textures as PNGs to paint,
           lightmaps apart, and the list that draws them: --textures DIR/textures.txt; default
           work/textures/NAME; PNGs already there are kept)
  qwc-game [--data DIR] [--mods DIR] fe SCRIPT.vso --out FILE.png [--time SECS] [--size WxH]   (one front-end layer)
  qwc-game [--data DIR] [--mods DIR] screen NODE|boot --out FILE.png [--time SECS] [--size WxH]   (the flow from a
           node, or booted with presses)
  qwc-game [--data DIR] [--mods DIR] frontend [NODE|boot] [--size WxH] [--keys SECS:KEY,...] [--time SECS]
           [--out FILE.png] [--remastered DIR] [--original-movies] [--openh264]   (the front end live: arrows,
           Enter, Escape or Backspace, Tab, Space; F12 saves the frame; --keys presses keys at those times, --time
           quits then, --out is where frames are saved; the movies in --remastered, default the packs', play
           in place of the disc's of the same name unless --original-movies, through the system's decoder where
           there is one, else or with --openh264 through OpenH264)
           The front-end commands (fe, screen, frontend) draw the packs' remastered textures (or the list given
           with --textures) in place of the disc's, unless --disc-textures, and the text in the vector fonts of the
           packs' fonts.txt (or the list given with --fonts), unless --disc-fonts. Without a pack on, the screens,
           the fonts and the HUD are the original's, and every stadium has the default look.
  qwc-game [--data DIR] [--mods DIR] cutscene NAME [TEAM OTHER] [--stadium NAME] --time SECS --out FILE.png   (a frame of a
           cutscene of root/cs, TEAM1 and TEAM2 the team folders, default Gryff and Slyth)
  qwc-game [--data DIR] [--mods DIR] movie FILE.tgq --out FILE.png|.yuv|.wav [--time SECS]   (a movie's picture,
           all its pictures as raw YUV 4:2:0, or its sound)
  The windows (frontend, play) take their size, window mode, vsync, look and movies from the player's settings
  (qwc_settings: settings.ron beside the profile; the launcher's Settings page) unless given here: --size WxH,
  --language NAME: the language the front end starts in (French, German, SimChina, ...: qwc_formats::sts::LANGUAGES;
  for a window the settings' when not given).
  --window windowed|borderless|fullscreen, --no-vsync, --look, --original-movies. The sound's levels are always
  the settings'.
  qwc-game export MODEL... [--out DIR]   (each model as a .glb to edit, finest level, its skeleton, morphs named
           morphNN and textures embedded, at its pack slot's path under DIR, default work/export)
  qwc-game pack list | check DIR | build DIR [--out FILE|DIR] | install FILE.qwcpack | new DIR ID NAME
           | enable ID | disable ID | top ID | migrate [FROM]   (resource packs, docs/LAUNCHER.md: what the profile
           turns on and where packs overlap; a pack folder checked; zipped into a .qwcpack; installed into the
           packs' folder, on and at the top; a new pack folder; turned on or off or moved to the top; migrate
           makes packs of a remastered/ folder: local.remastered, and local.hogsum-test (off) for its stadiums)
Every command reads the packs the player's profile turns on: --profile FILE, else $QWC_PROFILE, else profile.ron in
the library (%APPDATA%\\QWCRemastered, ~/.local/share/qwc-remastered; $QWC_LIBRARY elsewhere); --no-packs reads
none. The profile's game data and exe stand in for --data and --exe when neither is given. Each pack's disc/ is
laid over the game data, under --mods.";

pub fn parse_args() -> Args {
    let env_data = std::env::var_os("QWC_DATA").map(PathBuf::from);
    let mut data_given = env_data.is_some();
    let mut movies_given = false;
    let (mut size_given, mut look_given, mut window_given) = (false, false, false);
    let mut a = Args {
        data: env_data.unwrap_or_else(|| "original/cd_files".into()),
        profile: None,
        no_packs: false,
        mods: vec![],
        command: String::new(),
        models: vec![],
        anim: None,
        clip: None,
        out: None,
        stadium: None,
        time: 0.0,
        size: (1280, 720),
        window: qwc_settings::WindowMode::Windowed,
        vsync: true,
        sound: qwc_settings::Sound::default(),
        compress: false,
        language: None,
        keys: None,
        exe: None,
        movies: vec![],
        original_movies: false,
        openh264: false,
        textures: "remastered/textures.txt".into(),
        textures_given: false,
        disc_textures: false,
        fonts: "remastered/fonts.txt".into(),
        fonts_given: false,
        disc_fonts: false,
        watch: false,
        port: None,
        delay: None,
        no_upnp: false,
        look: qwc_render::Look::Remastered,
        yaw: 30.0,
        pitch: 15.0,
        zoom: 1.0,
        look_at: None,
        skeleton: "frontend/Common/Misc/sma_common.skl".into(),
        tex_from: "root/csmodels/potter/potter.fpm".into(),
        use_embedded: true,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| usage());
        match arg.as_str() {
            "--data" => {
                a.data = val().into();
                data_given = true;
            }
            "--profile" => a.profile = Some(val().into()),
            "--no-packs" => a.no_packs = true,
            "--mods" => a.mods.push(val().into()),
            "--anim" => a.anim = Some(val()),
            "--clip" => a.clip = Some(val()),
            "--keys" => a.keys = Some(val()),
            "--exe" => a.exe = Some(val().into()),
            "--remastered" => {
                a.movies = vec![val().into()];
                movies_given = true;
            }
            "--original-movies" => a.original_movies = true,
            "--openh264" => a.openh264 = true,
            "--textures" => {
                a.textures = val().into();
                a.textures_given = true;
            }
            "--disc-textures" => a.disc_textures = true,
            "--fonts" => {
                a.fonts = val().into();
                a.fonts_given = true;
            }
            "--disc-fonts" => a.disc_fonts = true,
            "--watch" => a.watch = true,
            "--port" => a.port = Some(val().parse().unwrap_or_else(|_| usage())),
            "--delay" => a.delay = Some(val().parse().unwrap_or_else(|_| usage())),
            "--no-upnp" => a.no_upnp = true,
            "--look" => {
                a.look = qwc_render::Look::parse(&val()).unwrap_or_else(|| usage());
                look_given = true;
            }
            "--language" => a.language = Some(val()),
            "--window" => {
                a.window = match val().to_ascii_lowercase().as_str() {
                    "windowed" => qwc_settings::WindowMode::Windowed,
                    "borderless" => qwc_settings::WindowMode::Borderless,
                    "fullscreen" => qwc_settings::WindowMode::Fullscreen,
                    _ => usage(),
                };
                window_given = true;
            }
            "--no-vsync" => a.vsync = false,
            "--out" => a.out = Some(val().into()),
            "--stadium" => a.stadium = Some(val()),
            "--time" => a.time = val().parse().unwrap_or_else(|_| usage()),
            "--yaw" => a.yaw = val().parse().unwrap_or_else(|_| usage()),
            "--pitch" => a.pitch = val().parse().unwrap_or_else(|_| usage()),
            "--skeleton" => a.skeleton = val(),
            "--tex-from" => a.tex_from = val(),
            "--original-textures" => a.use_embedded = false,
            "--look-at" => {
                let v: Vec<f32> = val().split(',').filter_map(|x| x.trim().parse().ok()).collect();
                a.look_at = Some(<[f32; 3]>::try_from(v).unwrap_or_else(|_| usage()));
            }
            "--zoom" => a.zoom = val().parse().unwrap_or_else(|_| usage()),
            "--size" => {
                let v = val();
                let (w, h) = v.split_once('x').unwrap_or_else(|| usage());
                a.size = (w.parse().unwrap_or_else(|_| usage()), h.parse().unwrap_or_else(|_| usage()));
                size_given = true;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0)
            }
            s if s.starts_with("--") => usage(),
            s if a.command.is_empty() => a.command = s.to_string(),
            s => a.models.push(s.to_string()),
        }
    }
    if a.command.is_empty()
        || (a.models.is_empty() && !matches!(a.command.as_str(), "frontend" | "play" | "pads" | "textures"))
    {
        usage()
    }
    // the packs the profile turns on, and its game data and exe where the command line names none
    let packs = qwc_packs::init(a.profile.as_deref(), a.no_packs);
    if !data_given {
        if let Some(d) = &packs.profile.data {
            a.data = d.clone();
        }
    }
    if a.exe.is_none() {
        a.exe = packs.profile.exe.clone();
    }
    if !movies_given {
        a.movies = packs.movie_dirs();
    }
    // the player's settings: the sound always, the display and the look for a window (headless pictures keep the
    // defaults, so they come out the same on every machine)
    let settings = qwc_settings::Settings::load();
    a.sound = settings.sound;
    let window = matches!(a.command.as_str(), "frontend" | "play" | "online") && a.out.is_none();
    if window {
        let d = settings.display;
        if !size_given {
            a.size = (d.width.max(320), d.height.max(240));
        }
        if !window_given {
            a.window = d.mode;
        }
        a.vsync &= d.vsync;
        if !look_given {
            a.look = match settings.graphics.look {
                qwc_settings::Look::Remastered => qwc_render::Look::Remastered,
                qwc_settings::Look::Original => qwc_render::Look::Original,
            };
        }
        a.original_movies |= !settings.graphics.remastered_movies;
        a.compress = settings.graphics.compress_textures;
        if a.language.is_none() {
            a.language = settings.language.clone();
        }
        eprintln!(
            "window: {}x{} {:?}, vsync {}, {:?} look, {} movies",
            a.size.0,
            a.size.1,
            a.window,
            if a.vsync { "on" } else { "off" },
            a.look,
            if a.original_movies { "the disc's" } else { "remastered" }
        );
    }
    a
}

/// The renderer's texture compression for `args` (`qwc_render::Renderer::compress`): on for a window when the
/// settings say so, its blocks kept in the library's `cache/bc7`.
pub fn compression(args: &Args) -> Option<Option<PathBuf>> {
    compression_when(args.compress)
}

/// The renderer's texture compression when `on`, or when `QWC_COMPRESS=1` (headless too, to look at it).
pub fn compression_when(on: bool) -> Option<Option<PathBuf>> {
    let on = on || std::env::var_os("QWC_COMPRESS").is_some_and(|v| v == "1");
    on.then(|| qwc_packs::library_dir().map(|d| d.join("cache").join("bc7")))
}

pub fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("qwc-game: {msg}");
    std::process::exit(1)
}
