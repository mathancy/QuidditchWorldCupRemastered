//! The player's pack store account in the launcher (`docs/PACK_STORE.md`, "Accounts" and "Profiles"): the bottom
//! strip's account slot (the picture, the name, Log in or Log out), signing in through the browser, and the Profile
//! page: anyone's, with the packs they share; your own adds Edit profile, the picture, Log out and Delete.

use std::collections::HashMap;

use eframe::egui::{self, pos2, vec2, FontId, Rect, RichText, Sense, Stroke};

use super::{Job, Launcher, Page};
use crate::account::{self, Fail, Me, Profile};
use crate::{launch, store, style};

/// A picture chosen to upload: its pixels, where its square sits (0 to 1 along the long side), and the preview.
struct Upload {
    rgba: Vec<u8>,
    w: u32,
    h: u32,
    focus: f32,
    preview: Option<(f32, egui::TextureHandle)>,
}

#[derive(Default)]
pub(super) struct AccountView {
    /// The signed-in session: its token and the account.
    pub(super) session: Option<(String, Me)>,
    /// Whether the kept token was looked at yet, and the store's answer about it.
    started: bool,
    resuming: Option<Job<Option<(String, Me)>>>,
    signing_in: Option<Job<(String, Me)>>,
    saving: Option<Job<Me>>,
    /// Pictures by account: loaded (none if it would not load), and the ones loading.
    pictures: HashMap<String, Option<egui::TextureHandle>>,
    loading: HashMap<String, Job<Vec<u8>>>,
    /// Pictures given as pixels, made textures when first drawn (the screenshots).
    given: HashMap<String, (Vec<u8>, u32, u32)>,
    pub(super) message: Option<(bool, String)>,
    /// The Profile page: whose, and what the store said.
    pub(super) shown: Option<String>,
    pub(super) profile: Option<Result<Profile, String>>,
    profile_job: Option<Job<Profile>>,
    /// Edit profile: the name typed, a picture chosen (being read, then previewed), an upload, deleting.
    name: String,
    reading: Option<Job<(Vec<u8>, u32, u32)>>,
    upload: Option<Upload>,
    uploading: Option<Job<()>>,
    confirm_delete: bool,
}

impl AccountView {
    pub(super) fn busy(&self) -> bool {
        self.resuming.is_some() || self.signing_in.is_some() || self.saving.is_some() || self.uploading.is_some()
    }
}

/// "24 Oct 2026" for an ISO time.
pub(super) fn day(iso: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mut parts = iso.get(..10).unwrap_or(iso).split('-');
    match (parts.next(), parts.next().and_then(|m| m.parse::<usize>().ok()), parts.next()) {
        (Some(y), Some(m @ 1..=12), Some(d)) => format!("{} {} {y}", d.trim_start_matches('0'), MONTHS[m - 1]),
        _ => iso.to_string(),
    }
}

/// The texture of an RGBA image.
fn texture(ctx: &egui::Context, name: &str, rgba: &[u8], w: u32, h: u32) -> egui::TextureHandle {
    let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba);
    ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
}

/// A round picture `side` points across at `centre`: the texture, else a silhouette.
fn round_picture(
    ui: &mut egui::Ui,
    art: &qwc_style::ornate::Art,
    tex: Option<&egui::TextureHandle>,
    centre: egui::Pos2,
    side: f32,
) {
    let r = Rect::from_center_size(centre, vec2(side, side));
    match tex {
        Some(t) => {
            ui.put(r, egui::Image::new(t).fit_to_exact_size(r.size()).corner_radius(side / 2.0));
        }
        None => {
            ui.painter().circle_filled(centre, side / 2.0, style::WELL.gamma_multiply(0.8));
            qwc_style::ornate::icon(ui.painter(), art, "icon_profile", r.shrink(side * 0.24), style::GOLD);
        }
    }
    ui.painter().circle_stroke(centre, side / 2.0, Stroke::new(1.5, style::GOLD_DEEP));
}

impl Launcher {
    fn library_or_message(&mut self) -> Option<std::path::PathBuf> {
        let l = self.library.clone();
        if l.is_none() {
            self.account_view.message = Some((false, "No library folder to keep the sign-in in.".into()));
        }
        l
    }

