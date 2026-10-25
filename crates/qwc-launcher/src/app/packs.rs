//! The Packs page: the packs installed in the profile's order (the highest first), each on or off, moved up and down,
//! removed; `.qwcpack` files installed (picked, or dropped on the window); where the packs on overlap, and what is
//! wrong with them. Every change is written to the profile at once; the game reads it when it starts.

use std::collections::BTreeMap;
use std::path::PathBuf;

use eframe::egui::{self, RichText};
use qwc_packs::{Kind, Pack, Stack};

use super::{Job, Launcher};
use crate::style;

/// What the page shows, read again after each change.
#[derive(Default)]
pub(super) struct PacksView {
    /// The installed packs by id, and the folders whose manifests did not read.
    pub(super) installed: BTreeMap<String, Pack>,
    errors: Vec<String>,
    /// Each installed pack's slots, counted by kind ("46 texture, 3 movie").
    fills: BTreeMap<String, String>,
    /// The packs on: where they overlap (slot, the packs filling it, the winner first), what is wrong.
    overlaps: Vec<(String, Vec<String>)>,
    problems: Vec<String>,
    /// A `.qwcpack` being installed, a pack waiting for the removal to be confirmed, the last thing done.
    pub(super) installing: Option<Job<Pack>>,
    confirm_remove: Option<String>,
    pub(super) message: Option<(bool, String)>,
    pub(super) read: bool,
    /// Whether the page shows the store instead of the installed packs, and the store's view.
    pub(super) store_open: bool,
    pub(super) store: super::store::StoreView,
    /// The store's collections, saved orders and sharing.
    pub(super) collections: super::collections::CollectionsView,
}

/// "46 texture, 3 movie" for a pack's slots.
fn counts(slots: &[String]) -> String {
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for s in slots {
        *by.entry(s.split('/').next().unwrap_or(s)).or_default() += 1;
    }
    let parts: Vec<String> = by.iter().map(|(k, n)| format!("{n} {k}{}", if *n == 1 { "" } else { "s" })).collect();
    if parts.is_empty() {
        "nothing the remaster reads yet".into()
    } else {
        parts.join(", ")
    }
}

impl Launcher {
    /// Read the packs' folder and the profile again; every installed pack the profile does not list is put at the
    /// bottom, off.
    pub(super) fn refresh_packs(&mut self) {
        let (installed, errors) = self.packs_profile().installed();
        for id in installed.keys() {
            if !self.profile.packs.iter().any(|e| &e.id == id) {
                self.profile.packs.push(qwc_packs::Entry { id: id.clone(), enabled: false });
            }
        }
        let fills = installed.iter().map(|(id, p)| (id.clone(), counts(&p.slots()))).collect();
        let stack = Stack::from_profile(self.packs_profile(), None);
        let v = &mut self.packs_view;
        v.overlaps = stack.overlaps().into_iter().map(|o| (o.slot, o.packs)).collect();
        v.problems = stack.problems;
        (v.installed, v.errors, v.fills, v.read) = (installed, errors, fills, true);
    }

    /// Write the profile after a change, and read the page again.
    pub(super) fn save_packs(&mut self, done: &str) {
        let result = self.profile_path().ok_or("no library folder".to_string()).and_then(|p| self.profile.write(&p));
        self.packs_view.message = Some(match result {
            Ok(()) => (true, format!("{done} The game uses it the next time it starts.")),
            Err(e) => (false, e),
        });
        self.refresh_packs();
    }

    fn install_pack(&mut self, ctx: &egui::Context, file: PathBuf) {
        let Some(dir) = self.packs_profile().packs_dir else {
            self.packs_view.message = Some((false, "No packs folder.".into()));
            return;
        };
        self.packs_view.message = None;
        self.packs_view.installing = Some(Job::start(ctx, move |_| qwc_packs::install(&file, &dir)));
    }

    /// The pack installed: on, at the top.
    pub(super) fn poll_packs(&mut self) {
        if let Some(r) = self.packs_view.installing.as_ref().and_then(Job::take) {
            self.packs_view.installing = None;
            let listing = self.packs_view.store.installing.take();
            match &r {
                // from the store: told, so the player can rate it; a file not the one reviewed, reported
                Ok(_) => {
                    if let Some(l) = listing {
                        self.installed_from_store(&[l]);
                    }
                }
                Err(e) => {
                    if let Some(l) = listing {
                        self.report_changed(l, e);
                    }
                }
            }
            match r {
                Ok(p) => {
                    self.profile.put_on_top(&p.manifest.id);
                    self.save_packs(&format!("Installed {} {}, on at the top.", p.manifest.name, p.manifest.version));
                }
                Err(e) => self.packs_view.message = Some((false, e)),
            }
        }
    }

