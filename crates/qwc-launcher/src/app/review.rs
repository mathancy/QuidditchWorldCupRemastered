//! Submitting, rating, reporting and reviewing in the launcher (`docs/PACK_STORE.md`, "Listings", "Review", "Reports
//! and removal", "Ratings"): the Submit to the store dialog (the author's file read, their link checked to give
//! exactly that file, then submitted), your submissions and their state, the stars and Report on the store's packs,
//! the report dialog, and the Review page for reviewers (the queue: each file checked, tried, approved or rejected;
//! the open reports and what is done about them).

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui::{self, RichText};

use super::{Job, Launcher, Page};
use crate::account::{self, MySubmission, OpenReport, PackFile, Pending};
use crate::store::{self, Checked};
use crate::style;

/// The Submit to the store dialog.
#[derive(Default)]
struct Submit {
    file: Option<PathBuf>,
    read: Option<Result<PackFile, String>>,
    url: String,
    description: String,
    screenshots: String,
    sending: Option<Job<u64>>,
    error: Option<String>,
}

/// The report dialog: what is reported, said for the player.
struct Report {
    kind: &'static str,
    target: String,
    about: String,
    reason: usize,
    note: String,
    sending: Option<Job<()>>,
    error: Option<String>,
}

#[derive(Default)]
pub(super) struct ReviewView {
    submit: Option<Submit>,
    report: Option<Report>,
    /// Your submissions, and your ratings by pack id.
    pub(super) mine: Option<Result<Vec<MySubmission>, String>>,
    mine_job: Option<Job<Vec<MySubmission>>>,
    pub(super) ratings: HashMap<String, u8>,
    ratings_job: Option<Job<Vec<account::MyRating>>>,
    rating: Option<Job<(String, u8)>>,
    /// The Review page: the queue, the reports, each file's check and each note.
    queue: Option<Result<Vec<Pending>, String>>,
    queue_job: Option<Job<Vec<Pending>>>,
    reports: Option<Result<Vec<OpenReport>, String>>,
    reports_job: Option<Job<Vec<OpenReport>>>,
    checks: HashMap<u64, Result<Checked, String>>,
    checking: Option<(u64, Job<Checked>)>,
    notes: HashMap<u64, String>,
    acting: Option<Job<String>>,
    pub(super) message: Option<(bool, String)>,
    /// The account your submissions and ratings were read for.
    loaded_for: Option<String>,
}

impl ReviewView {
    pub(super) fn busy(&self) -> bool {
        self.mine_job.is_some()
            || self.ratings_job.is_some()
            || self.rating.is_some()
            || self.queue_job.is_some()
            || self.reports_job.is_some()
            || self.checking.is_some()
            || self.acting.is_some()
            || self.submit.as_ref().is_some_and(|s| s.sending.is_some())
            || self.report.as_ref().is_some_and(|r| r.sending.is_some())
    }
}

/// "Waiting for review", "Listed", "Rejected", "Removed" for a submission's state, and its colour.
fn state(s: &str) -> (&'static str, egui::Color32) {
    match s {
        "approved" => ("Listed", style::GOOD),
        "rejected" => ("Rejected", style::BAD),
        "removed" => ("Removed", style::BAD),
        _ => ("Waiting for review", style::WARN),
    }
}

/// Five stars, `current` of them lit; the stars clicked, if any.
pub(super) fn stars(ui: &mut egui::Ui, current: u8, enabled: bool) -> Option<u8> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        for n in 1..=5u8 {
            let lit = n <= current;
            // unlit stars are the same star, dim: the launcher's fonts have no hollow one
            let text = RichText::new("\u{2605}").size(18.0).color(if lit {
                style::GOLD
            } else {
                style::GOLD_DEEP.gamma_multiply(0.6)
            });
            let star = egui::Button::new(text).frame(false);
            if ui.add_enabled(enabled, star).on_hover_text(format!("{n} of 5")).clicked() {
                clicked = Some(n);
            }
        }
    });
    clicked
}