    /// Sign in through the browser: the store's page opens, and the launcher waits for it to come back.
    pub(super) fn sign_in(&mut self, ctx: &egui::Context) {
        let Some(library) = self.library_or_message() else { return };
        self.account_view.message = None;
        let (base, root) = (store::store_url(), self.root.clone());
        self.account_view.signing_in = Some(Job::start(ctx, move |p| {
            let (token, me) = account::sign_in(&base, &p.cancel, Some(root), launch::open_url)?;
            account::store_token(&library, &token)?;
            Ok((token, me))
        }));
    }

    /// Log out: the session ended on the store (if it answers) and forgotten here.
    pub(super) fn sign_out(&mut self) {
        if let Some((token, _)) = self.account_view.session.take() {
            let base = store::store_url();
            std::thread::spawn(move || account::log_out(&base, &token));
        }
        if let Some(l) = &self.library {
            account::forget_token(l);
        }
        self.account_view.message = Some((true, "Logged out.".into()));
    }

    /// The account's jobs' answers; the kept token looked at on the first frame.
    pub(super) fn poll_account(&mut self, ctx: &egui::Context) {
        let v = &mut self.account_view;
        if !v.started {
            v.started = true;
            if let Some(token) = self.library.as_deref().and_then(account::stored_token) {
                let (base, library) = (store::store_url(), self.library.clone());
                v.resuming = Some(Job::start(ctx, move |_| match account::me(&base, &token) {
                    Ok(me) => Ok(Some((token, me))),
                    Err(Fail::SignedOut) => {
                        if let Some(l) = &library {
                            account::forget_token(l);
                        }
                        Ok(None)
                    }
                    Err(Fail::Said(e)) => Err(e),
                }));
            }
        }
        if let Some(r) = v.resuming.as_ref().and_then(Job::take) {
            v.resuming = None;
            match r {
                Ok(session) => v.session = session,
                // the store not answering: the token stays for next time
                Err(e) => v.message = Some((false, format!("Couldn't reach the store: {e}"))),
            }
        }
        if let Some(r) = v.signing_in.as_ref().and_then(Job::take) {
            v.signing_in = None;
            match r {
                Ok((token, me)) => {
                    v.message = Some((true, format!("Signed in as {}.", me.name)));
                    v.name = me.name.clone();
                    v.session = Some((token, me));
                }
                Err(e) if e == "stopped" => {}
                Err(e) => v.message = Some((false, e)),
            }
        }
        if let Some(r) = v.saving.as_ref().and_then(Job::take) {
            v.saving = None;
            match r {
                Ok(me) => {
                    v.name = me.name.clone();
                    if let Some((_, m)) = &mut v.session {
                        *m = me;
                    }
                    v.message = Some((true, "Saved.".into()));
                    self.reload_profile(ctx);
                }
                Err(e) => self.account_view.message = Some((false, e)),
            }
        }
        let v = &mut self.account_view;
        if let Some(r) = v.uploading.as_ref().and_then(Job::take) {
            v.uploading = None;
            match r {
                Ok(()) => {
                    v.upload = None;
                    v.message = Some((true, "Your picture is changed.".into()));
                    // read again: the slot's and the profile's
                    if let Some((_, me)) = &v.session {
                        v.pictures.remove(&me.id);
                    }
                    self.refresh_me(ctx);
                }
                Err(e) => v.message = Some((false, e)),
            }
        }
        let v = &mut self.account_view;
        if let Some(r) = v.reading.as_ref().and_then(Job::take) {
            v.reading = None;
            match r {
                Ok((rgba, w, h)) => v.upload = Some(Upload { rgba, w, h, focus: 0.5, preview: None }),
                Err(e) => v.message = Some((false, e)),
            }
        }
        if let Some(r) = v.profile_job.as_ref().and_then(Job::take) {
            v.profile_job = None;
            v.profile = Some(r);
        }
        let done: Vec<String> = v
            .loading
            .iter()
            .filter(|(_, j)| j.answer.lock().is_ok_and(|a| a.is_some()))
            .map(|(id, _)| id.clone())
            .collect();
        for id in done {
            let Some(job) = v.loading.remove(&id) else { continue };
            let tex = job
                .take()
                .and_then(|r| r.ok())
                .and_then(|bytes| account::decode_image(&bytes).ok())
                .map(|(rgba, w, h)| texture(ctx, &format!("picture {id}"), &rgba, w, h));
            v.pictures.insert(id, tex);
        }
    }

