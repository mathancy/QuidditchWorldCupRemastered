//! The Saves page: the game's save (its slots, whether it is intact), copies of it in the library's `backups/`
//! (made before each Play and on demand), restored or deleted after asking (`crate::saves`).

use std::path::PathBuf;

use eframe::egui::{self, RichText};

use super::Launcher;
use crate::saves::{self, Backup, Save};
use crate::style;

/// What the page shows, read again after each change.
#[derive(Default)]
pub(super) struct SavesView {
    read: bool,
    save: Option<Result<Save, String>>,
    modified: Option<String>,
    backups: Vec<Backup>,
    /// The Gameplay packs the save's progress was made under, and the newest matches played.
    rules: Vec<(String, String)>,
    matches: Vec<saves::Played>,
    /// A question waiting for its answer, and the last thing done.
    asking: Option<Ask>,
    message: Option<(bool, String)>,
}

#[derive(Clone)]
enum Ask {
    Restore(Backup),
    DeleteBackup(Backup),
    DeleteSave,
}

impl Launcher {
    fn backups_dir(&self) -> Option<PathBuf> {
        self.library.as_ref().map(|l| l.join("backups"))
    }

    fn refresh_saves(&mut self) {
        let v = &mut self.saves_view;
        v.read = true;
        let file = saves::save_path().filter(|p| p.is_file());
        v.save = file.as_ref().map(|f| std::fs::read(f).map_err(|e| e.to_string()).and_then(|b| Save::parse(&b)));
        v.modified = file.and_then(|f| f.metadata().ok()?.modified().ok()).map(saves::stamp);
        v.backups = self.library.as_ref().map(|l| saves::backups(&l.join("backups"))).unwrap_or_default();
        v.rules = saves::save_path().map(|p| saves::save_rules(&p)).unwrap_or_default();
        v.matches =
            self.library.as_ref().map(|l| saves::recent_matches(&l.join("matches.ron"), 10)).unwrap_or_default();
    }

    /// Before Play: a copy of the save, when there is one (the oldest automatic ones go).
    pub(super) fn back_up_before_playing(&self) {
        if let (Some(save), Some(dir)) = (saves::save_path().filter(|p| p.is_file()), self.backups_dir()) {
            if let Err(e) = saves::back_up(&save, &dir, true) {
                eprintln!("backup before playing: {e}");
            }
        }
    }

