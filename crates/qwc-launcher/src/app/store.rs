//! The Packs page's Store view: the pack store's packs (`crate::store`), searched, filtered and sorted, a page at a
//! time; each installed from its author's link only when the file is the one reviewed, then put on at the top like a
//! pack installed from a file.

use eframe::egui::{self, RichText};

use super::{Job, Launcher};
use crate::store::{self, Listed, Page, Query, Sort};
use crate::{style, updates};

#[derive(Default)]
pub(super) struct StoreView {
    /// What to look for.
    words: String,
    kind: Option<&'static str>,
    sort: Sort,
    /// The packs read so far (every page up to `page`), and whether the store has more.
    pub(super) packs: Vec<Listed>,
    more: bool,
    page: u32,
    /// A page being read, and whether it adds to the list (More) or starts it again.
    fetching: Option<Job<Page>>,
    appending: bool,
    pub(super) error: Option<String>,
    /// Whether the store was asked at all yet (it is, the first time the view opens).
    pub(super) looked: bool,
    /// The listing being installed (the install itself is the Packs page's job).
    pub(super) installing: Option<u64>,
}

/// "12.3 MB" for a size in bytes.
fn megabytes(bytes: u64) -> String {
    let mb = bytes as f64 / 1e6;
    if mb < 0.1 {
        format!("{:.0} KB", (bytes as f64 / 1e3).max(1.0))
    } else {
        format!("{mb:.1} MB")
    }
}

/// The listing's line under its name, after its author: the version, the size, the rating and installs.
fn facts(p: &Listed) -> String {
    let rating = match p.rating.average {
        Some(a) => format!("\u{2605} {a:.1} ({})", p.rating.count),
        None => "not rated yet".into(),
    };
    let installs = if p.installs == 1 { "1 install".to_string() } else { format!("{} installs", p.installs) };
    format!("{}  \u{b7}  {}  \u{b7}  {rating}  \u{b7}  {installs}", p.version, megabytes(p.size))
}

/// What the listing's button does, given the version installed (if any).
#[derive(Debug, PartialEq, Eq)]
enum Offer {
    Install,
    Update,
    Installed,
    /// Made for other remaster versions: the requirement.
    Needs(String),
}

fn offer(p: &Listed, installed: Option<&str>, this: &semver::Version) -> Offer {
    if !p.fits(this) {
        return Offer::Needs(p.engine.clone());
    }
    let v = |s: &str| semver::Version::parse(s).ok();
    match installed {
        None => Offer::Install,
        Some(have) => match (v(have), v(&p.version)) {
            (Some(have), Some(listed)) if have >= listed => Offer::Installed,
            (Some(_), Some(_)) => Offer::Update,
            _ if have == p.version => Offer::Installed,
            _ => Offer::Update,
        },
    }
}

impl Launcher {
    /// Ask the store for the first page of what the view's fields say, or the next page.
    fn store_fetch(&mut self, ctx: &egui::Context, next: bool) {
        let v = &mut self.packs_view.store;
        let page = if next { v.page + 1 } else { 1 };
        let query = Query { words: v.words.clone(), kind: v.kind, sort: v.sort, page };
        v.appending = next;
        v.error = None;
        v.looked = true;
        let base = store::store_url();
        v.fetching = Some(Job::start(ctx, move |_| store::browse(&base, &query)));
    }

    /// A page read: added to the list or replacing it.
    pub(super) fn poll_store(&mut self) {
        let v = &mut self.packs_view.store;
        let Some(r) = v.fetching.as_ref().and_then(Job::take) else { return };
        v.fetching = None;
        match r {
            Ok(page) => {
                v.page = if v.appending { v.page + 1 } else { 1 };
                if !v.appending {
                    v.packs.clear();
                }
                v.packs.extend(page.packs);
                v.more = page.more;
            }
            Err(e) => v.error = Some(e),
        }
    }

    /// Download and install `listed`; the Packs page's install job puts it on at the top when it is in.
    fn store_install(&mut self, ctx: &egui::Context, listed: Listed) {
        let (Some(packs_dir), Some(library)) = (self.packs_profile().packs_dir, self.library.clone()) else {
            self.packs_view.message = Some((false, "No packs folder.".into()));
            return;
        };
        self.packs_view.message = None;
        self.packs_view.store.installing = Some(listed.listing);
        let downloads = library.join("downloads");
        self.packs_view.installing = Some(Job::start(ctx, move |p| store::install(&listed, &downloads, &packs_dir, p)));
    }