    /// The account read again (after a change of picture).
    fn refresh_me(&mut self, ctx: &egui::Context) {
        let Some((token, _)) = self.account_view.session.clone() else { return };
        let base = store::store_url();
        self.account_view.saving = Some(Job::start(ctx, move |_| account::me(&base, &token).map_err(String::from)));
    }

    /// The picture of account `id`, asked for the first time it is wanted.
    pub(super) fn picture(&mut self, ctx: &egui::Context, id: &str) -> Option<egui::TextureHandle> {
        let v = &mut self.account_view;
        if let Some((rgba, w, h)) = v.given.remove(id) {
            v.pictures.insert(id.to_string(), Some(texture(ctx, &format!("picture {id}"), &rgba, w, h)));
        }
        if let Some(t) = v.pictures.get(id) {
            return t.clone();
        }
        if !v.loading.contains_key(id) {
            let (base, who) = (store::store_url(), id.to_string());
            v.loading
                .insert(id.to_string(), Job::start(ctx, move |_| account::picture(&base, &who).map_err(String::from)));
        }
        None
    }

    /// Open the Profile page on account `id`.
    pub(super) fn open_profile(&mut self, ctx: &egui::Context, id: &str) {
        self.page = Page::Profile;
        let v = &mut self.account_view;
        v.shown = Some(id.to_string());
        v.profile = None;
        v.upload = None;
        v.confirm_delete = false;
        if let Some((_, me)) = &v.session {
            v.name = me.name.clone();
        }
        self.reload_profile(ctx);
    }

    pub(super) fn reload_profile(&mut self, ctx: &egui::Context) {
        let Some(id) = self.account_view.shown.clone() else { return };
        let base = store::store_url();
        self.account_view.profile_job =
            Some(Job::start(ctx, move |_| account::profile(&base, &id).map_err(String::from)));
    }