    pub(super) fn packs(&mut self, ui: &mut egui::Ui) {
        let art = self.art.clone();
        let ctx = ui.ctx().clone();
        if !self.packs_view.read {
            self.refresh_packs();
        }
        // .qwcpack files dropped on the window
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect()
        });
        if let Some(f) = dropped.into_iter().find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("qwcpack")))
        {
            self.install_pack(&ctx, f);
        }

        ui.label(style::heading("Resource packs", 36.0));
        ui.label(
            RichText::new(
                "Packs lay new textures, movies, screens, fonts, looks and stadiums over the game. The highest pack \
                 wins where two change the same thing.",
            )
            .color(style::DIM),
        );
        ui.add_space(6.0);
        // the packs installed, or the store's
        ui.horizontal(|ui| {
            let v = &mut self.packs_view;
            if ui.selectable_label(!v.store_open, RichText::new("Your packs").size(18.0)).clicked() {
                v.store_open = false;
            }
            if ui.selectable_label(v.store_open, RichText::new("Store").size(18.0)).clicked() {
                v.store_open = true;
            }
        });
        ui.add_space(6.0);
        if self.packs_view.store_open {
            self.store_view(ui);
            return;
        }
        ui.horizontal(|ui| {
            let busy = self.packs_view.installing.is_some();
            if ui.add_enabled(!busy, style::primary_with(&art, "icon_download", "Install a pack...", 20.0)).clicked() {
                if let Some(f) = rfd::FileDialog::new().add_filter("Resource pack", &["qwcpack"]).pick_file() {
                    self.install_pack(&ctx, f);
                }
            }
            if ui.add(style::button_with(&art, "icon_folder", "Open Packs folder")).clicked() {
                if let Some(d) = self.packs_profile().packs_dir {
                    let _ = std::fs::create_dir_all(&d);
                    crate::launch::open_folder(&d);
                }
            }
            if ui.add(style::button_with(&art, "icon_refresh", "Read again")).clicked() {
                self.refresh_packs();
            }
            if crate::launch::studio_exe().is_some()
                && ui
                    .add(style::button_with(&art, "icon_plus", "Make packs in QWC Studio"))
                    .on_hover_text("The Studio's Packs mode")
                    .clicked()
            {
                if let Err(e) = crate::launch::studio() {
                    self.packs_view.message = Some((false, e));
                }
            }
            if ui
                .add(style::button_with(&art, "icon_download", "Submit to the store..."))
                .on_hover_text("Share a pack you made in the pack store, after a review")
                .clicked()
            {
                self.open_submit();
            }
            if ui
                .add(style::button_with(&art, "icon_plus", "Share as a collection..."))
                .on_hover_text("Share this order of your store packs in the pack store")
                .clicked()
            {
                self.share_order(&ctx);
            }
            if busy {
                ui.spinner();
                ui.label("Installing...");
            }
        });
        ui.label(RichText::new("Or drop a .qwcpack file on this window.").small().color(style::DIM));
        if let Some((ok, m)) = &self.packs_view.message {
            ui.label(RichText::new(m).color(if *ok { style::GOOD } else { style::BAD }));
        }
        ui.add_space(8.0);

        if self.profile.packs.is_empty() {
            style::tile(ui, &art, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("No packs installed yet.").color(style::DIM));
            });
        }
        let count = self.profile.packs.len();
        let mut change: Option<(usize, Action)> = None;
        for (i, e) in self.profile.packs.iter().enumerate() {
            let pack = self.packs_view.installed.get(&e.id);
            style::tile(ui, &art, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let mut on = e.enabled;
                    let label = pack.map_or(e.id.as_str(), |p| p.manifest.name.as_str());
                    if ui
                        .add_enabled(pack.is_some(), egui::Checkbox::new(&mut on, style::heading(label, 21.0)))
                        .changed()
                    {
                        change = Some((i, Action::Enable(on)));
                    }
                    if pack.is_some_and(|p| p.manifest.kind == Kind::Gameplay) {
                        ui.label(
                            RichText::new(" Gameplay ").small().background_color(style::WARN).color(style::BACKGROUND),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let hint = "Delete the pack from the packs folder";
                        if style::icon_button(ui, &art, "icon_delete", "Remove", hint, true).clicked() {
                            change = Some((i, Action::Remove));
                        }
                        if style::icon_button(ui, &art, "icon_down", "Down", "Move the pack down", i + 1 < count)
                            .clicked()
                        {
                            change = Some((i, Action::Down));
                        }
                        if style::icon_button(ui, &art, "icon_up", "Up", "Move the pack up (above wins)", i > 0)
                            .clicked()
                        {
                            change = Some((i, Action::Up));
                        }
                    });
                });
                match pack {
                    Some(p) => {
                        let m = &p.manifest;
                        let by =
                            if m.authors.is_empty() { String::new() } else { format!("  by {}", m.authors.join(", ")) };
                        ui.label(RichText::new(format!("{}  {}{by}", m.id, m.version)).small().color(style::DIM));
                        if !m.description.is_empty() {
                            ui.label(&m.description);
                        }
                        let fills = self.packs_view.fills.get(&e.id).map_or("", String::as_str);
                        ui.label(RichText::new(format!("Changes: {fills}")).small().color(style::DIM));
                    }
                    None => {
                        ui.label(
                            RichText::new("Not installed: listed in the profile, but not in the packs folder.")
                                .color(style::WARN),
                        );
                    }
                }
            });
            ui.add_space(6.0);
        }
        if let Some((i, action)) = change {
            self.apply(i, action);
        }

        if !self.packs_view.overlaps.is_empty()
            || !self.packs_view.problems.is_empty()
            || !self.packs_view.errors.is_empty()
        {
            ui.add_space(6.0);
            style::tile(ui, &art, |ui| {
                ui.set_width(ui.available_width());
                for p in self.packs_view.problems.iter().chain(&self.packs_view.errors) {
                    ui.label(RichText::new(p).color(style::WARN));
                }
                if !self.packs_view.overlaps.is_empty() {
                    let names: BTreeMap<&str, &str> = self
                        .packs_view
                        .installed
                        .iter()
                        .map(|(id, p)| (id.as_str(), p.manifest.name.as_str()))
                        .collect();
                    let title = format!("Where packs overlap ({})", self.packs_view.overlaps.len());
                    egui::CollapsingHeader::new(RichText::new(title).strong()).show(ui, |ui| {
                        for (slot, packs) in self.packs_view.overlaps.iter().take(200) {
                            let who: Vec<&str> =
                                packs.iter().map(|id| names.get(id.as_str()).copied().unwrap_or(id)).collect();
                            ui.label(RichText::new(format!("{slot}: {}", who.join(" over "))).small());
                        }
                    });
                }
            });
        }
        ui.add_space(6.0);
        self.saved_orders(ui);
        self.share_dialog(&ctx);
        self.confirm_removal(&ctx);
    }

    fn apply(&mut self, i: usize, action: Action) {
        let id = self.profile.packs[i].id.clone();
        let name = self.packs_view.installed.get(&id).map_or(id.clone(), |p| p.manifest.name.clone());
        match action {
            Action::Enable(on) => {
                self.profile.packs[i].enabled = on;
                self.save_packs(&format!("{name} is {}.", if on { "on" } else { "off" }));
            }
            Action::Up => {
                self.profile.packs.swap(i, i - 1);
                self.save_packs(&format!("{name} moved up."));
            }
            Action::Down => {
                self.profile.packs.swap(i, i + 1);
                self.save_packs(&format!("{name} moved down."));
            }
            Action::Remove => {
                if self.packs_view.installed.contains_key(&id) {
                    self.packs_view.confirm_remove = Some(id);
                } else {
                    self.profile.packs.remove(i);
                    self.save_packs(&format!("{name} is no longer listed."));
                }
            }
        }
    }

    fn confirm_removal(&mut self, ctx: &egui::Context) {
        let Some(id) = self.packs_view.confirm_remove.clone() else { return };
        let Some(pack) = self.packs_view.installed.get(&id).cloned() else {
            self.packs_view.confirm_remove = None;
            return;
        };
        let modal = egui::Modal::new(egui::Id::unique("remove pack")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(style::heading(format!("Remove {}?", pack.manifest.name), 24.0));
            ui.label(format!("Its folder is deleted: {}", pack.dir.display()));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(style::primary("Delete it", 18.0)).clicked() {
                    Some(true)
                } else if ui.button("Keep it").clicked() {
                    Some(false)
                } else {
                    None
                }
            })
            .inner
        });
        let answer = modal.inner.or(modal.should_close().then_some(false));
        match answer {
            Some(true) => {
                self.packs_view.confirm_remove = None;
                match std::fs::remove_dir_all(&pack.dir) {
                    Ok(()) => {
                        self.profile.packs.retain(|e| e.id != id);
                        self.save_packs(&format!("Removed {}.", pack.manifest.name));
                    }
                    Err(e) => self.packs_view.message = Some((false, format!("{}: {e}", pack.dir.display()))),
                }
            }
            Some(false) => self.packs_view.confirm_remove = None,
            None => {}
        }
    }
}

#[derive(Clone, Copy)]
enum Action {
    Enable(bool),
    Up,
    Down,
    Remove,
}