impl Launcher {
    fn token(&self) -> Option<String> {
        self.account_view.session.as_ref().map(|(t, _)| t.clone())
    }

    /// Your submissions and ratings, read again (after signing in, a submission, a rating).
    pub(super) fn load_mine(&mut self, ctx: &egui::Context) {
        let Some(token) = self.token() else { return };
        let base = store::store_url();
        let (b, t) = (base.clone(), token.clone());
        self.review_view.mine_job =
            Some(Job::start(ctx, move |_| account::my_submissions(&b, &t).map_err(String::from)));
        self.review_view.ratings_job =
            Some(Job::start(ctx, move |_| account::my_ratings(&base, &token).map_err(String::from)));
    }

    /// After a pack is installed from the store (checked): the store told, so the player can rate it.
    pub(super) fn installed_from_store(&self, listings: &[u64]) {
        let Some(token) = self.token() else { return };
        let (base, listings) = (store::store_url(), listings.to_vec());
        std::thread::spawn(move || {
            for l in listings {
                let _ = account::record_install(&base, &token, l);
            }
        });
    }

    /// A store install refused because the file isn't the one reviewed: reported for re-review (when signed in).
    pub(super) fn report_changed(&self, listing: u64, why: &str) {
        let changed = ["checksum", "larger than", "bytes, not the", "not the listed"].iter().any(|w| why.contains(w));
        let Some(token) = self.token().filter(|_| changed) else { return };
        let (base, note) = (store::store_url(), format!("Found by the launcher: {why}"));
        std::thread::spawn(move || account::report(&base, &token, "listing", &listing.to_string(), "changed", &note));
    }

    pub(super) fn poll_review(&mut self, ctx: &egui::Context) {
        // your submissions and ratings, read once per sign-in; forgotten on logging out
        let who = self.account_view.session.as_ref().map(|(_, m)| m.id.clone());
        if who != self.review_view.loaded_for {
            self.review_view.loaded_for = who.clone();
            self.review_view.mine = None;
            self.review_view.ratings.clear();
            if who.is_some() {
                self.load_mine(ctx);
            }
        }
        let v = &mut self.review_view;
        if let Some(r) = v.mine_job.as_ref().and_then(Job::take) {
            v.mine_job = None;
            v.mine = Some(r);
        }
        if let Some(r) = v.ratings_job.as_ref().and_then(Job::take) {
            v.ratings_job = None;
            if let Ok(list) = r {
                v.ratings = list.into_iter().map(|r| (r.pack, r.stars)).collect();
            }
        }
        if let Some(r) = v.rating.as_ref().and_then(Job::take) {
            v.rating = None;
            match r {
                Ok((pack, stars)) => {
                    v.ratings.insert(pack, stars);
                    v.message = Some((true, "Thanks for rating it.".into()));
                }
                Err(e) => v.message = Some((false, e)),
            }
        }
        if let Some(r) = v.queue_job.as_ref().and_then(Job::take) {
            v.queue_job = None;
            v.queue = Some(r);
        }
        if let Some(r) = v.reports_job.as_ref().and_then(Job::take) {
            v.reports_job = None;
            v.reports = Some(r);
        }
        if let Some(r) = v.checking.as_ref().and_then(|(_, j)| j.take()) {
            if let Some((id, _)) = v.checking.take() {
                v.checks.insert(id, r);
            }
        }
        if let Some(r) = v.acting.as_ref().and_then(Job::take) {
            v.acting = None;
            v.message = Some(match r {
                Ok(done) => (true, done),
                Err(e) => (false, e),
            });
            self.load_review(ctx);
            if self.page == Page::Profile {
                self.reload_profile(ctx);
            }
        }
        let v = &mut self.review_view;
        if let Some(sub) = &mut v.submit {
            if let Some(r) = sub.sending.as_ref().and_then(Job::take) {
                sub.sending = None;
                match r {
                    Ok(_) => {
                        let name = sub.read.as_ref().and_then(|r| r.as_ref().ok()).map(|f| f.manifest.name.clone());
                        v.submit = None;
                        self.packs_view.message = Some((
                            true,
                            format!(
                                "{} is submitted: it waits for a reviewer (your profile shows where it stands).",
                                name.unwrap_or_default()
                            ),
                        ));
                        self.load_mine(ctx);
                    }
                    Err(e) => sub.error = Some(e),
                }
            }
        }
        let v = &mut self.review_view;
        if let Some(rep) = &mut v.report {
            if let Some(r) = rep.sending.as_ref().and_then(Job::take) {
                rep.sending = None;
                match r {
                    Ok(()) => {
                        v.report = None;
                        v.message = Some((true, "Reported: a reviewer will look at it. Thank you.".into()));
                    }
                    Err(e) => rep.error = Some(e),
                }
            }
        }
    }