    /// The bottom strip's account section at `x`: the picture, "Account", the name, Log in or Log out.
    pub(super) fn account_slot(&mut self, ui: &mut egui::Ui, x: f32, rect: Rect) {
        let ctx = ui.ctx().clone();
        let art = self.art.clone();
        let me = self.account_view.session.as_ref().map(|(_, m)| m.clone());
        let tex = me.as_ref().and_then(|m| self.picture(&ctx, &m.id));
        let centre = pos2(x + 30.0, rect.center().y);
        round_picture(ui, &art, tex.as_ref(), centre, 52.0);
        let p = ui.painter().clone();
        let left = x + 30.0 + 26.0 + 16.0;
        let mut y = rect.top() + 14.0;
        let dim = p.layout_no_wrap("Account".into(), FontId::proportional(14.0), style::DIM);
        p.galley(pos2(left, y), dim.clone(), style::DIM);
        y += dim.size().y + 1.0;
        let busy = self.account_view.signing_in.is_some() || self.account_view.resuming.is_some();
        let (name, colour) = match (&me, busy) {
            (Some(m), _) => (m.name.clone(), style::TEXT),
            (None, true) => ("Signing in...".to_string(), style::DIM),
            (None, false) => ("Not signed in".to_string(), style::TEXT),
        };
        let g = p.layout_no_wrap(name, FontId::proportional(19.0), colour);
        let name_rect = Rect::from_min_size(pos2(left, y), g.size());
        p.galley(name_rect.min, g.clone(), colour);
        y += g.size().y + 1.0;
        if let Some(m) = &me {
            // the picture and the name open the profile
            let area = Rect::from_min_max(
                pos2(x, rect.top() + 10.0),
                pos2(name_rect.right().max(left + 60.0), name_rect.bottom()),
            );
            let r = ui.interact(area, ui.scope_id().with("account slot"), Sense::click());
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Your profile"));
            if r.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if r.clicked() {
                let id = m.id.clone();
                self.open_profile(&ctx, &id);
            }
        }
        let link = Rect::from_min_size(pos2(left, y - 2.0), vec2(160.0, 20.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(link), |ui| {
            ui.spacing_mut().button_padding = vec2(0.0, 0.0);
            let small = |t: &str| egui::Button::new(RichText::new(t).size(14.0).color(style::GOLD_LIGHT)).frame(false);
            if self.account_view.signing_in.is_some() {
                if ui.add(small("Stop signing in")).clicked() {
                    if let Some(j) = &self.account_view.signing_in {
                        j.progress.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            } else if let Some(m) = &me {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if ui.add(small("Log out")).clicked() {
                        self.sign_out();
                    }
                    if (m.role == "reviewer" || m.role == "admin") && ui.add(small("Review")).clicked() {
                        self.open_review(&ctx);
                    }
                });
            } else if !busy && ui.add(small("Log in with Discord")).clicked() {
                self.sign_in(&ctx);
            }
        });
    }

    /// The Profile page.
    pub(super) fn profile_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let art = self.art.clone();
        if let Some((ok, m)) = &self.account_view.message {
            ui.label(RichText::new(m).color(if *ok { style::GOOD } else { style::BAD }));
        }
        let Some(shown) = self.account_view.shown.clone() else { return };
        let mine = self.account_view.session.as_ref().is_some_and(|(_, m)| m.id == shown);
        let profile = match &self.account_view.profile {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Reading the profile...");
                });
                return;
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(style::WARN));
                return;
            }
            Some(Ok(p)) => p.clone(),
        };
        let tex = self.picture(&ctx, &profile.id);
        // the picture, the name, the Discord username, the date joined
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(112.0, 112.0), Sense::hover());
            round_picture(ui, &art, tex.as_ref(), r.center(), 104.0);
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.label(style::heading(&profile.name, 36.0));
                    if profile.role != "player" {
                        let role = if profile.role == "admin" { " Admin " } else { " Reviewer " };
                        ui.label(
                            RichText::new(role).small().background_color(style::GOLD_DEEP).color(style::GOLD_LIGHT),
                        );
                    }
                });
                if let Some(d) = &profile.discord_username {
                    ui.label(RichText::new(format!("@{d} on Discord")).color(style::DIM));
                }
                ui.label(RichText::new(format!("Joined {}", day(&profile.joined))).color(style::DIM));
            });
        });
        ui.add_space(10.0);
        if mine {
            self.edit_profile(ui);
            ui.add_space(10.0);
            if profile.role == "reviewer" || profile.role == "admin" {
                if ui.add(style::primary("Open the Review page", 18.0)).clicked() {
                    self.open_review(&ctx);
                }
                ui.add_space(10.0);
            }
            self.my_submissions(ui);
            ui.add_space(10.0);
        } else {
            ui.horizontal(|ui| {
                self.role_button(ui, &profile.id, &profile.role);
                if self.account_view.session.is_some() && ui.small_button("Report this player").clicked() {
                    self.open_report("account", &profile.id, &format!("The player {}", profile.name));
                }
            });
            ui.add_space(10.0);
        }
        ui.label(style::heading(if mine { "Your packs" } else { "Packs" }, 24.0));
        if profile.packs.is_empty() {
            ui.label(RichText::new("No packs shared yet.").color(style::DIM));
        }
        for p in &profile.packs {
            style::tile(ui, &art, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(style::heading(&p.name, 20.0));
                    if p.kind == "Gameplay" {
                        ui.label(
                            RichText::new(" Gameplay ").small().background_color(style::WARN).color(style::BACKGROUND),
                        );
                    }
                });
                let rating = p
                    .rating
                    .average
                    .map_or("not rated yet".to_string(), |a| format!("\u{2605} {a:.1} ({})", p.rating.count));
                ui.label(
                    RichText::new(format!(
                        "{}  \u{b7}  {rating}  \u{b7}  {}",
                        p.version,
                        if p.installs == 1 { "1 install".to_string() } else { format!("{} installs", p.installs) }
                    ))
                    .small()
                    .color(style::DIM),
                );
                if !p.description.is_empty() {
                    ui.label(&p.description);
                }
            });
            ui.add_space(6.0);
        }
        ui.add_space(8.0);
        ui.label(style::heading(if mine { "Your collections" } else { "Collections" }, 24.0));
        self.profile_collections(ui, &profile.collections, mine);
    }

    /// Your own profile's settings: the name, showing the Discord username, the picture, Log out and Delete.
    fn edit_profile(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let art = self.art.clone();
        let Some((token, me)) = self.account_view.session.clone() else { return };
        let busy = self.account_view.busy();
        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            ui.label(style::heading("Edit profile", 22.0));
            // the name
            ui.horizontal(|ui| {
                let l = ui.label("Your name in the store");
                ui.add(egui::TextEdit::singleline(&mut self.account_view.name).desired_width(240.0).char_limit(24))
                    .labelled_by(l.id);
                let changed = self.account_view.name.trim() != me.name;
                let allowed = me.name_change_after.is_none() || self.account_view.name.eq_ignore_ascii_case(&me.name);
                if ui.add_enabled(changed && allowed && !busy, egui::Button::new("Save name")).clicked() {
                    let (base, token, name) =
                        (store::store_url(), token.clone(), self.account_view.name.trim().to_string());
                    self.account_view.saving = Some(Job::start(&ctx, move |_| {
                        account::edit(&base, &token, Some(&name), None).map_err(String::from)
                    }));
                }
            });
            let when = match &me.name_change_after {
                Some(after) => format!("You can change your name again on {}.", day(after)),
                None => "You can change your name once every 30 days.".to_string(),
            };
            ui.label(RichText::new(when).small().color(style::DIM));
            // the Discord username
            if let Some(d) = &me.discord_username {
                let mut show = me.show_discord;
                if ui
                    .add_enabled(
                        !busy,
                        egui::Checkbox::new(&mut show, format!("Show my Discord username (@{d}) on my profile")),
                    )
                    .changed()
                {
                    let (base, token) = (store::store_url(), token.clone());
                    self.account_view.saving = Some(Job::start(&ctx, move |_| {
                        account::edit(&base, &token, None, Some(show)).map_err(String::from)
                    }));
                }
            }
            ui.add_space(6.0);
            self.picture_settings(ui, &token, &me);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new("Log out")).clicked() {
                    self.sign_out();
                    self.page = Page::Home;
                }
                if ui.add_enabled(!busy, egui::Button::new("Delete my account...")).clicked() {
                    self.account_view.confirm_delete = true;
                }
            });
        });
        self.confirm_delete(&ctx, &token);
    }

    /// The picture: upload one (read, a square chosen, previewed, then sent), or go back to the Discord avatar.
    fn picture_settings(&mut self, ui: &mut egui::Ui, token: &str, me: &Me) {
        let ctx = ui.ctx().clone();
        let busy = self.account_view.busy() || self.account_view.reading.is_some();
        ui.label(RichText::new("Your picture").strong());
        if let Some(up) = &mut self.account_view.upload {
            // the square, moved along the long side, previewed at 128 points
            if up.preview.as_ref().is_none_or(|(f, _)| *f != up.focus) {
                let sq = account::square(&up.rgba, up.w, up.h, up.focus, account::PICTURE_SIDE);
                up.preview = Some((
                    up.focus,
                    texture(&ctx, "picture preview", &sq, account::PICTURE_SIDE, account::PICTURE_SIDE),
                ));
            }
            let mut send = false;
            let mut cancel = false;
            ui.horizontal(|ui| {
                if let Some((_, t)) = &up.preview {
                    ui.add(egui::Image::new(t).fit_to_exact_size(vec2(128.0, 128.0)).corner_radius(64.0));
                }
                ui.vertical(|ui| {
                    if up.w != up.h {
                        let l = ui.label(if up.w > up.h { "Left to right" } else { "Top to bottom" });
                        ui.add(egui::Slider::new(&mut up.focus, 0.0..=1.0).show_value(false)).labelled_by(l.id);
                    }
                    ui.label(
                        RichText::new("Shown at 256 x 256; only the picture's pixels are sent.")
                            .small()
                            .color(style::DIM),
                    );
                    ui.horizontal(|ui| {
                        send = ui.add_enabled(!busy, style::primary("Upload", 17.0)).clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
            });
            if send {
                let sq = account::square(&up.rgba, up.w, up.h, up.focus, account::PICTURE_SIDE);
                let (base, token) = (store::store_url(), token.to_string());
                self.account_view.uploading = Some(Job::start(&ctx, move |_| {
                    let (bytes, mime) = account::encode_picture(&sq)?;
                    account::upload_picture(&base, &token, &bytes, mime).map_err(String::from)
                }));
            }
            if cancel {
                self.account_view.upload = None;
            }
            return;
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("Upload a picture...")).clicked() {
                if let Some(f) = rfd::FileDialog::new().add_filter("Picture", &["png", "jpg", "jpeg"]).pick_file() {
                    self.account_view.reading = Some(Job::start(&ctx, move |_| account::open_picture(&f)));
                }
            }
            if me.uploaded_avatar && ui.add_enabled(!busy, egui::Button::new("Use my Discord avatar")).clicked() {
                let (base, token) = (store::store_url(), token.to_string());
                self.account_view.uploading =
                    Some(Job::start(&ctx, move |_| account::reset_picture(&base, &token).map_err(String::from)));
            }
            if self.account_view.reading.is_some() {
                ui.spinner();
            }
        });
        let note =
            if me.uploaded_avatar { "Your uploaded picture." } else { "Your Discord avatar, until you upload one." };
        ui.label(RichText::new(note).small().color(style::DIM));
    }

    fn confirm_delete(&mut self, ctx: &egui::Context, token: &str) {
        if !self.account_view.confirm_delete {
            return;
        }
        let modal = egui::Modal::new(egui::Id::unique("delete account")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(style::heading("Delete your account?", 24.0));
            ui.label("Your name, picture and Discord details are deleted from the store, your ratings with them, and the packs you shared are no longer listed. Your packs on this computer stay.");
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
        match modal.inner.or(modal.should_close().then_some(false)) {
            Some(true) => {
                self.account_view.confirm_delete = false;
                let (base, token) = (store::store_url(), token.to_string());
                match account::delete(&base, &token) {
                    Ok(()) => {
                        self.account_view.session = None;
                        if let Some(l) = &self.library {
                            account::forget_token(l);
                        }
                        self.account_view.message = Some((true, "Your account is deleted.".into()));
                        self.page = Page::Home;
                    }
                    Err(e) => self.account_view.message = Some((false, e.to_string())),
                }
            }
            Some(false) => self.account_view.confirm_delete = false,
            None => {}
        }
    }

    /// The Profile page on these as if fetched, signed in as `me` when given (the screenshots).
    pub fn show_profile(
        &mut self,
        profile: Profile,
        me: Option<(String, Me)>,
        pictures: Vec<(String, Vec<u8>, u32, u32)>,
    ) {
        self.show_signed_in(me, pictures);
        self.account_view.shown = Some(profile.id.clone());
        self.account_view.profile = Some(Ok(profile));
        self.page = Page::Profile;
    }

    /// Signed in as `me` (or not) without asking the store, with these pictures (the screenshots).
    pub fn show_signed_in(&mut self, me: Option<(String, Me)>, pictures: Vec<(String, Vec<u8>, u32, u32)>) {
        for (id, rgba, w, h) in pictures {
            self.account_view.given.insert(id, (rgba, w, h));
        }
        self.account_view.started = true;
        if let Some((_, m)) = &me {
            self.account_view.name = m.name.clone();
        }
        self.account_view.session = me;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn days() {
        assert_eq!(super::day("2026-10-04T10:00:00Z"), "4 Oct 2026");
        assert_eq!(super::day("2026-01-24"), "24 Jan 2026");
        assert_eq!(super::day("soon"), "soon");
    }
}
