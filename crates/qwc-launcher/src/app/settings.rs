//! The Settings page: the display (resolution, window mode, vsync), the graphics (the look, the remastered movies),
//! the sound's levels and the language the game starts in, written to the player's `settings.ron` (`qwc_settings`)
//! as they change. The game reads it when it starts. The controls are the original's key bindings (`qwc_feck::keys`), in its own file beside the save,
//! which the game's Controls menu edits too.

use eframe::egui::{self, RichText};
use qwc_feck::keys::{self, Keys};
use qwc_settings::{Look, Settings, WindowMode};

use super::Launcher;
use crate::style;

/// The resolutions offered (16:9 and the common others); the monitor's own is added when it is not one of them.
const RESOLUTIONS: &[(u32, u32)] =
    &[(1280, 720), (1366, 768), (1600, 900), (1920, 1080), (2560, 1080), (2560, 1440), (3440, 1440), (3840, 2160)];

/// What the page holds: the settings as last read or written, where they are, and what the last write said.
pub(super) struct SettingsView {
    pub(super) settings: Settings,
    path: Option<std::path::PathBuf>,
    message: Option<(bool, String)>,
    /// The key bindings, read from their file beside the save (the defaults without one).
    pub(super) keys: Keys,
}

/// The languages the imported game has (`language/<FOLDER>.ccd`, or the folder): each as
/// `qwc_formats::sts::LANGUAGES` lists it (folder, hash name, English name).
fn languages(data: Option<&std::path::Path>) -> Vec<(&'static str, &'static str, &'static str)> {
    let Some(dir) = data.map(|d| d.join("language")) else { return Vec::new() };
    let names: Vec<String> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_ascii_uppercase())
        .collect();
    qwc_formats::sts::LANGUAGES
        .iter()
        .copied()
        .filter(|(folder, _, _)| names.iter().any(|n| n == folder || *n == format!("{folder}.CCD")))
        .collect()
}

/// The key bindings' file: `KeyConfig data` in the save's folder.
fn keys_file() -> Option<std::path::PathBuf> {
    crate::saves::save_path().and_then(|p| Some(p.parent()?.join(keys::FILE_NAME)))
}

impl SettingsView {
    /// The player's settings, read now (the defaults when there are none; said when they do not read).
    pub(super) fn load() -> SettingsView {
        let path = qwc_settings::path();
        let (settings, message) = match path.as_deref().map(Settings::read) {
            Some(Ok(s)) => (s, None),
            Some(Err(e)) => (Settings::default(), Some((false, format!("{e}; the defaults are shown")))),
            None => (Settings::default(), Some((false, "No config folder in this environment.".into()))),
        };
        let keys = keys_file().map(Keys::with_file).unwrap_or_default();
        SettingsView { settings, path, message, keys }
    }

    /// Write the key bindings to their file (its folder made when the game has not saved yet).
    fn save_keys(&mut self) {
        let made = self.keys.file.as_ref().and_then(|f| f.parent()).map(std::fs::create_dir_all);
        self.message = Some(match (made, self.keys.save()) {
            (Some(Ok(())), true) => (true, "Controls saved. The game uses them the next time it starts.".into()),
            (Some(Err(e)), _) => (false, e.to_string()),
            _ => (false, "The controls could not be saved.".into()),
        });
    }

    pub(super) fn save(&mut self) {
        self.message = Some(match self.path.as_deref().map(|p| self.settings.write(p)) {
            Some(Ok(())) => (true, "Saved. The game uses them the next time it starts.".into()),
            Some(Err(e)) => (false, e),
            None => (false, "No config folder in this environment.".into()),
        });
    }
}

/// A slider for a level (0 to 1, shown in percent); whether it changed.
fn level(ui: &mut egui::Ui, name: &str, value: &mut f32) -> bool {
    let mut pct = (*value * 100.0).round();
    let r = ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%").step_by(1.0).fixed_decimals(0).text(name));
    if r.changed() {
        *value = pct / 100.0;
    }
    r.drag_stopped() || (r.changed() && !r.dragged())
}