    pub(super) fn saves(&mut self, ui: &mut egui::Ui) {
        let art = self.art.clone();
        if !self.saves_view.read {
            self.refresh_saves();
        }
        let ctx = ui.ctx().clone();
        ui.label(style::heading("Saves", 36.0));
        ui.label(
            RichText::new(
                "The game keeps one save with four slots, where the original kept it, so both share it. A copy is \
                 made each time you press Play.",
            )
            .color(style::DIM),
        );
        ui.add_space(8.0);
        let file = saves::save_path();
        let mut ask = None;
        let mut back_up = false;
        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            ui.label(style::heading("Your save", 23.0));
            if let Some(f) = &file {
                ui.label(RichText::new(f.display().to_string()).small().color(style::DIM));
            }
            match &self.saves_view.save {
                None => {
                    ui.label(
                        RichText::new("No save yet: the game makes one the first time it saves.").color(style::DIM),
                    );
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(format!("The save does not read: {e}")).color(style::BAD));
                }
                Some(Ok(s)) => {
                    let when = self.saves_view.modified.as_deref().unwrap_or("?");
                    if s.intact {
                        ui.label(RichText::new(format!("Intact, last written {when} (UTC).")).color(style::GOOD));
                    } else {
                        ui.label(
                            RichText::new(format!(
                                "Damaged (its check does not hold): the game would not load it. Written {when}."
                            ))
                            .color(style::BAD),
                        );
                    }
                    for (i, slot) in s.slots.iter().enumerate() {
                        let current = if i as u32 + 1 == s.current { "  (the last used)" } else { "" };
                        let text = if slot.used {
                            format!("Slot {}: {} Quidditch cards{current}", i + 1, slot.cards)
                        } else {
                            format!("Slot {}: empty", i + 1)
                        };
                        let r = ui.label(RichText::new(text).color(if slot.used { style::TEXT } else { style::DIM }));
                        if slot.used {
                            r.on_hover_text(format!(
                                "House {}, cup {} (place {}); World Cup {} (place {})",
                                slot.house,
                                slot.house_cup,
                                slot.house_position,
                                slot.world_cup,
                                slot.world_cup_position
                            ));
                        }
                    }
                }
            }
            if !self.saves_view.rules.is_empty() {
                let list: Vec<String> = self.saves_view.rules.iter().map(|(id, v)| format!("{id} {v}")).collect();
                ui.label(
                    RichText::new(format!("Progress made with Gameplay packs on: {}", list.join(", ")))
                        .color(style::WARN),
                );
            }
            ui.horizontal(|ui| {
                let present = self.saves_view.save.is_some();
                back_up = ui.add_enabled(present, style::button_with(&art, "icon_saves", "Back up now")).clicked();
                if ui.add(style::button_with(&art, "icon_folder", "Open the save folder")).clicked() {
                    if let Some(d) = file.as_ref().and_then(|f| f.parent()) {
                        let _ = std::fs::create_dir_all(d);
                        crate::launch::open_folder(d);
                    }
                }
                if ui.add_enabled(present, style::button_with(&art, "icon_delete", "Delete the save")).clicked() {
                    ask = Some(Ask::DeleteSave);
                }
            });
        });
        ui.add_space(8.0);
        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            ui.label(style::heading(format!("Backups ({})", self.saves_view.backups.len()), 23.0));
            if self.saves_view.backups.is_empty() {
                ui.label(RichText::new("None yet.").color(style::DIM));
            }
            for b in &self.saves_view.backups {
                ui.horizontal(|ui| {
                    let kind = if b.auto { "before playing" } else { "yours" };
                    ui.label(format!("{} UTC", b.when));
                    ui.label(RichText::new(kind).small().color(style::DIM));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(style::button_with(&art, "icon_delete", "Delete")).clicked() {
                            ask = Some(Ask::DeleteBackup(b.clone()));
                        }
                        if ui.add(style::button_with(&art, "icon_refresh", "Restore")).clicked() {
                            ask = Some(Ask::Restore(b.clone()));
                        }
                    });
                });
            }
            if ui.add(style::button_with(&art, "icon_folder", "Open the backups folder")).clicked() {
                if let Some(d) = self.backups_dir() {
                    let _ = std::fs::create_dir_all(&d);
                    crate::launch::open_folder(&d);
                }
            }
        });
        if !self.saves_view.matches.is_empty() {
            ui.add_space(8.0);
            style::tile(ui, &art, |ui| {
                ui.set_width(ui.available_width());
                ui.label(style::heading("Recent matches", 23.0));
                for m in &self.saves_view.matches {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} {} - {} {}", m.teams[0], m.score[0], m.score[1], m.teams[1]));
                        let what =
                            if m.kind.eq_ignore_ascii_case("quidd") { m.stadium.clone() } else { m.kind.clone() };
                        ui.label(RichText::new(format!("{what}, {} UTC", m.when)).small().color(style::DIM));
                        if !m.rules.is_empty() {
                            let ids: Vec<&str> = m.rules.iter().map(|(id, _)| id.as_str()).collect();
                            ui.label(
                                RichText::new(format!(" Gameplay: {} ", ids.join(", ")))
                                    .small()
                                    .background_color(style::WARN)
                                    .color(style::BACKGROUND),
                            );
                        }
                    });
                }
            });
        }
        if let Some((ok, m)) = &self.saves_view.message {
            ui.label(RichText::new(m).color(if *ok { style::GOOD } else { style::BAD }));
        }
        if back_up {
            let r = match (file.as_ref(), self.backups_dir()) {
                (Some(f), Some(d)) => saves::back_up(f, &d, false).map(|_| "Backed up.".to_string()),
                _ => Err("no save or no library folder".into()),
            };
            self.saves_view.message = Some(r.map_or_else(|e| (false, e), |m| (true, m)));
            self.refresh_saves();
        }
        if ask.is_some() {
            self.saves_view.asking = ask;
        }
        self.ask_saves(&ctx);
    }

    fn ask_saves(&mut self, ctx: &egui::Context) {
        let Some(ask) = self.saves_view.asking.clone() else { return };
        let (title, body, yes) = match &ask {
            Ask::Restore(b) => (
                "Restore this backup?".to_string(),
                format!(
                    "The save is replaced by the copy of {} (UTC). The save it replaces is backed up first.",
                    b.when
                ),
                "Restore it",
            ),
            Ask::DeleteBackup(b) => {
                ("Delete this backup?".to_string(), format!("The copy of {} (UTC) is deleted.", b.when), "Delete it")
            }
            Ask::DeleteSave => (
                "Delete the save?".to_string(),
                "All four slots go. A backup is made first, so it can be restored.".to_string(),
                "Delete it",
            ),
        };
        let modal = egui::Modal::new(egui::Id::unique("saves question")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(style::heading(title, 24.0));
            ui.label(body);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(style::primary(yes, 18.0)).clicked() {
                    Some(true)
                } else if ui.button("Cancel").clicked() {
                    Some(false)
                } else {
                    None
                }
            })
            .inner
        });
        let answer = modal.inner.or(modal.should_close().then_some(false));
        let Some(answer) = answer else { return };
        self.saves_view.asking = None;
        if !answer {
            return;
        }
        let (file, dir) = (saves::save_path(), self.backups_dir());
        let r = match (ask, file, dir) {
            (Ask::Restore(b), Some(f), Some(d)) => {
                saves::restore(&b.path, &f, &d).map(|_| format!("Restored the copy of {}.", b.when))
            }
            (Ask::DeleteBackup(b), _, _) => b
                .path
                .parent()
                .map_or(Ok(()), std::fs::remove_dir_all)
                .map(|_| "Backup deleted.".to_string())
                .map_err(|e| e.to_string()),
            (Ask::DeleteSave, Some(f), Some(d)) => saves::back_up(&f, &d, false)
                .and_then(|_| std::fs::remove_file(&f).map_err(|e| e.to_string()))
                .map(|_| "Save deleted (a backup was made first).".to_string()),
            _ => Err("no save folder or no library folder".into()),
        };
        self.saves_view.message = Some(r.map_or_else(|e| (false, e), |m| (true, m)));
        self.refresh_saves();
    }
}
