//! The launcher's window: its own frame and top bar (`chrome`), the setup on the first run (the exe, the game files,
//! the import), then the Play page (`home`), the Online page (`online`), the Packs page (`packs`), the Settings page
//! (`settings`), the Saves page (`saves`) and the News page (`news`).

mod account;
mod chrome;
mod collections;
mod home;
mod news;
mod online;
mod packs;
mod review;
mod saves;
mod settings;
mod shape;
mod store;

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};

use crate::setup::{self, ExeCheck, ExeVerdict, GameCheck, Progress};
use crate::style;

/// Where the retail installer puts the game, looked at first.
const INSTALLED: &str = r"C:\Program Files (x86)\EA GAMES\Harry Potter - Quidditch WC";

/// Work on a thread: how far it is and, when it is done, its answer.
struct Job<T> {
    progress: Arc<Progress>,
    answer: Arc<Mutex<Option<Result<T, String>>>>,
}

impl<T: Send + 'static> Job<T> {
    fn start(ctx: &egui::Context, work: impl FnOnce(&Progress) -> Result<T, String> + Send + 'static) -> Job<T> {
        let progress = Arc::new(Progress::default());
        let answer = Arc::new(Mutex::new(None));
        let (p, a, ctx) = (progress.clone(), answer.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = work(&p);
            if let Ok(mut slot) = a.lock() {
                *slot = Some(r);
            }
            ctx.request_repaint();
        });
        Job { progress, answer }
    }

    /// The answer, the first time it is asked for after the work is done.
    fn take(&self) -> Option<Result<T, String>> {
        self.answer.lock().ok()?.take()
    }

    /// A bar for the work so far.
    fn bar(&self, ui: &mut egui::Ui, doing: &str) {
        let what = self.progress.what.lock().map(|w| w.clone()).unwrap_or_default();
        let f = self.progress.fraction();
        ui.add(egui::ProgressBar::new(f).fill(style::GOLD).text(format!("{doing} {:.0}%  {what}", f * 100.0)));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Setup,
    Home,
    Online,
    Packs,
    Settings,
    Saves,
    News,
    /// A player's profile in the pack store (yours, or a pack's author's).
    Profile,
    /// The store's reviewers' page: the queue and the reports.
    Review,
}

pub struct Launcher {
    /// The folder of the shipped files (the fonts), and whether the style is set on the window's context yet.
    root: PathBuf,
    styled: bool,
    /// A game folder to check on the first frame (the usual install, found on its own).
    check_soon: bool,
    /// The remaster's folder in the user's profile, and its profile.
    library: Option<PathBuf>,
    profile: qwc_packs::Profile,
    page: Page,
    exe_path: String,
    exe: Option<ExeCheck>,
    game_path: String,
    checking: Option<Job<GameCheck>>,
    game: Option<Result<GameCheck, String>>,
    importing: Option<Job<PathBuf>>,
    import_error: Option<String>,
    /// The game while it runs, and what the last run said.
    running: Option<Child>,
    status: String,
    logo: Option<egui::TextureHandle>,
    /// The art the look uses where it has it (`art/` beside the launcher's sources, shipped with it).
    art: qwc_style::ornate::Art,
    home_view: home::HomeView,
    /// What the window's corners were last cut for (`shape`).
    shaped: shape::Shaped,
    packs_view: packs::PacksView,
    settings_view: settings::SettingsView,
    saves_view: saves::SavesView,
    news_view: news::NewsView,
    online_view: online::OnlineView,
    account_view: account::AccountView,
    review_view: review::ReviewView,
    /// Look for a new release on the first frame (the window's launcher, when the settings say so).
    news_soon: bool,
}

