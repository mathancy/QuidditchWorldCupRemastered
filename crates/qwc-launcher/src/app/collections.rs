//! Collections in the launcher (`docs/PACK_STORE.md`, "Collections"): the Store's Collections view (a collection's
//! packs installed in one go, each checked as any store pack, then put in its order with the player's own order
//! saved first), the Your packs view's saved orders and "Share as a collection", and a profile's collections.

use std::collections::{BTreeMap, HashMap};

use eframe::egui::{self, RichText};

use super::{Job, Launcher};
use crate::account;
use crate::store::{self, Collection, CollectionPage, CollectionSummary, Installed};
use crate::{style, updates};

/// One of the player's packs, looked up in the store for sharing: its listing at the installed version, if any.
#[derive(Clone)]
struct Found {
    name: String,
    enabled: bool,
    listing: Option<u64>,
}

#[derive(Default)]
struct Share {
    name: String,
    description: String,
    finding: Option<Job<Vec<Found>>>,
    found: Option<Vec<Found>>,
    publishing: Option<Job<String>>,
    error: Option<String>,
}

#[derive(Default)]
pub(super) struct CollectionsView {
    /// Whether the Store shows collections rather than packs.
    pub(super) open: bool,
    words: String,
    by_name: bool,
    pub(super) list: Vec<CollectionSummary>,
    more: bool,
    page: u32,
    fetching: Option<Job<CollectionPage>>,
    appending: bool,
    error: Option<String>,
    pub(super) looked: bool,
    /// Collections opened to see their packs.
    details: HashMap<String, Result<Collection, String>>,
    detail_jobs: HashMap<String, Job<Collection>>,
    /// A collection being installed, and which.
    installing: Option<Job<(Collection, Installed)>>,
    installing_id: Option<String>,
    /// The Your packs view: a name for the order to keep, sharing it.
    order_name: String,
    share: Option<Share>,
    /// A collection of yours being removed.
    removing: Option<Job<String>>,
}

impl CollectionsView {
    pub(super) fn busy(&self) -> bool {
        self.fetching.is_some()
            || self.installing.is_some()
            || !self.detail_jobs.is_empty()
            || self.removing.is_some()
            || self.share.as_ref().is_some_and(|s| s.finding.is_some() || s.publishing.is_some())
    }
}

/// "by Ana · 4 packs · 24 Oct 2026" for a collection.
fn facts(c: &CollectionSummary) -> String {
    let packs = if c.packs == 1 { "1 pack".to_string() } else { format!("{} packs", c.packs) };
    format!("{packs}  \u{b7}  updated {}", super::account::day(&c.updated))
}

/// The packs of a collection that this remaster can't play, said for the player; none when it can play them all.
fn needs(c: &Collection) -> Option<String> {
    let this = updates::current();
    let bad: Vec<String> =
        c.items.iter().filter(|i| i.listed && !i.fits(&this)).map(|i| format!("{} ({})", i.name, i.engine)).collect();
    (!bad.is_empty()).then(|| format!("Needs another remaster version: {}.", bad.join(", ")))
}

impl Launcher {
    fn collections_fetch(&mut self, ctx: &egui::Context, next: bool) {
        let v = &mut self.packs_view.collections;
        let page = if next { v.page + 1 } else { 1 };
        let (base, words, by_name) = (store::store_url(), v.words.clone(), v.by_name);
        v.appending = next;
        v.error = None;
        v.looked = true;
        v.fetching = Some(Job::start(ctx, move |_| store::browse_collections(&base, &words, by_name, page)));
    }

    /// Open (or close) a collection to see its packs; read from the store the first time.
    fn toggle_detail(&mut self, ctx: &egui::Context, id: &str) {
        let v = &mut self.packs_view.collections;
        if v.details.remove(id).is_some() || v.detail_jobs.contains_key(id) {
            return;
        }
        let (base, who) = (store::store_url(), id.to_string());
        v.detail_jobs.insert(id.to_string(), Job::start(ctx, move |_| store::collection(&base, &who)));
    }