    // -----------------------------------------------------------------------------------------------------------
    // Rating and reporting from the store

    /// Rate `pack` (its listing `listing`) with `stars`.
    pub(super) fn rate(&mut self, ctx: &egui::Context, pack: &str, listing: u64, stars: u8) {
        let Some(token) = self.token() else { return };
        let (base, pack) = (store::store_url(), pack.to_string());
        self.review_view.message = None;
        self.review_view.rating = Some(Job::start(ctx, move |_| {
            account::rate(&base, &token, listing, stars).map(|()| (pack, stars)).map_err(String::from)
        }));
    }

    /// Open the report dialog on `kind` `target` (said as `about`).
    pub(super) fn open_report(&mut self, kind: &'static str, target: &str, about: &str) {
        self.review_view.report = Some(Report {
            kind,
            target: target.to_string(),
            about: about.to_string(),
            reason: 0,
            note: String::new(),
            sending: None,
            error: None,
        });
    }

    pub(super) fn report_dialog(&mut self, ctx: &egui::Context) {
        let token = self.token();
        let Some(rep) = &mut self.review_view.report else { return };
        let mut close = false;
        egui::Modal::new(egui::Id::unique("report")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.label(style::heading("Report", 24.0));
            ui.label(RichText::new(&rep.about).color(style::DIM));
            if token.is_none() {
                ui.label(
                    RichText::new("Log in with Discord to report (or tell us on the project's Discord server).")
                        .color(style::WARN),
                );
            }
            ui.add_space(6.0);
            // the file-changed reason is the launcher's own; players pick from the others
            for (i, (_, said)) in account::REASONS.iter().enumerate().filter(|(_, (k, _))| *k != "changed") {
                ui.radio_value(&mut rep.reason, i, *said);
            }
            let l = ui.label("What's wrong (optional)");
            ui.add(
                egui::TextEdit::multiline(&mut rep.note).desired_rows(3).desired_width(f32::INFINITY).char_limit(1000),
            )
            .labelled_by(l.id);
            if let Some(e) = &rep.error {
                ui.label(RichText::new(e).color(style::BAD));
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(token.is_some() && rep.sending.is_none(), style::primary("Send", 18.0)).clicked() {
                    let (base, token) = (store::store_url(), token.clone().unwrap_or_default());
                    let (kind, target, reason, note) =
                        (rep.kind, rep.target.clone(), account::REASONS[rep.reason].0, rep.note.clone());
                    rep.sending = Some(Job::start(ctx, move |_| {
                        account::report(&base, &token, kind, &target, reason, &note).map_err(String::from)
                    }));
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.review_view.report = None;
        }
    }

    // -----------------------------------------------------------------------------------------------------------
    // Submitting

    pub(super) fn open_submit(&mut self) {
        self.review_view.submit = Some(Submit::default());
    }

    pub(super) fn submit_dialog(&mut self, ctx: &egui::Context) {
        let token = self.token();
        let Some(sub) = &mut self.review_view.submit else { return };
        let mut close = false;
        egui::Modal::new(egui::Id::unique("submit pack")).show(ctx, |ui| {
            ui.set_width(600.0);
            ui.label(style::heading("Submit to the store", 24.0));
            ui.label(
                RichText::new(
                    "The store lists packs that authors host themselves. Build the .qwcpack in QWC Studio, upload it as \
                     a release on GitHub, GitLab or Codeberg (or to Dropbox), then give its download link here. A \
                     reviewer checks it before it is listed.",
                )
                .small()
                .color(style::DIM),
            );
            if token.is_none() {
                ui.label(RichText::new("Log in with Discord first (the bottom of the Play page).").color(style::WARN));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Choose the .qwcpack...").clicked() {
                    if let Some(f) = rfd::FileDialog::new().add_filter("Resource pack", &["qwcpack"]).pick_file() {
                        let read = account::read_pack_file(&f);
                        if let Ok(p) = &read {
                            sub.description = p.manifest.description.clone();
                        }
                        sub.read = Some(read);
                        sub.file = Some(f);
                    }
                }
                if let Some(f) = &sub.file {
                    ui.label(RichText::new(f.display().to_string()).small().color(style::DIM));
                }
            });
            let mut ready = false;
            match &sub.read {
                Some(Ok(p)) => {
                    let m = &p.manifest;
                    let kind = if m.kind == qwc_packs::Kind::Gameplay { "Gameplay" } else { "Visual" };
                    ui.label(format!("{}  {}  ({kind}, {:.1} MB)", m.name, m.version, p.size as f64 / 1e6));
                    ui.label(RichText::new(format!("{}  \u{b7}  for remaster {}", m.id, if m.engine.is_empty() { "any" } else { &m.engine })).small().color(style::DIM));
                    match m.declaration.as_ref().map_or(Some("it has no statement for the store"), qwc_packs::Declaration::missing) {
                        Some(why) => {
                            ui.label(
                                RichText::new(format!(
                                    "Not ready for the store: {why}. Open the pack in QWC Studio, fill in For the pack store, and build it again."
                                ))
                                .color(style::WARN),
                            );
                        }
                        None => {
                            ready = true;
                            ui.label(RichText::new("The statement is complete: your own work, its licence and credits.").color(style::GOOD));
                        }
                    }
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(style::BAD));
                }
                None => {}
            }
            ui.add_space(6.0);
            let l = ui.label("Download link");
            ui.add(egui::TextEdit::singleline(&mut sub.url).hint_text("https://github.com/you/pack/releases/download/v1.0.0/pack.qwcpack").desired_width(f32::INFINITY))
                .labelled_by(l.id);
            let l = ui.label("Description");
            ui.add(egui::TextEdit::multiline(&mut sub.description).desired_rows(3).desired_width(f32::INFINITY).char_limit(4000))
                .labelled_by(l.id);
            let l = ui.label("Screenshots (optional): up to 5 https:// picture links, one a line");
            ui.add(egui::TextEdit::multiline(&mut sub.screenshots).desired_rows(2).desired_width(f32::INFINITY)).labelled_by(l.id);
            if let Some(e) = &sub.error {
                ui.label(RichText::new(e).color(style::BAD));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let go = ready && token.is_some() && sub.url.trim().starts_with("http") && sub.sending.is_none();
                if ui
                    .add_enabled(go, style::primary("Check the link and submit", 18.0))
                    .on_hover_text("Downloads your link first: it must give exactly this file")
                    .clicked()
                {
                    if let Some(Ok(p)) = &sub.read {
                        let (base, token, p) = (store::store_url(), token.clone().unwrap_or_default(), p.clone());
                        let (url, description) = (sub.url.trim().to_string(), sub.description.trim().to_string());
                        let shots: Vec<String> = sub.screenshots.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
                        let dir = std::env::temp_dir().join(format!("qwc-submit-{}", std::process::id()));
                        sub.error = None;
                        sub.sending = Some(Job::start(ctx, move |progress| {
                            // the link must give this very file, as a reviewer's and players' launchers will check
                            let file = dir.join("check.qwcpack");
                            let _ = std::fs::create_dir_all(&dir);
                            let fetched = crate::updates::fetch_checked(&url, &file, Some(p.size), &p.sha256, progress);
                            let _ = std::fs::remove_dir_all(&dir);
                            fetched.map_err(|e| format!("The link doesn't give this file: {e}"))?;
                            account::submit(&base, &token, &p, &url, &description, &shots).map_err(String::from)
                        }));
                    }
                }
                if let Some(job) = &sub.sending {
                    job.bar(ui, "Checking the link");
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.review_view.submit = None;
        }
    }

    /// Your submissions on your profile: each version, where it stands, and a reviewer's note.
    pub(super) fn my_submissions(&mut self, ui: &mut egui::Ui) {
        let art = self.art.clone();
        ui.label(style::heading("Your submissions", 24.0));
        match &self.review_view.mine {
            None => {
                ui.label(RichText::new("Reading...").color(style::DIM));
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(style::WARN));
            }
            Some(Ok(list)) if list.is_empty() => {
                ui.label(
                    RichText::new("Nothing submitted yet: Packs, Your packs, Submit to the store.").color(style::DIM),
                );
            }
            Some(Ok(list)) => {
                style::tile(ui, &art, |ui| {
                    ui.set_width(ui.available_width());
                    for s in list {
                        let (said, colour) = state(&s.status);
                        ui.horizontal(|ui| {
                            ui.label(format!("{}  {}", s.name, s.version));
                            ui.label(RichText::new(said).color(colour));
                            ui.label(RichText::new(super::account::day(&s.submitted)).small().color(style::DIM));
                        });
                        if let Some(n) = s.note.as_deref().filter(|n| !n.is_empty()) {
                            ui.label(RichText::new(format!("Reviewer: {n}")).small().color(style::DIM));
                        }
                    }
                });
            }
        }
    }

    // -----------------------------------------------------------------------------------------------------------
    // The Review page

    /// Read the queue and the reports again.
    pub(super) fn load_review(&mut self, ctx: &egui::Context) {
        let Some(token) = self.token() else { return };
        let base = store::store_url();
        let (b, t) = (base.clone(), token.clone());
        self.review_view.queue_job =
            Some(Job::start(ctx, move |_| account::review_queue(&b, &t).map_err(String::from)));
        self.review_view.reports_job =
            Some(Job::start(ctx, move |_| account::open_reports(&base, &token).map_err(String::from)));
    }

    pub(super) fn open_review(&mut self, ctx: &egui::Context) {
        self.page = Page::Review;
        self.review_view.message = None;
        self.load_review(ctx);
    }

    /// Something a reviewer does, then the page read again: `work` answers what to say.
    fn act(&mut self, ctx: &egui::Context, work: impl FnOnce(&str, &str) -> Result<String, String> + Send + 'static) {
        let Some(token) = self.token() else { return };
        let base = store::store_url();
        self.review_view.message = None;
        self.review_view.acting = Some(Job::start(ctx, move |_| work(&base, &token)));
    }

    pub(super) fn review_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let art = self.art.clone();
        let me = self.account_view.session.as_ref().map(|(_, m)| m.clone());
        let Some(me) = me.filter(|m| m.role == "reviewer" || m.role == "admin") else {
            ui.label(RichText::new("The Review page is for the store's reviewers.").color(style::DIM));
            return;
        };
        ui.horizontal(|ui| {
            ui.label(style::heading("Review", 36.0));
            if ui.add(style::button_with(&art, "icon_refresh", "Read again")).clicked() {
                self.load_review(&ctx);
            }
            if self.review_view.busy() {
                ui.spinner();
            }
        });
        ui.label(
            RichText::new(
                "Check each file, try it in the game, and hold it to the content rules: the author's own work, nothing \
                 from the game's files or made from them, no EA or Warner Bros. logos or artwork. Reject with a note the \
                 author reads.",
            )
            .color(style::DIM),
        );
        if let Some((ok, m)) = &self.review_view.message {
            ui.label(RichText::new(m).color(if *ok { style::GOOD } else { style::BAD }));
        }
        ui.add_space(8.0);

        // the queue
        let queue = match &self.review_view.queue {
            Some(Ok(q)) => q.clone(),
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(style::WARN));
                vec![]
            }
            None => vec![],
        };
        ui.label(style::heading(format!("Waiting for review ({})", queue.len()), 24.0));
        if queue.is_empty() {
            ui.label(RichText::new("Nothing waiting.").color(style::DIM));
        }
        for p in &queue {
            self.pending_tile(ui, p, &me);
            ui.add_space(6.0);
        }

        // the reports
        ui.add_space(8.0);
        let reports = match &self.review_view.reports {
            Some(Ok(r)) => r.clone(),
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(style::WARN));
                vec![]
            }
            None => vec![],
        };
        ui.label(style::heading(format!("Reports ({})", reports.len()), 24.0));
        if reports.is_empty() {
            ui.label(RichText::new("No open reports.").color(style::DIM));
        }
        for r in &reports {
            self.report_tile(ui, r, me.role == "admin");
            ui.add_space(6.0);
        }
    }

    fn pending_tile(&mut self, ui: &mut egui::Ui, p: &Pending, me: &account::Me) {
        let ctx = ui.ctx().clone();
        let art = self.art.clone();
        let own = p.author.id == me.id;
        let busy = self.review_view.acting.is_some();
        let mut author = None;
        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(style::heading(format!("{}  {}", p.name, p.version), 21.0));
                let new = if p.listed_before == 0 { " New pack " } else { " New version " };
                ui.label(RichText::new(new).small().background_color(style::GOLD_DEEP).color(style::GOLD_LIGHT));
                if p.kind == "Gameplay" {
                    ui.label(
                        RichText::new(" Gameplay ").small().background_color(style::WARN).color(style::BACKGROUND),
                    );
                }
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(RichText::new("by").small().color(style::DIM));
                if ui
                    .add(egui::Button::new(RichText::new(&p.author.name).small().color(style::GOLD_LIGHT)).frame(false))
                    .clicked()
                {
                    author = Some(p.author.id.clone());
                }
                ui.label(
                    RichText::new(format!(
                        "\u{b7}  {}  \u{b7}  {:.1} MB  \u{b7}  submitted {}",
                        p.id,
                        p.size as f64 / 1e6,
                        super::account::day(&p.submitted)
                    ))
                    .small()
                    .color(style::DIM),
                );
            });
            if !p.description.is_empty() {
                ui.label(&p.description);
            }
            // the author's statement
            let prov = &p.provenance;
            let mut said = vec![if prov.own_work { "own work: yes" } else { "own work: NOT DECLARED" }.to_string()];
            if prov.recipes {
                said.push("uses recipes on the player's files".into());
            }
            said.push(format!("licence: {}", p.licence));
            ui.label(RichText::new(said.join("  \u{b7}  ")).small());
            if !prov.credits.is_empty() {
                ui.label(RichText::new(format!("Credits: {}", prov.credits)).small().color(style::DIM));
            }
            let mut facts = vec![format!("remaster {}", if p.engine.is_empty() { "any" } else { &p.engine })];
            if !p.requires.is_empty() {
                facts.push(format!("needs {}", p.requires.join(", ")));
            }
            if !p.conflicts.is_empty() {
                facts.push(format!("not with {}", p.conflicts.join(", ")));
            }
            facts.push(format!("from {}", p.url));
            ui.label(RichText::new(facts.join("  \u{b7}  ")).small().color(style::DIM));
            for s in &p.screenshots {
                ui.hyperlink_to(RichText::new(format!("Screenshot: {s}")).small(), s);
            }
            // the file: checked, then tried
            match self.review_view.checks.get(&p.listing) {
                Some(Ok(c)) => {
                    ui.label(
                        RichText::new(format!("The file is the one submitted. It changes: {}.", c.fills))
                            .color(style::GOOD),
                    );
                    for (sev, text) in &c.problems {
                        let colour = if *sev == qwc_packs::Severity::Error { style::BAD } else { style::WARN };
                        ui.label(RichText::new(text).small().color(colour));
                    }
                    if ui
                        .add_enabled(self.packs_view.installing.is_none(), egui::Button::new("Install to try"))
                        .on_hover_text("Installs it as a pack, on at the top, to look at in the game")
                        .clicked()
                    {
                        let file = c.file.clone();
                        if let Some(dir) = self.packs_profile().packs_dir {
                            self.packs_view.installing =
                                Some(Job::start(&ctx, move |_| qwc_packs::install(&file, &dir)));
                            self.review_view.message =
                                Some((true, format!("{} is being installed: Packs, Your packs.", p.name)));
                        }
                    }
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(format!("The file doesn't check: {e}")).color(style::BAD));
                }
                None => {}
            }
            let checking_this = self.review_view.checking.as_ref().is_some_and(|(id, _)| *id == p.listing);
            if checking_this {
                if let Some((_, job)) = &self.review_view.checking {
                    job.bar(ui, "Checking");
                }
            } else if ui
                .add_enabled(self.review_view.checking.is_none(), egui::Button::new("Download and check the file"))
                .clicked()
            {
                if let Some(library) = self.library.clone() {
                    let pending = p.clone();
                    let downloads = library.join("downloads");
                    self.review_view.checking = Some((
                        p.listing,
                        Job::start(&ctx, move |progress| {
                            store::check_submission(&pending.target(), &downloads, progress)
                        }),
                    ));
                }
            }
            // the decision
            if own {
                ui.label(RichText::new("Your own pack: another reviewer reviews it.").color(style::DIM));
                return;
            }
            let note = self.review_view.notes.entry(p.listing).or_default();
            let l = ui.label("Note for the author (needed to reject)");
            ui.add(egui::TextEdit::singleline(note).desired_width(f32::INFINITY).char_limit(1000)).labelled_by(l.id);
            let note = note.trim().to_string();
            let checked = matches!(self.review_view.checks.get(&p.listing), Some(Ok(_)));
            let mut decision = None;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(checked && !busy, style::primary("Approve", 17.0))
                    .on_disabled_hover_text("Download and check the file first")
                    .clicked()
                {
                    decision = Some(true);
                }
                if ui.add_enabled(!note.is_empty() && !busy, egui::Button::new("Reject")).clicked() {
                    decision = Some(false);
                }
            });
            if let Some(approve) = decision {
                let (listing, name) = (p.listing, format!("{} {}", p.name, p.version));
                self.act(&ctx, move |base, token| {
                    account::decide(base, token, listing, approve, &note)
                        .map(|()| format!("{name} is {}.", if approve { "approved and listed" } else { "rejected" }))
                        .map_err(String::from)
                });
            }
        });
        if let Some(id) = author {
            self.open_profile(&ctx, &id);
        }
    }

    fn report_tile(&mut self, ui: &mut egui::Ui, r: &OpenReport, admin: bool) {
        let ctx = ui.ctx().clone();
        let art = self.art.clone();
        let busy = self.review_view.acting.is_some();
        let reason = account::REASONS.iter().find(|(k, _)| *k == r.reason).map_or(r.reason.as_str(), |(_, s)| s);
        let mut action: Option<(&'static str, &'static str)> = None;
        let mut open = false;
        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            let kind = match r.target_kind.as_str() {
                "listing" => "Pack",
                "collection" => "Collection",
                _ => "Player",
            };
            ui.horizontal(|ui| {
                ui.label(style::heading(format!("{kind}: {}", r.about.as_deref().unwrap_or("(gone)")), 20.0));
                if let Some(s) = &r.state {
                    ui.label(
                        RichText::new(format!(" {s} "))
                            .small()
                            .background_color(style::GOLD_DEEP)
                            .color(style::GOLD_LIGHT),
                    );
                }
            });
            ui.label(RichText::new(reason).color(style::WARN));
            if !r.note.is_empty() {
                ui.label(&r.note);
            }
            ui.label(
                RichText::new(format!(
                    "Reported by {} on {}",
                    r.reporter.as_deref().unwrap_or("a deleted account"),
                    super::account::day(&r.at)
                ))
                .small()
                .color(style::DIM),
            );
            ui.horizontal(|ui| {
                if r.target_kind == "account" && ui.button("Open the profile").clicked() {
                    open = true;
                }
                if ui.add_enabled(!busy, egui::Button::new("Dismiss")).clicked() {
                    action = Some(("dismiss", "dismissed"));
                }
                match r.target_kind.as_str() {
                    "listing" | "collection" => {
                        if ui.add_enabled(!busy, egui::Button::new("Remove it from the store")).clicked() {
                            action = Some(("remove", "removed from the store"));
                        }
                    }
                    _ => {
                        if ui.add_enabled(!busy, egui::Button::new("Reset their picture")).clicked() {
                            action = Some(("reset-picture", "back to their Discord avatar"));
                        }
                        if admin && ui.add_enabled(!busy, egui::Button::new("Close the account")).clicked() {
                            action = Some(("close", "closed"));
                        }
                    }
                }
            });
        });
        if open {
            self.open_profile(&ctx, &r.target_id);
        }
        if let Some((what, done)) = action {
            let (id, about) = (r.id, r.about.clone().unwrap_or_default());
            self.act(&ctx, move |base, token| {
                account::resolve_report(base, token, id, what)
                    .map(|()| format!("{about}: {done}."))
                    .map_err(String::from)
            });
        }
    }

    /// An admin's button on another player's profile: make them a reviewer, or a player again.
    pub(super) fn role_button(&mut self, ui: &mut egui::Ui, id: &str, role: &str) {
        let admin = self.account_view.session.as_ref().is_some_and(|(_, m)| m.role == "admin" && m.id != id);
        if !admin || role == "admin" {
            return;
        }
        let ctx = ui.ctx().clone();
        let reviewer = role == "reviewer";
        let label = if reviewer { "Remove the reviewer role" } else { "Make a reviewer" };
        if ui.add_enabled(self.review_view.acting.is_none(), egui::Button::new(label)).clicked() {
            let id = id.to_string();
            self.act(&ctx, move |base, token| {
                account::set_role(base, token, &id, !reviewer)
                    .map(|()| {
                        if reviewer {
                            "They're a player again.".to_string()
                        } else {
                            "They're a reviewer now.".to_string()
                        }
                    })
                    .map_err(String::from)
            });
        }
    }
}