    pub(super) fn store_view(&mut self, ui: &mut egui::Ui) {
        let art = self.art.clone();
        let ctx = ui.ctx().clone();
        // the store's packs, or its collections
        ui.horizontal(|ui| {
            let c = &mut self.packs_view.collections;
            ui.selectable_value(&mut c.open, false, RichText::new("Packs").size(16.0));
            ui.selectable_value(&mut c.open, true, RichText::new("Collections").size(16.0));
        });
        ui.add_space(4.0);
        if self.packs_view.collections.open {
            self.collections_view(ui);
            return;
        }
        if !self.packs_view.store.looked {
            self.store_fetch(&ctx, false);
        }
        // the search: words, a kind, an order
        let mut search = false;
        ui.horizontal(|ui| {
            let v = &mut self.packs_view.store;
            let words = ui
                .add(egui::TextEdit::singleline(&mut v.words).hint_text("Search packs, authors").desired_width(320.0));
            search |= words.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let kinds = [(None, "All kinds"), (Some("Visual"), "Visual"), (Some("Gameplay"), "Gameplay")];
            let kind_label = kinds.iter().find(|k| k.0 == v.kind).map_or("All kinds", |k| k.1);
            ui.add_space(8.0);
            let l = ui.label("Kind");
            style::dropdown(&art, egui::ComboBox::from_id_salt("store kind"))
                .selected_text(kind_label)
                .show_ui(ui, |ui| {
                    for (k, label) in kinds {
                        search |= ui.selectable_value(&mut v.kind, k, label).changed();
                    }
                })
                .response
                .labelled_by(l.id);
            let l = ui.label("Order");
            style::dropdown(&art, egui::ComboBox::from_id_salt("store sort"))
                .selected_text(v.sort.label())
                .show_ui(ui, |ui| {
                    for s in Sort::ALL {
                        search |= ui.selectable_value(&mut v.sort, s, s.label()).changed();
                    }
                })
                .response
                .labelled_by(l.id);
            search |= ui.add(style::button_with(&art, "icon_refresh", "Search")).clicked();
            if v.fetching.is_some() {
                ui.spinner();
            }
        });
        if search {
            self.store_fetch(&ctx, false);
        }
        ui.label(
            RichText::new(
                "Every pack here was reviewed. It downloads from its author's own link, and installs only if the file \
                 is exactly the one reviewed.",
            )
            .small()
            .color(style::DIM),
        );
        if let Some((ok, m)) = &self.packs_view.message {
            ui.label(RichText::new(m).color(if *ok { style::GOOD } else { style::BAD }));
        }
        if let Some(e) = &self.packs_view.store.error {
            ui.label(RichText::new(e).color(style::WARN));
        }
        ui.add_space(8.0);

        let v = &self.packs_view.store;
        if v.looked && v.fetching.is_none() && v.error.is_none() && v.packs.is_empty() {
            style::tile(ui, &art, |ui| {
                ui.set_width(ui.available_width());
                let none = if v.words.trim().is_empty() && v.kind.is_none() {
                    "No packs in the store yet."
                } else {
                    "No packs match."
                };
                ui.label(RichText::new(none).color(style::DIM));
            });
        }
        let this = updates::current();
        let busy = self.packs_view.installing.is_some();
        let mut install: Option<Listed> = None;
        let mut author: Option<String> = None;
        let mut rated: Option<(String, u64, u8)> = None;
        let mut report: Option<(u64, String)> = None;
        let signed_in = self.account_view.session.is_some();
        let rating_busy = self.review_view.busy();
        for p in &self.packs_view.store.packs {
            let have = self.packs_view.installed.get(&p.id).map(|i| i.manifest.version.as_str());
            let offer = offer(p, have, &this);
            style::tile(ui, &art, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(style::heading(&p.name, 21.0));
                    if p.kind == "Gameplay" {
                        ui.label(
                            RichText::new(" Gameplay ").small().background_color(style::WARN).color(style::BACKGROUND),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (label, enabled) = match &offer {
                            Offer::Install => ("Install".to_string(), !busy),
                            Offer::Update => ("Update".to_string(), !busy),
                            Offer::Installed => ("Installed".to_string(), false),
                            Offer::Needs(req) => (format!("Needs QWCR {req}"), false),
                        };
                        if ui.add_enabled(enabled, style::primary_with(&art, "icon_download", &label, 17.0)).clicked() {
                            install = Some(p.clone());
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(RichText::new("by").small().color(style::DIM));
                    let name =
                        egui::Button::new(RichText::new(&p.author.name).small().color(style::GOLD_LIGHT)).frame(false);
                    if ui.add(name).on_hover_text("Their profile").clicked() {
                        author = Some(p.author.id.clone());
                    }
                    ui.label(RichText::new(format!("\u{b7}  {}", facts(p))).small().color(style::DIM));
                });
                // your rating (once installed from the store) and Report, signed in
                if signed_in {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Your rating").small().color(style::DIM));
                        let current = self.review_view.ratings.get(&p.id).copied().unwrap_or(0);
                        if let Some(n) = super::review::stars(ui, current, !rating_busy && have.is_some()) {
                            rated = Some((p.id.clone(), p.listing, n));
                        }
                        if have.is_none() {
                            ui.label(RichText::new("(install it to rate it)").small().color(style::DIM));
                        }
                        if ui.small_button("Report").clicked() {
                            report = Some((p.listing, format!("{} {} by {}", p.name, p.version, p.author.name)));
                        }
                    });
                }
                if !p.description.is_empty() {
                    ui.label(&p.description);
                }
                if self.packs_view.store.installing == Some(p.listing) {
                    if let Some(job) = &self.packs_view.installing {
                        job.bar(ui, "Downloading");
                    }
                }
            });
            ui.add_space(6.0);
        }
        if let Some(p) = install {
            self.store_install(&ctx, p);
        }
        if let Some(id) = author {
            self.open_profile(&ctx, &id);
        }
        if let Some((pack, listing, n)) = rated {
            self.rate(&ctx, &pack, listing, n);
        }
        if let Some((listing, about)) = report {
            self.open_report("listing", &listing.to_string(), &about);
        }
        if let Some((ok, m)) = &self.review_view.message {
            ui.label(RichText::new(m).color(if *ok { style::GOOD } else { style::BAD }));
        }
        let v = &self.packs_view.store;
        if v.more && v.fetching.is_none() && ui.button("More").clicked() {
            self.store_fetch(&ctx, true);
        }
        if std::env::var_os("QWC_STORE_URL").is_some() {
            ui.label(RichText::new(format!("Store: {}", store::store_url())).small().color(style::DIM));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(version: &str, engine: &str) -> Listed {
        serde_json::from_value(serde_json::json!({
            "id": "ana.night", "name": "Night", "version": version, "kind": "Visual",
            "author": {"id": "a1", "name": "Ana"}, "engine": engine, "listing": 1,
            "url": "https://x/n.qwcpack", "sha256": "a", "size": 2_500_000,
            "rating": {"average": 4.5, "count": 2}, "installs": 1
        }))
        .unwrap()
    }

    #[test]
    fn the_button_by_what_is_installed() {
        let this = semver::Version::new(0, 1, 1);
        assert_eq!(offer(&listed("1.1.0", ""), None, &this), Offer::Install);
        assert_eq!(offer(&listed("1.1.0", ""), Some("1.0.0"), &this), Offer::Update);
        assert_eq!(offer(&listed("1.1.0", ""), Some("1.1.0"), &this), Offer::Installed);
        assert_eq!(offer(&listed("1.1.0", ""), Some("1.2.0"), &this), Offer::Installed);
        assert_eq!(offer(&listed("1.1.0", ">=0.2"), None, &this), Offer::Needs(">=0.2".into()));
    }

    #[test]
    fn the_facts_line() {
        assert_eq!(facts(&listed("1.1.0", "")), "1.1.0  \u{b7}  2.5 MB  \u{b7}  \u{2605} 4.5 (2)  \u{b7}  1 install");
        assert_eq!(megabytes(2_000), "2 KB");
    }
}