    /// Install collection `id`: its packs (each checked), then its order (the player's saved first) when done.
    pub(super) fn install_collection(&mut self, ctx: &egui::Context, id: &str) {
        let (Some(packs_dir), Some(library)) = (self.packs_profile().packs_dir, self.library.clone()) else {
            self.packs_view.message = Some((false, "No packs folder.".into()));
            return;
        };
        if !self.packs_view.read {
            self.refresh_packs();
        }
        let have: BTreeMap<String, String> =
            self.packs_view.installed.iter().map(|(id, p)| (id.clone(), p.manifest.version.clone())).collect();
        let (base, who, downloads) = (store::store_url(), id.to_string(), library.join("downloads"));
        self.packs_view.message = None;
        self.packs_view.collections.installing_id = Some(id.to_string());
        self.packs_view.collections.installing = Some(Job::start(ctx, move |p| {
            let c = store::collection(&base, &who)?;
            if let Some(why) = needs(&c) {
                return Err(why);
            }
            let done = store::install_collection(&c, &have, &downloads, &packs_dir, p)?;
            Ok((c, done))
        }));
    }

    pub(super) fn poll_collections(&mut self, ctx: &egui::Context) {
        let v = &mut self.packs_view.collections;
        if let Some(r) = v.fetching.as_ref().and_then(Job::take) {
            v.fetching = None;
            match r {
                Ok(page) => {
                    v.page = if v.appending { v.page + 1 } else { 1 };
                    if !v.appending {
                        v.list.clear();
                    }
                    v.list.extend(page.collections);
                    v.more = page.more;
                }
                Err(e) => v.error = Some(e),
            }
        }
        let done: Vec<String> = v
            .detail_jobs
            .iter()
            .filter(|(_, j)| j.answer.lock().is_ok_and(|a| a.is_some()))
            .map(|(k, _)| k.clone())
            .collect();
        for id in done {
            if let Some(r) = v.detail_jobs.remove(&id).and_then(|j| j.take()) {
                v.details.insert(id, r);
            }
        }
        if let Some(r) = v.removing.as_ref().and_then(Job::take) {
            v.removing = None;
            match r {
                Ok(name) => {
                    self.account_view.message = Some((true, format!("{name} is no longer in the store.")));
                    self.packs_view.collections.list.retain(|c| c.name != name);
                    self.reload_profile(ctx);
                }
                Err(e) => self.account_view.message = Some((false, e)),
            }
        }
        if let Some(share) = &mut self.packs_view.collections.share {
            if let Some(r) = share.finding.as_ref().and_then(Job::take) {
                share.finding = None;
                match r {
                    Ok(f) => share.found = Some(f),
                    Err(e) => share.error = Some(e),
                }
            }
            if let Some(r) = share.publishing.as_ref().and_then(Job::take) {
                share.publishing = None;
                match r {
                    Ok(_) => {
                        let name = share.name.clone();
                        self.packs_view.collections.share = None;
                        self.packs_view.message = Some((true, format!("{name} is shared: it's on your profile.")));
                    }
                    Err(e) => share.error = Some(e),
                }
            }
        }
        let Some(r) = self.packs_view.collections.installing.as_ref().and_then(Job::take) else { return };
        self.packs_view.collections.installing = None;
        self.packs_view.collections.installing_id = None;
        match r {
            Ok((c, done)) => {
                self.installed_from_store(&done.listings);
                let name = &c.summary.name;
                let saved = format!("Before {name}");
                self.profile.save_loadout(&saved);
                let order: Vec<qwc_packs::Entry> = c
                    .items
                    .iter()
                    .filter(|i| i.listed)
                    .map(|i| qwc_packs::Entry { id: i.id.clone(), enabled: i.enabled })
                    .collect();
                self.profile.apply_order(&order);
                let mut said = format!("{name} is in its order, with {} new.", done.installed.len());
                if !done.skipped.is_empty() {
                    said += &format!(" Left out (no longer in the store): {}.", done.skipped.join(", "));
                }
                said += &format!(" Your order before it is kept as \"{saved}\" (Your packs, Saved orders).");
                self.save_packs(&said);
            }
            Err(e) => self.packs_view.message = Some((false, e)),
        }
    }