impl Launcher {
    /// The Review page on these as if read, signed in as `me` (the screenshots); `checked`: the first in the queue's
    /// file as if checked.
    pub fn show_review(
        &mut self,
        me: (String, account::Me),
        queue: Vec<Pending>,
        reports: Vec<OpenReport>,
        checked: Option<Checked>,
    ) {
        self.account_view.session = Some(me.clone());
        self.review_view.loaded_for = Some(me.1.id);
        if let (Some(c), Some(first)) = (checked, queue.first()) {
            self.review_view.checks.insert(first.listing, Ok(c));
        }
        self.review_view.queue = Some(Ok(queue));
        self.review_view.reports = Some(Ok(reports));
        self.page = Page::Review;
    }

    /// Signed in as `me` with these ratings and submissions (the screenshots).
    pub fn show_mine(&mut self, me: (String, account::Me), ratings: Vec<(String, u8)>, mine: Vec<MySubmission>) {
        self.review_view.loaded_for = Some(me.1.id.clone());
        self.account_view.session = Some(me);
        self.review_view.ratings = ratings.into_iter().collect();
        self.review_view.mine = Some(Ok(mine));
    }

    /// The Submit to the store dialog open on `file` (the screenshots).
    pub fn show_submit(&mut self, file: PathBuf, url: &str) {
        let read = account::read_pack_file(&file);
        let description = read.as_ref().map(|p| p.manifest.description.clone()).unwrap_or_default();
        self.review_view.submit =
            Some(Submit { file: Some(file), read: Some(read), url: url.into(), description, ..Submit::default() });
        self.page = Page::Packs;
    }
}