impl Launcher {
    /// The launcher on `library` (none: no library folder in this environment), styled from the shipped files
    /// under `root` on its first frame.
    pub fn new(root: &Path, library: Option<PathBuf>) -> Launcher {
        let profile = library
            .as_ref()
            .map(|l| l.join("profile.ron"))
            .filter(|p| p.is_file())
            .and_then(|p| qwc_packs::Profile::read(&p).ok())
            .unwrap_or_default();
        let mut l = Launcher {
            root: root.to_path_buf(),
            styled: false,
            check_soon: false,
            library,
            profile,
            page: Page::Setup,
            exe_path: String::new(),
            exe: None,
            game_path: String::new(),
            checking: None,
            game: None,
            importing: None,
            import_error: None,
            running: None,
            status: String::new(),
            logo: None,
            art: qwc_style::ornate::Art::default(),
            home_view: home::HomeView::default(),
            shaped: None,
            packs_view: packs::PacksView::default(),
            settings_view: settings::SettingsView::load(),
            saves_view: saves::SavesView::default(),
            news_view: news::NewsView::default(),
            online_view: online::OnlineView::default(),
            account_view: account::AccountView::default(),
            review_view: review::ReviewView::default(),
            news_soon: false,
        };
        if l.ready() {
            l.page = Page::Home;
            if let Some(exe) = l.profile.exe.clone() {
                l.exe = Some(setup::check_exe(&exe));
            }
        } else if Path::new(INSTALLED).is_dir() {
            // the usual install: its folder, and its exe (often an unwrapped one where the game runs without the
            // disc)
            l.game_path = INSTALLED.into();
            l.set_exe(Path::new(INSTALLED).join("QWC.exe"));
            l.check_soon = true;
        }
        l
    }

    /// Whether the profile has imported game files and an exe.
    fn ready(&self) -> bool {
        self.profile.data.as_ref().is_some_and(|d| d.is_dir()) && self.profile.exe.as_ref().is_some_and(|e| e.is_file())
    }

    fn profile_path(&self) -> Option<PathBuf> {
        self.library.as_ref().map(|l| l.join("profile.ron"))
    }

    /// The profile with its packs' folder filled in: its own, else this library's `packs` (not written back).
    fn packs_profile(&self) -> qwc_packs::Profile {
        let mut p = self.profile.clone();
        if p.packs_dir.is_none() {
            p.packs_dir = self.library.as_ref().map(|l| l.join("packs"));
        }
        p
    }

    fn set_exe(&mut self, path: PathBuf) {
        self.exe_path = path.display().to_string();
        self.exe = Some(setup::check_exe(&path));
    }

    fn check_game(&mut self, ctx: &egui::Context) {
        let dir = PathBuf::from(self.game_path.trim());
        self.game = None;
        self.checking = Some(Job::start(ctx, move |p| setup::check_game(&dir, p)));
    }

