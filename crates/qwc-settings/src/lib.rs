//! The player's settings (`docs/LAUNCHER.md`): one `settings.ron`, read and written by the launcher's Settings page
//! and by the game, so whichever changed it last wins. Not the original's (the original kept its options in the
//! memory card and the registry).
//!
//! The file is in the config folder ([`path`]): `%APPDATA%\QWCRemastered\settings.ron` on Windows,
//! `$XDG_CONFIG_HOME/qwc-remastered/settings.ron` (`~/.config/...`) elsewhere, `$QWC_SETTINGS` instead when set. A
//! setting the file leaves out keeps its default, so older files still read.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How the game's window is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowMode {
    #[default]
    Windowed,
    /// A window without borders covering the monitor (at the monitor's own resolution).
    Borderless,
    /// The monitor switched to the resolution chosen (exclusive fullscreen); borderless when it has no such mode.
    Fullscreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Display {
    /// The window's size in pixels (the picture's resolution in fullscreen).
    pub width: u32,
    pub height: u32,
    pub mode: WindowMode,
    /// Frames shown in step with the monitor's refresh (no tearing).
    pub vsync: bool,
}

impl Default for Display {
    fn default() -> Display {
        Display { width: 1280, height: 720, mode: WindowMode::Windowed, vsync: true }
    }
}

/// How the matches and the cutscenes are lit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Look {
    /// The remaster's lighting: the sun with shadows, fog, HDR.
    #[default]
    Remastered,
    /// As the original drew them.
    Original,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Graphics {
    pub look: Look,
    /// The packs' remastered movies in place of the disc's.
    pub remastered_movies: bool,
    /// The packs' big pictures as BC7 on the GPU: a quarter of the memory, a little softer.
    pub compress_textures: bool,
}

impl Default for Graphics {
    fn default() -> Graphics {
        Graphics { look: Look::Remastered, remastered_movies: true, compress_textures: true }
    }
}

/// The sound's levels, 0 to 1 (linear gain). The music, effects and speech are each also scaled by the master.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sound {
    pub master: f32,
    pub music: f32,
    pub effects: f32,
    pub speech: f32,
}

impl Default for Sound {
    fn default() -> Sound {
        Sound { master: 1.0, music: 1.0, effects: 1.0, speech: 1.0 }
    }
}

impl Sound {
    /// The music's, the effects' and the speech's gains with the master applied, each kept in 0..=1.
    pub fn gains(&self) -> [f32; 3] {
        let m = self.master.clamp(0.0, 1.0);
        [self.music, self.effects, self.speech].map(|g| g.clamp(0.0, 1.0) * m)
    }
}

/// The launcher's own: whether it looks for a new release when it opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Updates {
    pub check_on_start: bool,
}

impl Default for Updates {
    fn default() -> Updates {
        Updates { check_on_start: true }
    }
}

/// How the game plays where the remaster differs from the original.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gameplay {
    /// The players flying the formations ease into their turns and changes of pace (`qwc_sim::formation::glide`);
    /// off, they fly as the original's do, turning round on the spot.
    pub momentum: bool,
}

impl Default for Gameplay {
    fn default() -> Gameplay {
        Gameplay { momentum: true }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub display: Display,
    pub graphics: Graphics,
    pub sound: Sound,
    pub gameplay: Gameplay,
    pub updates: Updates,
    /// The language the game starts in: a name of `qwc_formats::sts::LANGUAGES` (`French`, `SimChina`); none for US
    /// English, as the original starts. The game's own language menus change it for the session.
    pub language: Option<String>,
}

/// The settings file: `$QWC_SETTINGS`, else `settings.ron` in the config folder (none in an environment without one).
pub fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("QWC_SETTINGS") {
        return Some(p.into());
    }
    let dir = if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?).join("QWCRemastered")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?
            .join("qwc-remastered")
    };
    Some(dir.join("settings.ron"))
}

impl Settings {
    pub fn parse(text: &str) -> Result<Settings, String> {
        ron::from_str(text).map_err(|e| e.to_string())
    }

    pub fn to_text(&self) -> String {
        let pretty = ron::ser::PrettyConfig::new().struct_names(true);
        ron::ser::to_string_pretty(self, pretty).expect("settings always serialise") + "\n"
    }

    /// The settings at `path`: the defaults when there is no file; an error when it does not read.
    pub fn read(path: &Path) -> Result<Settings, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Settings::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn write(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        // written beside, then moved over, so the game never reads half a file
        let part = path.with_extension("ron.part");
        std::fs::write(&part, self.to_text()).map_err(|e| format!("{}: {e}", part.display()))?;
        std::fs::rename(&part, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The player's settings ([`path`]), the defaults when there are none or they do not read (said why).
    pub fn load() -> Settings {
        path().map_or_else(Settings::default, |p| {
            Settings::read(&p).unwrap_or_else(|e| {
                eprintln!("{e}; the default settings are used");
                Settings::default()
            })
        })
    }

    /// Write the player's settings ([`path`]).
    pub fn save(&self) -> Result<(), String> {
        self.write(&path().ok_or("no config folder")?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_back_and_keeps_defaults_for_what_is_left_out() {
        let s = Settings {
            display: Display { width: 1920, height: 1080, mode: WindowMode::Borderless, vsync: false },
            graphics: Graphics { look: Look::Original, remastered_movies: false, compress_textures: false },
            sound: Sound { master: 0.8, music: 0.5, effects: 1.0, speech: 0.25 },
            gameplay: Gameplay { momentum: false },
            updates: Updates { check_on_start: false },
            language: Some("French".into()),
        };
        assert_eq!(Settings::parse(&s.to_text()).unwrap(), s);
        assert_eq!(Settings::parse("Settings()").unwrap().language, None);
        assert!(Settings::parse("Settings()").unwrap().gameplay.momentum);
        let partial = Settings::parse("Settings(display: Display(width: 2560, height: 1440))").unwrap();
        assert_eq!((partial.display.width, partial.display.vsync, partial.sound), (2560, true, Sound::default()));
        assert!(Settings::parse("Settings(display: 3)").is_err());
    }

    #[test]
    fn gains_take_the_master() {
        let s = Sound { master: 0.5, music: 1.0, effects: 0.5, speech: 2.0 };
        assert_eq!(s.gains(), [0.5, 0.25, 0.5]);
    }

    #[test]
    fn a_missing_file_is_the_defaults() {
        let dir = std::env::temp_dir().join(format!("qwc-settings-{}", std::process::id()));
        let p = dir.join("settings.ron");
        assert_eq!(Settings::read(&p).unwrap(), Settings::default());
        let s = Settings { sound: Sound { music: 0.3, ..Sound::default() }, ..Settings::default() };
        s.write(&p).unwrap();
        assert_eq!(Settings::read(&p).unwrap(), s);
        let _ = std::fs::remove_dir_all(dir);
    }
}