impl Launcher {
    pub(super) fn settings(&mut self, ui: &mut egui::Ui) {
        let art = self.art.clone();
        let monitor = ui.ctx().input(|i| i.viewport().monitor_size).map(|s| {
            let ppp = ui.ctx().pixels_per_point();
            ((s.x * ppp).round() as u32, (s.y * ppp).round() as u32)
        });
        let languages = languages(self.profile.data.as_deref());
        let v = &mut self.settings_view;
        let mut changed = false;
        ui.label(style::heading("Settings", 36.0));
        ui.label(RichText::new("Saved as you change them; the game reads them when it starts.").color(style::DIM));
        ui.add_space(8.0);

        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            ui.label(style::heading("Display", 23.0));
            let d = &mut v.settings.display;
            let mut sizes: Vec<(u32, u32)> = RESOLUTIONS.to_vec();
            for extra in monitor.into_iter().chain([(d.width, d.height)]) {
                if !sizes.contains(&extra) {
                    sizes.push(extra);
                }
            }
            sizes.sort();
            let name = |s: (u32, u32)| {
                let own = if Some(s) == monitor { "  (this monitor)" } else { "" };
                format!("{} x {}{own}", s.0, s.1)
            };
            let mut size = (d.width, d.height);
            style::dropdown(&art, egui::ComboBox::from_label("Resolution")).selected_text(name(size)).show_ui(
                ui,
                |ui| {
                    for s in &sizes {
                        ui.selectable_value(&mut size, *s, name(*s));
                    }
                },
            );
            if size != (d.width, d.height) {
                (d.width, d.height) = size;
                changed = true;
            }
            ui.add_space(4.0);
            for (mode, label, what) in [
                (WindowMode::Windowed, "Windowed", "A window of that size."),
                (WindowMode::Borderless, "Borderless", "A window covering the monitor, at the monitor's resolution."),
                (WindowMode::Fullscreen, "Fullscreen", "The monitor switched to that resolution."),
            ] {
                ui.horizontal(|ui| {
                    changed |= ui.radio_value(&mut d.mode, mode, label).changed();
                    ui.label(RichText::new(what).small().color(style::DIM));
                });
            }
            changed |= ui.checkbox(&mut d.vsync, "Vsync: frames in step with the monitor (no tearing)").changed();
        });
        ui.add_space(8.0);

        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            ui.label(style::heading("Graphics", 23.0));
            let g = &mut v.settings.graphics;
            for (look, label, what) in [
                (Look::Remastered, "Remastered lighting", "The sun with shadows, fog and HDR."),
                (Look::Original, "Original lighting", "The matches lit as the original lit them."),
            ] {
                ui.horizontal(|ui| {
                    changed |= ui.radio_value(&mut g.look, look, label).changed();
                    ui.label(RichText::new(what).small().color(style::DIM));
                });
            }
            changed |= ui
                .checkbox(&mut g.remastered_movies, "Remastered movies, where a pack has them (else the disc's)")
                .changed();
            changed |= ui
                .checkbox(
                    &mut g.compress_textures,
                    "Compress the packs' big textures (a quarter of the video memory, a little softer)",
                )
                .changed();
        });
        ui.add_space(8.0);

        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            ui.label(style::heading("Gameplay", 23.0));
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut v.settings.gameplay.momentum, "Momentum").changed();
                ui.label(
                    RichText::new("The other players ease into their turns; off, they turn as the original's do.")
                        .small()
                        .color(style::DIM),
                );
            });
        });
        ui.add_space(8.0);

        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            ui.label(style::heading("Sound", 23.0));
            let s = &mut v.settings.sound;
            changed |= level(ui, "Master", &mut s.master);
            changed |= level(ui, "Music", &mut s.music);
            changed |= level(ui, "Sound effects", &mut s.effects);
            changed |= level(ui, "Commentary and speech", &mut s.speech);
        });
        ui.add_space(8.0);

        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            ui.label(style::heading("Language", 23.0));
            let us = "English (US), as the game starts";
            let shown = |l: &Option<String>| match l.as_deref().and_then(qwc_formats::sts::language) {
                Some((_, _, english)) => english.to_string(),
                None => us.to_string(),
            };
            let mut picked = v.settings.language.clone();
            style::dropdown(&art, egui::ComboBox::from_label("The game's language"))
                .selected_text(shown(&picked))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut picked, None, us);
                    for (folder, name, english) in &languages {
                        if *folder != "US" {
                            ui.selectable_value(&mut picked, Some(name.to_string()), *english);
                        }
                    }
                });
            if picked != v.settings.language {
                v.settings.language = picked;
                changed = true;
            }
            ui.label(
                RichText::new(
                    "The menus' text and its fonts. The commentary stays in the languages the game has it in. The \
                     game's own language menu changes it for one session.",
                )
                .small()
                .color(style::DIM),
            );
            if languages.is_empty() {
                ui.label(RichText::new("Import the game to choose from its languages.").small().color(style::WARN));
            }
        });
        ui.add_space(8.0);

        let mut keys_changed = false;
        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            ui.label(style::heading("Controls", 23.0));
            ui.label(
                RichText::new(
                    "Two sets of keys, both moving your player in a match. Picking a key that another action uses \
                     takes it from there. Gamepads have their own buttons.",
                )
                .small()
                .color(style::DIM),
            );
            egui::Grid::new("keys").num_columns(3).spacing([16.0, 4.0]).show(ui, |ui| {
                ui.label("");
                ui.label(RichText::new("First set").strong());
                ui.label(RichText::new("Second set").strong());
                ui.end_row();
                for (action, name) in keys::ACTIONS {
                    let row = ui.label(name).id;
                    for player in 0..2u32 {
                        let current = v.keys.get(player, action);
                        let mut picked = current;
                        let combo = style::dropdown(&art, egui::ComboBox::from_id_salt(("key", action, player)))
                            .selected_text(keys::key_name(current))
                            .width(170.0)
                            .show_ui(ui, |ui| {
                                for (id, key) in keys::KEY_NAMES {
                                    ui.selectable_value(&mut picked, *id, *key);
                                }
                            });
                        combo.response.labelled_by(row);
                        if picked != current {
                            v.keys.rebind(player, action, picked);
                            keys_changed = true;
                        }
                    }
                    ui.end_row();
                }
            });
            if ui.button("Default controls").clicked() {
                v.keys.table = keys::DEFAULTS.to_vec();
                keys_changed = true;
            }
        });
        if keys_changed {
            v.save_keys();
        }
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            if ui.button("Back to the defaults").clicked() {
                v.settings = Settings::default();
                changed = true;
            }
            if let Some(p) = &v.path {
                ui.label(RichText::new(p.display().to_string()).small().color(style::DIM));
            }
        });
        if changed {
            v.save();
        }
        if let Some((ok, m)) = &v.message {
            ui.label(RichText::new(m).color(if *ok { style::GOOD } else { style::BAD }));
        }
    }
}

#[cfg(test)]
mod tests {
    /// The languages offered are the imported game's, in the remaster's order.
    #[test]
    fn languages_of_the_imported_game() {
        let d = std::env::temp_dir().join(format!("qwc-launcher-languages-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("language")).unwrap();
        for f in ["French.ccd", "US.ccd", "INDEX.SDX", "simchina.ccd"] {
            std::fs::write(d.join("language").join(f), "").unwrap();
        }
        let names: Vec<&str> = super::languages(Some(&d)).iter().map(|l| l.1).collect();
        assert_eq!(names, ["US", "French", "SimChina"]);
        assert!(super::languages(None).is_empty());
        let _ = std::fs::remove_dir_all(d);
    }

    /// The bindings' file is the original's, beside the save.
    #[test]
    fn the_keys_file_is_beside_the_save() {
        let (Some(f), Some(save)) = (super::keys_file(), crate::saves::save_path()) else { return };
        assert_eq!(f.parent(), save.parent());
        assert!(f.ends_with("KeyConfig data"));
    }
}
