//! The Packs mode's Tuning tab, for Gameplay packs: their console scripts (`tuning/*.cli`, run on the match's console
//! after the game's own, and `tuning/teams/<team folder>/*.cli`, into that team's slot) edited as text, each line
//! checked as the console would run it while it is typed, and the original's registered entries listed to insert
//! (`qwc_sim::console::registry`: the variables and commands, their scope, their parameters and defaults).

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use qwc_packs::Kind;
use qwc_sim::console::{Kind as EntryKind, Scope, Ty};

use super::PackEditor;

/// The tab's state: the file open, its text as edited and as saved, the registry's filter, a new file's name.
#[derive(Default)]
pub(super) struct TuningView {
    pub(crate) file: Option<PathBuf>,
    pub(crate) text: String,
    saved: String,
    filter: String,
    pub(crate) new_name: String,
}

impl TuningView {
    pub(super) fn dirty(&self) -> bool {
        self.file.is_some() && self.text != self.saved
    }

    /// Write the script open; what was done.
    pub(super) fn save(&mut self) -> Result<String, String> {
        let Some(file) = &self.file else { return Ok(String::new()) };
        std::fs::write(file, &self.text).map_err(|e| format!("{}: {e}", file.display()))?;
        self.saved = self.text.clone();
        Ok(format!("Saved {}.", file.display()))
    }
}

/// The pack's tuning scripts, the match's first then each team's.
fn scripts(dir: &Path) -> Vec<PathBuf> {
    let cli = |d: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("cli")))
            .collect();
        v.sort();
        v
    };
    let tuning = dir.join("tuning");
    let mut out = cli(&tuning);
    let mut teams: Vec<PathBuf> =
        std::fs::read_dir(tuning.join("teams")).into_iter().flatten().flatten().map(|e| e.path()).collect();
    teams.sort();
    for t in teams {
        out.extend(cli(&t));
    }
    out
}

fn ty(t: &Ty) -> &'static str {
    match t {
        Ty::Int => "int",
        Ty::Float => "float",
        Ty::Bool => "bool",
        Ty::Word => "word",
        Ty::Str => "text",
        Ty::Enum => "choice",
    }
}

pub(super) fn tuning(pe: &mut PackEditor, ui: &mut egui::Ui) {
    let Some(dir) = pe.pack.as_ref().map(|p| p.dir.clone()) else { return };
    if pe.manifest.kind != Kind::Gameplay {
        ui.label("Tuning changes how the game plays, so it is for Gameplay packs only: set the pack's kind to Gameplay (on the left) and save, then come back.");
        return;
    }
    ui.label(
        "Console scripts in the game's own syntax, one entry a line (Pitch.GoalHeight 40). tuning/*.cli run after the \
         game's own scripts for every match; tuning/teams/<team folder>/*.cli run into that team's slot.",
    );
    let tv = &mut pe.tuning;
    let mut status = None;
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        let files = scripts(&dir);
        ui.horizontal_wrapped(|ui| {
            for f in &files {
                let rel = f.strip_prefix(&dir).unwrap_or(f).to_string_lossy().replace('\\', "/");
                if ui.selectable_label(tv.file.as_ref() == Some(f), rel).clicked() && !tv.dirty() {
                    tv.text = std::fs::read_to_string(f).unwrap_or_default();
                    tv.saved = tv.text.clone();
                    tv.file = Some(f.clone());
                }
            }
        });
        ui.horizontal(|ui| {
            let l = ui.label("New file").id;
            ui.add(
                egui::TextEdit::singleline(&mut tv.new_name)
                    .hint_text("match.cli or teams/Gryff/aim.cli")
                    .desired_width(220.0),
            )
            .labelled_by(l);
            let name = tv.new_name.trim().trim_start_matches("tuning/").to_string();
            let ok = name.to_ascii_lowercase().ends_with(".cli") && !name.contains("..") && !tv.dirty();
            if ui.add_enabled(ok, egui::Button::new("Make it")).clicked() {
                let f = name.split('/').fold(dir.join("tuning"), |d, p| d.join(p));
                if let Some(d) = f.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                if !f.exists() {
                    let _ = std::fs::write(&f, "# Tuning: one console entry a line\n");
                }
                tv.text = std::fs::read_to_string(&f).unwrap_or_default();
                tv.saved = tv.text.clone();
                tv.file = Some(f);
                tv.new_name.clear();
                status = Some(String::new());
            }
        });
        if tv.file.is_none() {
            ui.label("Open a script above, or make one.");
            return;
        }
        let name = tv.file.as_ref().and_then(|f| f.strip_prefix(&dir).ok()).map(|f| f.display().to_string());
        let l = ui.label(RichText::new(name.unwrap_or_default()).strong()).id;
        ui.add(egui::TextEdit::multiline(&mut tv.text).code_editor().desired_rows(18).desired_width(f32::INFINITY))
            .labelled_by(l);
        // the lines as the console runs them
        let errors = qwc_sim::Console::new().run_script(tv.text.as_bytes());
        if errors.is_empty() {
            ui.colored_label(qwc_style::GOOD, "Every line reads.");
        }
        for e in &errors {
            ui.colored_label(qwc_style::BAD, e.to_string());
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(tv.dirty(), egui::Button::new("Save the script")).clicked() {
                status = Some(tv.save().unwrap_or_else(|e| e));
            }
            if ui.add_enabled(tv.dirty(), egui::Button::new("Revert")).clicked() {
                tv.text = tv.saved.clone();
            }
        });

        let ui = &mut cols[1];
        ui.horizontal(|ui| {
            let l = ui.label("Find a setting").id;
            ui.text_edit_singleline(&mut tv.filter).labelled_by(l);
        });
        let filter = tv.filter.to_ascii_lowercase();
        let defs: Vec<&qwc_sim::console::Def> = qwc_sim::console::registry()
            .iter()
            .filter(|d| d.scope != Scope::Ignored && d.path.to_ascii_lowercase().contains(&filter))
            .collect();
        ui.label(RichText::new(format!("{} entries; click one to add its line", defs.len())).small());
        let row = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        egui::ScrollArea::vertical().id_salt("registry").auto_shrink([false; 2]).show_rows(
            ui,
            row,
            defs.len(),
            |ui, range| {
                for d in &defs[range] {
                    let params: Vec<&str> = d.params.iter().map(ty).collect();
                    let scope = if d.scope == Scope::Team { ", a team's" } else { "" };
                    let kind = if d.kind == EntryKind::Command { "command" } else { "variable" };
                    let default = if d.kind == EntryKind::Variable && !d.params.is_empty() {
                        format!(" = {}", d.default[0])
                    } else {
                        String::new()
                    };
                    let text = format!(
                        "{} ({}{scope}{default})",
                        d.path,
                        if params.is_empty() { kind.to_string() } else { params.join(", ") }
                    );
                    if ui.selectable_label(false, text).clicked() && tv.file.is_some() {
                        if !tv.text.is_empty() && !tv.text.ends_with('\n') {
                            tv.text.push('\n');
                        }
                        let value = if d.kind == EntryKind::Variable && !d.params.is_empty() {
                            d.default[0].to_string()
                        } else {
                            String::new()
                        };
                        tv.text += &format!("{} {value}\n", d.path);
                    }
                }
            },
        );
    });
    if let Some(s) = status {
        if !s.is_empty() {
            pe.status = s;
        }
        pe.refresh();
    }
}