    /// One frame of the window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.styled {
            style::install(&ctx, &self.root);
            self.logo = style::logo(&ctx);
            self.art = qwc_style::ornate::Art::load(&ctx, &self.root.join("crates/qwc-launcher/art"));
            self.styled = true;
        }
        if std::mem::take(&mut self.check_soon) {
            self.check_game(&ctx);
        }
        if std::mem::take(&mut self.news_soon) {
            self.check_releases(&ctx);
        }
        self.poll_news(&ctx);
        self.poll_account(&ctx);
        self.poll_collections(&ctx);
        self.poll_review(&ctx);
        self.poll();
        let installing = self.packs_view.installing.is_some()
            || self.news_view.busy()
            || self.online_view.busy()
            || self.account_view.busy()
            || self.packs_view.collections.busy()
            || self.review_view.busy();
        if self.checking.is_some() || self.importing.is_some() || self.running.is_some() || installing {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        chrome::backdrop(&ctx, &self.art);
        egui::Panel::top("bar")
            .exact_size(chrome::BAR)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new())
            .show(ui, |ui| self.top_bar(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(22, 18))).show(
            ui,
            |ui| {
                if self.page == Page::Home {
                    self.home(ui);
                    return;
                }
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.page {
                    Page::Setup => self.setup(ui),
                    Page::Home => {}
                    Page::Online => self.online(ui),
                    Page::Packs => self.packs(ui),
                    Page::Settings => self.settings(ui),
                    Page::Saves => self.saves(ui),
                    Page::News => self.news(ui),
                    Page::Profile => self.profile_page(ui),
                    Page::Review => self.review_page(ui),
                });
            },
        );
        self.submit_dialog(&ctx);
        self.report_dialog(&ctx);
        chrome::frame(&ctx, &self.art);
    }

    /// The jobs' answers and the game's end.
    fn poll(&mut self) {
        self.poll_packs();
        self.poll_store();
        self.poll_online();
        if let Some(r) = self.checking.as_ref().and_then(Job::take) {
            self.checking = None;
            self.game = Some(r);
        }
        if let Some(r) = self.importing.as_ref().and_then(Job::take) {
            self.importing = None;
            match r {
                Ok(path) => {
                    self.profile = qwc_packs::Profile::read(&path).unwrap_or_default();
                    self.page = Page::Home;
                    self.status = "The game files are imported.".into();
                }
                Err(e) => self.import_error = Some(e),
            }
        }
        if let Some(child) = &mut self.running {
            if let Ok(Some(exit)) = child.try_wait() {
                self.running = None;
                self.status = if exit.success() {
                    String::new()
                } else {
                    format!("The game stopped with an error ({exit}). Its log: {}", self.log_path().display())
                };
            }
        }
    }

    fn log_path(&self) -> PathBuf {
        self.library.as_ref().map_or_else(|| "game.log".into(), |l| l.join("logs/game.log"))
    }

    fn setup(&mut self, ui: &mut egui::Ui) {
        let art = self.art.clone();
        let ctx = ui.ctx().clone();
        ui.label(style::heading("Set up", 36.0));
        ui.label(
            "QWC Remastered plays from your own copy of the game. Point it at your QWC.exe and your game's files, \
             and it copies what it needs into its own library.",
        );
        ui.add_space(12.0);

        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            let title = ui.label(style::heading("1   Your QWC.exe", 23.0)).id;
            ui.label(
                RichText::new(
                    "An unprotected QWC.exe from your own copy of the game: the remaster reads two of its tables.",
                )
                .color(style::DIM),
            );
            let mut picked = None;
            ui.horizontal(|ui| {
                let edit = ui
                    .add(egui::TextEdit::singleline(&mut self.exe_path).desired_width(ui.available_width() - 110.0))
                    .labelled_by(title);
                if edit.lost_focus() && !self.exe_path.trim().is_empty() {
                    picked = Some(PathBuf::from(self.exe_path.trim()));
                }
                if ui.button("Browse...").clicked() {
                    picked = rfd::FileDialog::new().add_filter("QWC.exe", &["exe"]).pick_file();
                }
            });
            if let Some(p) = picked {
                self.set_exe(p);
            }
            if let Some(c) = &self.exe {
                let colour = match c.verdict {
                    ExeVerdict::Verified(_) => style::GOOD,
                    ExeVerdict::Unwrapped => style::WARN,
                    _ => style::BAD,
                };
                ui.label(RichText::new(c.verdict.message()).color(colour));
            }
        });
        ui.add_space(10.0);

        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            let title = ui.label(style::heading("2   Your game files", 23.0)).id;
            ui.label(
                RichText::new("The folder the game is installed in, or a copy of the disc's files.").color(style::DIM),
            );
            let mut picked = false;
            ui.horizontal(|ui| {
                let edit = ui
                    .add(egui::TextEdit::singleline(&mut self.game_path).desired_width(ui.available_width() - 110.0))
                    .labelled_by(title);
                picked |= edit.lost_focus() && !self.game_path.trim().is_empty();
                if ui.button("Browse...").clicked() {
                    if let Some(d) = rfd::FileDialog::new().pick_folder() {
                        self.game_path = d.display().to_string();
                        picked = true;
                    }
                }
            });
            if picked {
                self.check_game(&ctx);
            }
            if let Some(job) = &self.checking {
                job.bar(ui, "Checking");
            }
            match &self.game {
                Some(Ok(g)) => {
                    let colour = if !g.importable() {
                        style::BAD
                    } else if g.verified {
                        style::GOOD
                    } else {
                        style::WARN
                    };
                    ui.label(RichText::new(g.summary()).color(colour));
                    if g.importable()
                        && !g.verified
                        && (!g.changed.is_empty() || !g.missing.is_empty() || !g.extra.is_empty())
                    {
                        egui::CollapsingHeader::new("What differs").show(ui, |ui| {
                            for (what, list) in
                                [("Changed", &g.changed), ("Missing", &g.missing), ("Not in the edition", &g.extra)]
                            {
                                if list.is_empty() {
                                    continue;
                                }
                                ui.label(RichText::new(format!("{what} ({})", list.len())).strong());
                                for f in list.iter().take(30) {
                                    ui.label(RichText::new(f).small().color(style::DIM));
                                }
                                if list.len() > 30 {
                                    ui.label(
                                        RichText::new(format!("and {} more", list.len() - 30))
                                            .small()
                                            .color(style::DIM),
                                    );
                                }
                            }
                        });
                    }
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(style::BAD));
                }
                None => {}
            }
        });
        ui.add_space(10.0);

        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            ui.label(style::heading("3   Import", 23.0));
            let game = self.game.as_ref().and_then(|g| g.as_ref().ok()).filter(|g| g.importable());
            let exe = self.exe.as_ref().filter(|e| e.verdict.usable());
            match (&self.library, game) {
                (Some(lib), Some(g)) => ui.label(
                    RichText::new(format!(
                        "Copies the game's files ({:.0} MB) and the exe into {}. Your copy stays as it is.",
                        g.bytes as f64 / 1e6,
                        setup::game_dir(lib, g).display()
                    ))
                    .color(style::DIM),
                ),
                (None, _) => ui.label(RichText::new("No library folder in this environment.").color(style::BAD)),
                _ => ui.label(
                    RichText::new("Once both are checked, the game's files are copied into the remaster's library.")
                        .color(style::DIM),
                ),
            };
            if let Some(job) = &self.importing {
                job.bar(ui, "Importing");
                if ui.button("Stop").clicked() {
                    job.progress.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            } else {
                let can = self.library.is_some() && game.is_some() && exe.is_some() && self.checking.is_none();
                let go = ui.add_enabled_ui(can, |ui| ui.add_sized([180.0, 44.0], style::primary("Import", 22.0))).inner;
                if go.clicked() {
                    let (game, exe, lib) = (game.cloned(), exe.cloned(), self.library.clone());
                    if let (Some(game), Some(exe), Some(lib)) = (game, exe, lib) {
                        self.import_error = None;
                        self.importing = Some(Job::start(&ctx, move |p| setup::import(&game, &exe, &lib, p)));
                    }
                }
            }
            if let Some(e) = &self.import_error {
                ui.label(RichText::new(e).color(style::BAD));
            }
        });
        if self.ready() {
            ui.add_space(10.0);
            if ui.add(style::button_with(&art, "icon_left", "Back")).clicked() {
                self.page = Page::Home;
            }
        }
    }

    /// The page shown.
    pub fn page(&self) -> Page {
        self.page
    }

    /// Show `page` (the screenshots).
    pub fn open(&mut self, page: Page) {
        self.page = page;
    }

    /// Look for a new release when the window opens, if the player's settings say so (not in tests: no network).
    pub fn check_updates_on_start(&mut self) {
        self.news_soon = self.settings_view.settings.updates.check_on_start;
    }

    /// Show these releases as if fetched (the screenshots).
    pub fn show_releases(&mut self, releases: Vec<crate::updates::Release>) {
        self.news_view.releases = Some(Ok(releases));
    }

    /// Open the Packs page's Store view on these packs as if fetched (the screenshots).
    pub fn show_store(&mut self, packs: Vec<crate::store::Listed>) {
        self.page = Page::Packs;
        self.packs_view.store_open = true;
        self.packs_view.store.looked = true;
        self.packs_view.store.packs = packs;
    }

    /// Whether a check or an import is still working (or about to start).
    pub fn busy(&self) -> bool {
        self.checking.is_some() || self.importing.is_some() || self.check_soon
    }
}

impl eframe::App for Launcher {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        shape::apply(&mut self.shaped, frame, ui.ctx());
        Launcher::ui(self, ui);
    }
}

/// The launcher for the screenshots and tests: on `library`, with the setup's paths filled in (and checked on the
/// first frame).
pub fn for_shots(root: &Path, library: Option<PathBuf>, exe: Option<&Path>, game: Option<&Path>) -> Launcher {
    let mut l = Launcher::new(root, library);
    if let Some(e) = exe {
        l.set_exe(e.to_path_buf());
    }
    if let Some(g) = game {
        l.game_path = g.display().to_string();
        l.check_soon = true;
    }
    l
}
