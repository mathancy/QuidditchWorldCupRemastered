//! The News page: the remaster's releases (their notes), the newest one past this version offered, its installer
//! downloaded and checked (`crate::updates`) and run. Looked for when the launcher opens, unless the settings say not.

use std::path::PathBuf;

use eframe::egui::{self, RichText};

use super::{Job, Launcher};
use crate::style;
use crate::updates::{self, Block, Release};

/// A release's notes ([`updates::notes`]) in `ink`, quotes and alerts in `note`, headings at `heading` points.
pub(super) fn draw_notes(ui: &mut egui::Ui, body: &str, ink: egui::Color32, note: egui::Color32, heading: f32) {
    for block in updates::notes(body) {
        match block {
            Block::Heading(t) => {
                ui.add_space(4.0);
                ui.label(RichText::new(t).font(egui::FontId::new(heading, style::heading_family())).color(ink));
            }
            Block::Text(t) => {
                ui.label(RichText::new(t).color(ink));
            }
            Block::Note(t) => {
                ui.label(RichText::new(t).color(note).strong());
            }
            Block::Item(depth, t) => {
                ui.horizontal_top(|ui| {
                    ui.add_space(20.0 * depth as f32);
                    ui.label(RichText::new(if depth == 0 { "\u{2022}" } else { "\u{2013}" }).color(ink));
                    ui.add(egui::Label::new(RichText::new(t).color(ink)).wrap());
                });
            }
        }
    }
}

#[derive(Default)]
pub(super) struct NewsView {
    fetching: Option<Job<Vec<Release>>>,
    pub(super) releases: Option<Result<Vec<Release>, String>>,
    downloading: Option<Job<PathBuf>>,
    message: Option<(bool, String)>,
}

impl NewsView {
    /// The newest release past this version, once the releases are in.
    pub(super) fn newer(&self) -> Option<&Release> {
        let r = self.releases.as_ref()?.as_ref().ok()?;
        updates::newer(r, &updates::current())
    }

    pub(super) fn busy(&self) -> bool {
        self.fetching.is_some() || self.downloading.is_some()
    }
}

impl Launcher {
    pub(super) fn check_releases(&mut self, ctx: &egui::Context) {
        let url = updates::releases_url();
        self.news_view.fetching = Some(Job::start(ctx, move |_| updates::fetch(&url)));
    }

    pub(super) fn poll_news(&mut self, ctx: &egui::Context) {
        if let Some(r) = self.news_view.fetching.as_ref().and_then(Job::take) {
            self.news_view.fetching = None;
            self.news_view.releases = Some(r);
        }
        if let Some(r) = self.news_view.downloading.as_ref().and_then(Job::take) {
            self.news_view.downloading = None;
            self.news_view.message = Some(match r {
                Ok(file) => self.run_installer(ctx, &file),
                Err(e) => (false, e),
            });
        }
    }

    /// The downloaded installer started (Windows: the launcher closes for it); on Linux the AppImage is shown.
    fn run_installer(&self, ctx: &egui::Context, file: &std::path::Path) -> (bool, String) {
        if cfg!(windows) {
            match std::process::Command::new(file).spawn() {
                Ok(_) => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    (true, "The installer is running.".into())
                }
                Err(e) => (false, format!("{}: {e}", file.display())),
            }
        } else {
            if let Some(d) = file.parent() {
                crate::launch::open_folder(d);
            }
            (true, format!("Downloaded and checked: {}. Run it in place of this one.", file.display()))
        }
    }

    pub(super) fn news(&mut self, ui: &mut egui::Ui) {
        let art = self.art.clone();
        let ctx = ui.ctx().clone();
        ui.label(style::heading("News and updates", 36.0));
        ui.label(RichText::new(format!("This is version {}.", updates::current())).color(style::DIM));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let busy = self.news_view.busy();
            if ui.add_enabled(!busy, egui::Button::new("Check now")).clicked() {
                self.check_releases(&ctx);
            }
            let s = &mut self.settings_view.settings.updates.check_on_start;
            if ui.checkbox(s, "Check when the launcher opens").changed() {
                self.settings_view.save();
            }
            if self.news_view.fetching.is_some() {
                ui.spinner();
            }
        });
        ui.add_space(8.0);

        if let Some(new) = self.news_view.newer().cloned() {
            egui::Frame::new()
                .fill(style::GOLD.gamma_multiply(0.25))
                .corner_radius(10)
                .inner_margin(egui::Margin::same(16))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let version = new.version().map_or(new.tag_name.clone(), |v| v.to_string());
                    ui.label(style::heading(format!("Version {version} is out"), 24.0));
                    ui.label(new.title());
                    match (&self.news_view.downloading, new.installer()) {
                        (Some(job), _) => {
                            job.bar(ui, "Downloading");
                            if ui.button("Stop").clicked() {
                                job.progress.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                        (None, Some(asset)) => {
                            let label = if cfg!(windows) { "Download and install" } else { "Download" };
                            if ui.add(style::primary(label, 20.0)).clicked() {
                                let dir =
                                    self.library.as_ref().map_or_else(std::env::temp_dir, |l| l.join("downloads"));
                                let r = new.clone();
                                self.news_view.message = None;
                                self.news_view.downloading =
                                    Some(Job::start(&ctx, move |p| updates::download(&r, &dir, p)));
                            }
                            ui.label(
                                RichText::new(format!("{} ({:.0} MB)", asset.name, asset.size as f64 / 1e6))
                                    .small()
                                    .color(style::DIM),
                            );
                        }
                        (None, None) => {
                            ui.label(RichText::new("It has no installer for this system yet.").color(style::DIM));
                        }
                    }
                });
            ui.add_space(8.0);
        }
        if let Some((ok, m)) = &self.news_view.message {
            ui.label(RichText::new(m).color(if *ok { style::GOOD } else { style::BAD }));
        }

        match &self.news_view.releases {
            None if self.news_view.fetching.is_none() => {
                ui.label(RichText::new("Not checked yet.").color(style::DIM));
            }
            None => {}
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(style::WARN));
            }
            Some(Ok(list)) if list.is_empty() => {
                ui.label(RichText::new("No releases are published yet.").color(style::DIM));
            }
            Some(Ok(list)) => {
                for r in list.iter().take(10) {
                    style::tile(ui, &art, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(style::heading(r.title(), 22.0));
                            if r.prerelease {
                                ui.label(
                                    RichText::new(" test build ")
                                        .small()
                                        .background_color(style::WARN)
                                        .color(style::BACKGROUND),
                                );
                            }
                            ui.label(RichText::new(r.date()).small().color(style::DIM));
                        });
                        if let Some(body) = r.body.as_deref().filter(|b| !b.trim().is_empty()) {
                            draw_notes(ui, body, style::TEXT, style::WARN, 18.0);
                        }
                    });
                    ui.add_space(6.0);
                }
            }
        }
    }
}