    /// One collection's tile: its name, maker, packs, and Install; opened, its packs in order.
    fn collection_tile(&mut self, ui: &mut egui::Ui, c: &CollectionSummary, mine: bool) {
        let art = self.art.clone();
        let ctx = ui.ctx().clone();
        let busy = self.packs_view.installing.is_some() || self.packs_view.collections.installing.is_some();
        let mut author = None;
        let mut report = false;
        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(style::heading(&c.name, 21.0));
                if !c.complete {
                    ui.label(
                        RichText::new(" Incomplete ").small().background_color(style::WARN).color(style::BACKGROUND),
                    )
                    .on_hover_text("A pack in it is no longer in the store; the rest still install");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(!busy, style::primary_with(&art, "icon_download", "Install collection", 17.0))
                        .clicked()
                    {
                        self.install_collection(&ctx, &c.id);
                    }
                    if self.account_view.session.is_some() && !mine && ui.small_button("Report").clicked() {
                        report = true;
                    }
                    let open = self.packs_view.collections.details.contains_key(&c.id);
                    if ui.button(if open { "Hide packs" } else { "Show packs" }).clicked() {
                        self.toggle_detail(&ctx, &c.id);
                    }
                    if mine
                        && ui
                            .add_enabled(
                                self.packs_view.collections.removing.is_none(),
                                egui::Button::new("Remove from the store"),
                            )
                            .clicked()
                    {
                        if let Some((token, _)) = self.account_view.session.clone() {
                            let (base, id, name) = (store::store_url(), c.id.clone(), c.name.clone());
                            self.packs_view.collections.removing = Some(Job::start(&ctx, move |_| {
                                account::delete_collection(&base, &token, &id).map(|()| name).map_err(String::from)
                            }));
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(RichText::new("by").small().color(style::DIM));
                let name =
                    egui::Button::new(RichText::new(&c.author.name).small().color(style::GOLD_LIGHT)).frame(false);
                if ui.add(name).on_hover_text("Their profile").clicked() {
                    author = Some(c.author.id.clone());
                }
                ui.label(RichText::new(format!("\u{b7}  {}", facts(c))).small().color(style::DIM));
            });
            if !c.description.is_empty() {
                ui.label(&c.description);
            }
            match self.packs_view.collections.details.get(&c.id) {
                Some(Ok(d)) => {
                    ui.add_space(4.0);
                    for (n, i) in d.items.iter().enumerate() {
                        let mut line = format!("{}.  {}  {}", n + 1, i.name, i.version);
                        if !i.enabled {
                            line += "  (off)";
                        }
                        if !i.listed {
                            line += "  (no longer in the store)";
                        }
                        let colour = if i.listed { style::TEXT } else { style::WARN };
                        ui.label(RichText::new(line).color(colour));
                    }
                    if let Some(why) = needs(d) {
                        ui.label(RichText::new(why).color(style::WARN));
                    }
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(style::WARN));
                }
                None if self.packs_view.collections.detail_jobs.contains_key(&c.id) => {
                    ui.spinner();
                }
                None => {}
            }
        });
        if self.packs_view.collections.installing_id.as_deref() == Some(c.id.as_str()) {
            if let Some(job) = &self.packs_view.collections.installing {
                job.bar(ui, "Installing");
            }
        }
        if report {
            self.open_report("collection", &c.id, &format!("The collection {} by {}", c.name, c.author.name));
        }
        if let Some(id) = author {
            self.open_profile(&ctx, &id);
        }
    }

    /// The Store's Collections view.
    pub(super) fn collections_view(&mut self, ui: &mut egui::Ui) {
        let art = self.art.clone();
        let ctx = ui.ctx().clone();
        if !self.packs_view.collections.looked {
            self.collections_fetch(&ctx, false);
        }
        let mut search = false;
        ui.horizontal(|ui| {
            let v = &mut self.packs_view.collections;
            let words = ui.add(
                egui::TextEdit::singleline(&mut v.words).hint_text("Search collections, makers").desired_width(320.0),
            );
            search |= words.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            search |= ui.selectable_value(&mut v.by_name, false, "Newest").changed();
            search |= ui.selectable_value(&mut v.by_name, true, "By name").changed();
            search |= ui.add(style::button_with(&art, "icon_refresh", "Search")).clicked();
            if v.fetching.is_some() {
                ui.spinner();
            }
        });
        if search {
            self.collections_fetch(&ctx, false);
        }
        ui.label(
            RichText::new(
                "A collection is packs chosen and ordered to work together. Installing one gets every pack in it, each \
                 checked like any store pack, and puts them in its order; your own order is kept to go back to.",
            )
            .small()
            .color(style::DIM),
        );
        if let Some((ok, m)) = &self.packs_view.message {
            ui.label(RichText::new(m).color(if *ok { style::GOOD } else { style::BAD }));
        }
        if let Some(e) = &self.packs_view.collections.error {
            ui.label(RichText::new(e).color(style::WARN));
        }
        ui.add_space(8.0);
        let v = &self.packs_view.collections;
        if v.looked && v.fetching.is_none() && v.error.is_none() && v.list.is_empty() {
            style::tile(ui, &art, |ui| {
                ui.set_width(ui.available_width());
                let none = if v.words.trim().is_empty() {
                    "No collections in the store yet."
                } else {
                    "No collections match."
                };
                ui.label(RichText::new(none).color(style::DIM));
            });
        }
        let me = self.account_view.session.as_ref().map(|(_, m)| m.id.clone());
        for c in self.packs_view.collections.list.clone() {
            let mine = me.as_deref() == Some(c.author.id.as_str());
            self.collection_tile(ui, &c, mine);
            ui.add_space(6.0);
        }
        let v = &self.packs_view.collections;
        if v.more && v.fetching.is_none() && ui.button("More").clicked() {
            self.collections_fetch(&ctx, true);
        }
    }

    /// A profile's collections (on the Profile page).
    pub(super) fn profile_collections(&mut self, ui: &mut egui::Ui, list: &[CollectionSummary], mine: bool) {
        if list.is_empty() {
            ui.label(RichText::new("No collections shared yet.").color(style::DIM));
        }
        for c in list {
            self.collection_tile(ui, c, mine);
            ui.add_space(6.0);
        }
    }

    /// The Your packs view's saved orders: use one, delete one, keep the current order under a name.
    pub(super) fn saved_orders(&mut self, ui: &mut egui::Ui) {
        let art = self.art.clone();
        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Saved orders").strong());
            ui.label(
                RichText::new(
                    "Keep this order of packs to come back to. Installing a collection keeps your order here first.",
                )
                .small()
                .color(style::DIM),
            );
            let mut act: Option<(bool, String)> = None;
            for l in &self.profile.loadouts {
                ui.horizontal(|ui| {
                    let on = l.packs.iter().filter(|e| e.enabled).count();
                    ui.label(format!(
                        "{}  ({} pack{}, {on} on)",
                        l.name,
                        l.packs.len(),
                        if l.packs.len() == 1 { "" } else { "s" }
                    ));
                    if ui.small_button("Use").clicked() {
                        act = Some((true, l.name.clone()));
                    }
                    if ui.small_button("Delete").clicked() {
                        act = Some((false, l.name.clone()));
                    }
                });
            }
            ui.horizontal(|ui| {
                let l = ui.label("Keep this order as");
                ui.add(egui::TextEdit::singleline(&mut self.packs_view.collections.order_name).desired_width(200.0))
                    .labelled_by(l.id);
                let name = self.packs_view.collections.order_name.trim().to_string();
                if ui.add_enabled(!name.is_empty(), egui::Button::new("Save")).clicked() {
                    self.profile.save_loadout(&name);
                    self.packs_view.collections.order_name.clear();
                    self.save_packs(&format!("This order is kept as \"{name}\"."));
                }
            });
            match act {
                Some((true, name)) => {
                    self.profile.use_loadout(&name);
                    self.save_packs(&format!("Back to \"{name}\"."));
                }
                Some((false, name)) => {
                    self.profile.remove_loadout(&name);
                    self.save_packs(&format!("\"{name}\" is deleted."));
                }
                None => {}
            }
        });
    }

    /// Start sharing the current order as a collection: each pack looked up in the store at its installed version.
    pub(super) fn share_order(&mut self, ctx: &egui::Context) {
        if !self.packs_view.read {
            self.refresh_packs();
        }
        let packs: Vec<(String, String, String, bool)> = self
            .profile
            .packs
            .iter()
            .filter_map(|e| {
                let p = self.packs_view.installed.get(&e.id)?;
                Some((e.id.clone(), p.manifest.name.clone(), p.manifest.version.clone(), e.enabled))
            })
            .collect();
        let base = store::store_url();
        let finding = Job::start(ctx, move |_| {
            let mut found = Vec::new();
            for (id, name, version, enabled) in packs {
                let listing = store::pack(&base, &id)?
                    .and_then(|p| p.versions.into_iter().find(|v| v.version == version))
                    .map(|v| v.listing);
                found.push(Found { name, enabled, listing });
            }
            Ok(found)
        });
        self.packs_view.collections.share = Some(Share { finding: Some(finding), ..Share::default() });
    }

    /// The sharing dialog: a name and description, the packs that go in (and those that can't), Publish.
    pub(super) fn share_dialog(&mut self, ctx: &egui::Context) {
        let Some(share) = &mut self.packs_view.collections.share else { return };
        let token = self.account_view.session.as_ref().map(|(t, _)| t.clone());
        let mut close = false;
        egui::Modal::new(egui::Id::unique("share collection")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.label(style::heading("Share as a collection", 24.0));
            if token.is_none() {
                ui.label(RichText::new("Log in with Discord first (the bottom of the Play page).").color(style::WARN));
            }
            let l = ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut share.name).desired_width(f32::INFINITY).char_limit(60))
                .labelled_by(l.id);
            let l = ui.label("Description");
            ui.add(egui::TextEdit::multiline(&mut share.description).desired_rows(3).desired_width(f32::INFINITY))
                .labelled_by(l.id);
            ui.add_space(6.0);
            match (&share.finding, &share.found) {
                (Some(_), _) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Looking up your packs in the store...");
                    });
                }
                (None, Some(found)) => {
                    let (inside, outside): (Vec<&Found>, Vec<&Found>) = found.iter().partition(|f| f.listing.is_some());
                    ui.label(RichText::new("In the collection, in your order:").strong());
                    if inside.is_empty() {
                        ui.label(
                            RichText::new("None of your packs is in the store at the version you have.")
                                .color(style::WARN),
                        );
                    }
                    for f in &inside {
                        ui.label(format!("{}{}", f.name, if f.enabled { "" } else { "  (off)" }));
                    }
                    if !outside.is_empty() {
                        ui.label(
                            RichText::new(format!(
                                "Left out, not in the store at the version you have: {}.",
                                outside.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ")
                            ))
                            .small()
                            .color(style::DIM),
                        );
                    }
                }
                (None, None) => {}
            }
            if let Some(e) = &share.error {
                ui.label(RichText::new(e).color(style::BAD));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let items: Vec<(u64, bool)> =
                    share.found.iter().flatten().filter_map(|f| Some((f.listing?, f.enabled))).collect();
                let ready =
                    token.is_some() && !items.is_empty() && share.name.trim().len() >= 3 && share.publishing.is_none();
                if ui.add_enabled(ready, style::primary("Publish", 18.0)).clicked() {
                    let (base, token) = (store::store_url(), token.clone().unwrap_or_default());
                    let (name, description) = (share.name.trim().to_string(), share.description.trim().to_string());
                    share.error = None;
                    share.publishing = Some(Job::start(ctx, move |_| {
                        account::publish_collection(&base, &token, &name, &description, &items).map_err(String::from)
                    }));
                }
                if share.publishing.is_some() {
                    ui.spinner();
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.packs_view.collections.share = None;
        }
    }

    /// Show these collections as if fetched (the screenshots).
    pub fn show_collections(&mut self, list: Vec<CollectionSummary>, opened: Vec<Collection>) {
        self.page = super::Page::Packs;
        self.packs_view.store_open = true;
        let v = &mut self.packs_view.collections;
        v.open = true;
        v.looked = true;
        v.list = list;
        for c in opened {
            v.details.insert(c.summary.id.clone(), Ok(c));
        }
    }
}

impl Launcher {
    /// The Packs page's Your packs view with these orders kept (in memory only, the screenshots).
    pub fn show_saved_orders(&mut self, names: &[&str]) {
        self.page = super::Page::Packs;
        for n in names {
            self.profile.save_loadout(n);
        }
    }
}
