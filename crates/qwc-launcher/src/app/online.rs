//! The Online page: a match against a friend over the network with no server in between (`qwc-net`,
//! `docs/ONLINE.md`): host (a team, a stadium, a port) or join (the host's address, a team). The game runs
//! `qwc-game online host|join`, its status lines (`@online ...`) show here, and the match opens in the game's window
//! once both machines have loaded it. What was last chosen is kept in the library's `online.ron`.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin};
use std::sync::{Arc, Mutex};

use std::time::Duration;

use eframe::egui::{self, Color32, RichText};
use serde::{Deserialize, Serialize};

use super::home::{stadium, team};
use super::{Job, Launcher};
use crate::style;

/// The teams' folders (`teams/`), the order the lists show them.
const TEAMS: [&str; 13] = [
    "England", "France", "Germany", "Bulgaria", "Japan", "spain", "USA", "Aus", "Nordic", "Gryff", "Huffle", "Raven",
    "Slyth",
];
/// The stadiums' folders (`stadium/`).
const STADIUMS: [&str; 12] = [
    "england", "france", "germany", "Bulgaria", "japan", "spain", "usa", "Aus", "nordic", "hog_sum", "hog_win",
    "qditch",
];
/// The port the host listens on unless changed (`qwc_net::DEFAULT_PORT`).
const DEFAULT_PORT: u16 = 20030;

/// What was last chosen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Choices {
    team: String,
    stadium: String,
    port: u16,
    address: String,
}

impl Default for Choices {
    fn default() -> Choices {
        Choices { team: "England".into(), stadium: "england".into(), port: DEFAULT_PORT, address: String::new() }
    }
}

/// The game playing online, the status lines it has printed so far, and its standard input (the host's `punch`
/// lines).
struct Running {
    child: Child,
    stdin: Option<ChildStdin>,
    hosting: bool,
    lines: Arc<Mutex<Vec<String>>>,
}

#[derive(Default)]
pub(super) struct OnlineView {
    read: bool,
    choices: Choices,
    running: Option<Running>,
    /// What the last game said at its end (or why it could not start): kept once it has gone.
    last: Vec<(Color32, String)>,
    /// The address typed to let a player in.
    let_in: String,
    /// The connection's check (`qwc_net::probe`), running or done.
    probe: Option<Job<qwc_net::Probe>>,
    probed: Option<Result<qwc_net::Probe, String>>,
    /// The Join panel's check of the host's address (`qwc_net::query`), running or done, and the address it was for.
    check: Option<Job<qwc_net::LobbyInfo>>,
    checked: Option<(String, Result<qwc_net::LobbyInfo, String>)>,
    checking: String,
}

impl Drop for OnlineView {
    /// The launcher closing stops a game still waiting to host or join (it has no window to close yet); a match
    /// being played goes on in its window.
    fn drop(&mut self) {
        if let Some(r) = &mut self.running {
            let lines = r.lines.lock().map(|l| l.clone()).unwrap_or_default();
            if !lines.iter().any(|l| l.starts_with("start")) {
                let _ = r.child.kill();
            }
        }
    }
}

/// The connection's bars for a round trip (the user's levels, 2026-10-24): three under 50 ms, two under 150 ms, one
/// slower; no answer at all, `no_wifi`. The art's icon and the colour it is drawn in.
fn signal(rtt: Option<Duration>) -> (&'static str, Color32) {
    match rtt {
        None => ("icon_no_wifi", style::BAD),
        Some(r) if r < Duration::from_millis(50) => ("icon_wifi3", style::GOOD),
        Some(r) if r < Duration::from_millis(150) => ("icon_wifi2", style::WARN),
        Some(_) => ("icon_wifi1", style::BAD),
    }
}

/// The connection's bars for `rtt` ([`signal`]), a line's height, its round trip on hover.
fn signal_icon(ui: &mut egui::Ui, art: &qwc_style::ornate::Art, rtt: Option<Duration>) {
    let (icon, colour) = signal(rtt);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
    qwc_style::ornate::icon(ui.painter(), art, icon, rect, colour);
    let hint = rtt.map_or_else(|| "No answer".to_string(), |r| format!("Round trip {} ms", r.as_millis()));
    response.on_hover_text(hint);
}

/// The round trip a game's status lines give once playing (`start DELAY RTT_MS`).
fn playing_rtt(lines: &[String]) -> Option<Duration> {
    let rest = lines.iter().find_map(|l| l.strip_prefix("start "))?;
    rest.split(' ').nth(1)?.parse().ok().map(Duration::from_millis)
}

/// A stadium as the check names it: the nations' grounds as "England Stadium", Hogwarts' pitches and the World Cup
/// final by their own names.
fn ground(folder: &str) -> String {
    let name = stadium(folder);
    if folder.to_ascii_lowercase().starts_with("hog") || folder.eq_ignore_ascii_case("qditch") {
        name
    } else {
        format!("{name} Stadium")
    }
}

/// What the check of a host's address found, for the Join panel: the colour and the line.
fn read_check(info: &Result<qwc_net::LobbyInfo, String>, my_team: &str) -> Vec<(Color32, String)> {
    let info = match info {
        Ok(i) => i,
        Err(e) => return vec![(style::BAD, format!("No host answered: {e}."))],
    };
    let version = info.build.split('/').next().unwrap_or("");
    if version != env!("CARGO_PKG_VERSION") {
        return vec![(
            style::BAD,
            format!(
                "A host is there, but it plays version {version} and you {}: update to play.",
                env!("CARGO_PKG_VERSION")
            ),
        )];
    }
    let text = String::from_utf8_lossy(&info.about);
    let field = |k: &str| text.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix(' ').map(str::to_string));
    let what = match (field("team"), field("stadium")) {
        (Some(t), Some(s)) => format!(": {} at {}", team(&t), ground(&s)),
        _ => String::new(),
    };
    let mut out = vec![(
        style::GOOD,
        format!("A host is waiting{what} (the round trip {} ms). You can join.", info.rtt.as_millis()),
    )];
    if field("team").is_some_and(|t| t.eq_ignore_ascii_case(my_team)) {
        out.push((style::DIM, "They play your team: you will get the other team they chose for you.".into()));
    }
    out
}

impl OnlineView {
    /// Whether a game is playing online or the connection is being checked (the window repaints meanwhile).
    pub(super) fn busy(&self) -> bool {
        self.running.is_some() || self.probe.is_some() || self.check.is_some()
    }
}

/// The addresses a game's status lines give to share, with their names: the host's on the internet and on the
/// network, and IPv6; the joiner's on the internet.
fn addresses(lines: &[String], hosting: bool) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for l in lines {
        let (word, rest) = l.split_once(' ').unwrap_or((l.as_str(), ""));
        let addr = rest.split(' ').next().unwrap_or("").to_string();
        let name = match word {
            "public" if hosting => "On the internet",
            "public" => "Yours, for the host to let in",
            "address" => "On your network",
            "address6" => "IPv6",
            _ => continue,
        };
        if !addr.is_empty() {
            out.push((name, addr));
        }
    }
    out
}

/// What the connection's check found, line by line.
fn read_probe(p: &qwc_net::Probe) -> Vec<(Color32, String)> {
    let mut out = Vec::new();
    match p.lan {
        Some(ip) => out.push((style::TEXT, format!("Your address on your network: {ip}."))),
        None => out.push((style::BAD, "No network connection was found.".into())),
    }
    match p.stun.public {
        Some(a) => out.push((style::TEXT, format!("Your address on the internet: {}.", a.ip()))),
        None => out.push((style::WARN, "The public STUN servers did not answer: is the internet reachable?".into())),
    }
    if p.stun.symmetric {
        out.push((
            style::WARN,
            "Your router gives each address it talks to another port (a symmetric NAT): letting a player in does not              get through it. Port forwarding, UPnP or a virtual network do."
                .into(),
        ));
    } else if p.stun.public.is_some() {
        out.push((style::GOOD, "Your router keeps one port for every address: letting a player in works.".into()));
    }
    match &p.upnp {
        Ok(ip) => out.push((
            style::GOOD,
            format!(
                "Your router takes UPnP: hosting asks it to forward the port by itself{}.",
                ip.map_or_else(String::new, |ip| format!(" (its address {ip})"))
            ),
        )),
        Err(e) => out.push((style::DIM, format!("UPnP: {e}. Letting in or port forwarding remain."))),
    }
    if let Some(ip) = p.ipv6 {
        out.push((
            style::TEXT,
            format!(
                "You have IPv6 ({ip}): a joiner with IPv6 can reach you without a router's help, firewalls allowing."
            ),
        ));
    }
    out
}

/// What a game's status lines say, in order, for the page: each line its colour and text.
fn read_status(lines: &[String], hosting: bool) -> Vec<(Color32, String)> {
    let mut out = Vec::new();
    for l in lines {
        let (word, rest) = l.split_once(' ').unwrap_or((l.as_str(), ""));
        let said = match word {
            "hosting" => (style::TEXT, format!("Waiting for a player to join, on UDP port {rest}.")),
            "address" => (
                style::GOOD,
                format!("Your address on your network: {rest} (for a player on the same network, or a virtual one)."),
            ),
            "address6" => {
                (style::TEXT, format!("Your IPv6 address: {rest} (for a joiner with IPv6, firewalls allowing)."))
            }
            "upnp" if rest.is_empty() => (style::GOOD, "Your router forwards the game's port (UPnP).".to_string()),
            "upnp" => (
                style::GOOD,
                format!("Your router forwards the game's port (UPnP): friends on the internet join {rest}."),
            ),
            "upnp-failed" => (
                style::DIM,
                format!("Your router did not forward the port ({rest}): let your friend in with their address, below."),
            ),
            "public" => {
                let (addr, symmetric) = (rest.split(' ').next().unwrap_or(rest), rest.ends_with(" symmetric"));
                let said = if hosting {
                    (style::GOOD, format!("Your address on the internet: {addr}."))
                } else {
                    (
                        style::TEXT,
                        format!(
                            "Your address on the internet: {addr}. If the host cannot be reached, send it to them to let \
                             you in."
                        ),
                    )
                };
                if symmetric {
                    out.push(said);
                    (
                        style::WARN,
                        "Your router changes the port for each address (a symmetric NAT): letting in will not get \
                         through it; port forwarding or a virtual network will."
                            .to_string(),
                    )
                } else {
                    said
                }
            }
            "letting-in" => (style::TEXT, format!("Letting in {rest}: they can join now.")),
            "letting-in-failed" => (style::BAD, format!("Could not let in that address: {rest}.")),
            "other-build" => {
                let (from, build) = rest.split_once(' ').unwrap_or((rest, "?"));
                (style::WARN, format!("{from} tried to join with another version of the remaster ({build})."))
            }
            "joined" => (style::GOOD, format!("A player joined from {rest}: both games are loading the match.")),
            "joining" => (style::TEXT, format!("Looking for the host at {rest}...")),
            "connected" => (style::GOOD, format!("Found the host at {rest}: both games are loading the match.")),
            "start" => {
                let mut w = rest.split(' ');
                let (delay, rtt) = (w.next().unwrap_or("?"), w.next().unwrap_or("?"));
                (style::GOOD, format!("Playing: the round trip is {rtt} ms, the input {delay} frame(s) late."))
            }
            "end" => {
                let mut w = rest.split(' ');
                let (a, b) = (w.next().unwrap_or("?"), w.next().unwrap_or("?"));
                (style::GOLD_LIGHT, format!("Full time: {a} - {b}."))
            }
            "over" => (style::WARN, format!("The match ended: {rest}.")),
            "error" => (style::BAD, rest.to_string()),
            _ => continue,
        };
        out.push(said);
    }
    if out.is_empty() {
        out.push((style::DIM, if hosting { "Starting to host..." } else { "Starting..." }.to_string()));
    }
    out
}

impl Launcher {
    fn online_choices_path(&self) -> Option<PathBuf> {
        self.library.as_ref().map(|l| l.join("online.ron"))
    }

    fn online_log(&self) -> PathBuf {
        self.library.as_ref().map_or_else(|| "online.log".into(), |l| l.join("logs/online.log"))
    }

    fn save_online_choices(&self) {
        if let (Some(p), Ok(text)) = (self.online_choices_path(), ron::to_string(&self.online_view.choices)) {
            let _ = std::fs::write(p, text);
        }
    }

    /// Starts `qwc-game online ARGS`, its status lines read on a thread.
    fn start_online(&mut self, ctx: &egui::Context, hosting: bool, args: Vec<String>) {
        self.save_online_choices();
        let started = self
            .profile_path()
            .ok_or_else(|| "no library folder".to_string())
            .and_then(|p| crate::launch::online(&p, &self.online_log(), &args));
        let mut child = match started {
            Ok(c) => c,
            Err(e) => {
                self.online_view.last = vec![(style::BAD, e)];
                return;
            }
        };
        let lines = Arc::new(Mutex::new(Vec::new()));
        if let Some(out) = child.stdout.take() {
            let (lines, ctx) = (lines.clone(), ctx.clone());
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                    if let Some(status) = line.strip_prefix("@online ") {
                        if let Ok(mut l) = lines.lock() {
                            l.push(status.to_string());
                        }
                        ctx.request_repaint();
                    }
                }
            });
        }
        self.online_view.last.clear();
        let stdin = child.stdin.take();
        self.online_view.running = Some(Running { child, stdin, hosting, lines });
    }

    /// Show the Join panel's check of `address` as if it had answered so (the screenshots).
    pub fn show_check(&mut self, address: &str, answer: Result<qwc_net::LobbyInfo, String>) {
        self.online_view.read = true;
        self.online_view.choices.address = address.into();
        self.online_view.checked = Some((address.into(), answer));
    }

    /// The online game's end: what it said is kept.
    pub(super) fn poll_online(&mut self) {
        if let Some(r) = self.online_view.probe.as_ref().and_then(Job::take) {
            self.online_view.probe = None;
            self.online_view.probed = Some(r);
        }
        if let Some(r) = self.online_view.check.as_ref().and_then(Job::take) {
            self.online_view.check = None;
            self.online_view.checked = Some((self.online_view.checking.clone(), r));
        }
        let Some(r) = &mut self.online_view.running else { return };
        let Ok(Some(exit)) = r.child.try_wait() else { return };
        let lines = r.lines.lock().map(|l| l.clone()).unwrap_or_default();
        let mut said = read_status(&lines, r.hosting);
        if !exit.success() && !lines.iter().any(|l| l.starts_with("error")) {
            said.push((style::BAD, format!("The game stopped ({exit}). Its log: {}", self.online_log().display())));
        }
        said.push((style::DIM, "The game has closed.".into()));
        self.online_view.last = said;
        self.online_view.running = None;
    }

    pub(super) fn online(&mut self, ui: &mut egui::Ui) {
        let art = self.art.clone();
        let ctx = ui.ctx().clone();
        if !self.online_view.read {
            self.online_view.read = true;
            self.online_view.choices = self
                .online_choices_path()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|t| ron::from_str(&t).ok())
                .unwrap_or_default();
        }
        ui.label(style::heading("Online", 36.0));
        ui.label(
            RichText::new(
                "A match against a friend over the internet or your home network, with no server in between: one of \
                 you hosts, the other joins. You both need the same version of the remaster and the same game files \
                 (and the same Gameplay packs on).",
            )
            .color(style::DIM),
        );
        ui.add_space(8.0);
        let idle = self.online_view.running.is_none();
        let mut start = None;
        let mut check = None;
        let checking = self.online_view.check.is_some();
        // the last check's answer, while the address is still the one checked
        let checked = self.online_view.checked.as_ref().filter(|(a, _)| *a == self.online_view.choices.address.trim());
        let check_said = checked.map(|(_, r)| read_check(r, &self.online_view.choices.team));
        let check_rtt = checked.map(|(_, r)| r.as_ref().ok().map(|i| i.rtt));
        let c = &mut self.online_view.choices;

        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            ui.label(style::heading("Host a match", 23.0));
            ui.label(
                RichText::new("You play the first team and choose the stadium; your friend joins your address.")
                    .color(style::DIM),
            );
            ui.horizontal(|ui| {
                team_list(ui, &art, "host team", "Your team", &mut c.team);
                ui.add_space(12.0);
                let l = ui.label("Stadium");
                style::dropdown(&art, egui::ComboBox::from_id_salt("stadium"))
                    .selected_text(stadium(&c.stadium))
                    .show_ui(ui, |ui| {
                        for s in STADIUMS {
                            ui.selectable_value(&mut c.stadium, s.to_string(), stadium(s));
                        }
                    })
                    .response
                    .labelled_by(l.id);
                ui.add_space(12.0);
                ui.label("Port");
                ui.add(egui::DragValue::new(&mut c.port).range(1024..=65535))
                    .on_hover_text("The UDP port the game listens on (20030 unless you change it)");
            });
            ui.add_space(4.0);
            let host = ui.add_enabled(idle, style::primary("Host", 20.0));
            if host.clicked() {
                let args = vec![
                    "host".into(),
                    c.team.clone(),
                    "--stadium".into(),
                    c.stadium.clone(),
                    "--port".into(),
                    c.port.to_string(),
                ];
                start = Some((true, args));
            }
        });
        ui.add_space(10.0);

        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            ui.label(style::heading("Join a match", 23.0));
            ui.label(
                RichText::new("The host's address, as their Online page shows it (add :PORT if they changed it).")
                    .color(style::DIM),
            );
            ui.horizontal(|ui| {
                let label = ui.label("The host's address");
                ui.add(
                    egui::TextEdit::singleline(&mut c.address)
                        .hint_text("192.168.1.20 or 81.2.69.160:20030")
                        .desired_width(320.0),
                )
                .labelled_by(label.id);
                ui.add_space(12.0);
                team_list(ui, &art, "join team", "Your team", &mut c.team);
            });
            ui.label(
                RichText::new("If the host plays the same team, you get the other team they chose for you.")
                    .small()
                    .color(style::DIM),
            );
            ui.add_space(4.0);
            let address = c.address.trim().to_string();
            ui.horizontal(|ui| {
                let join = ui.add_enabled(idle && !address.is_empty(), style::primary("Join", 20.0));
                if join.clicked() {
                    start = Some((false, vec!["join".into(), address.clone(), c.team.clone()]));
                }
                ui.add_space(8.0);
                let hint = "Ask the address whether a host is waiting there, without joining";
                let label = if checking { "Checking..." } else { "Check" };
                if ui
                    .add_enabled(!checking && !address.is_empty(), egui::Button::new(label))
                    .on_hover_text(hint)
                    .clicked()
                {
                    check = Some(address.clone());
                }
            });
            // the first line beside the connection's bars
            for (k, (colour, text)) in check_said.into_iter().flatten().enumerate() {
                ui.horizontal(|ui| {
                    if k == 0 {
                        signal_icon(ui, &art, check_rtt.flatten());
                    }
                    ui.label(RichText::new(text).color(colour));
                });
            }
        });
        if let Some(address) = check {
            let v = &mut self.online_view;
            v.checking = address.clone();
            v.checked = None;
            v.check = Some(Job::start(&ctx, move |_| {
                qwc_net::query(&address, DEFAULT_PORT, std::time::Duration::from_secs(3)).map_err(|e| e.to_string())
            }));
        }
        ui.add_space(10.0);

        if let Some((hosting, args)) = start {
            self.start_online(&ctx, hosting, args);
        }
        let (mut stop, mut let_in) = (false, None);
        let lines = self.online_view.running.as_ref().map(|r| r.lines.lock().map(|l| l.clone()).unwrap_or_default());
        let shown = match (&self.online_view.running, &lines) {
            (Some(r), Some(lines)) => {
                Some((if r.hosting { "Hosting" } else { "Joining" }, read_status(lines, r.hosting), Some(r.hosting)))
            }
            _ if !self.online_view.last.is_empty() => Some(("The last game", self.online_view.last.clone(), None)),
            _ => None,
        };
        if let Some((title, said, running)) = shown {
            let v = &mut self.online_view;
            style::tile(ui, &art, |ui| {
                ui.set_width(ui.available_width());
                ui.label(style::heading(title, 23.0));
                let lines = lines.unwrap_or_default();
                for (colour, text) in said {
                    ui.horizontal(|ui| {
                        // playing: the connection's bars beside the round trip's line
                        if running.is_some() && text.starts_with("Playing:") {
                            signal_icon(ui, &art, playing_rtt(&lines));
                        }
                        ui.label(RichText::new(text).color(colour));
                    });
                }
                let Some(hosting) = running else { return };
                let playing = lines.iter().any(|l| l.starts_with("joined") || l.starts_with("connected"));
                // the addresses to give, each with its copy button
                let shared = addresses(&lines, hosting);
                if !shared.is_empty() && !playing {
                    ui.add_space(4.0);
                    for (name, addr) in shared {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{name}:")).color(style::DIM));
                            ui.label(RichText::new(&addr).strong().color(style::GOLD_LIGHT));
                            if ui.small_button("Copy").clicked() {
                                ui.ctx().copy_text(addr.clone());
                            }
                        });
                    }
                }
                if hosting && !playing {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let l = ui.label("Let a player in");
                        ui.add(
                            egui::TextEdit::singleline(&mut v.let_in)
                                .hint_text("the address their page shows")
                                .desired_width(240.0),
                        )
                        .labelled_by(l.id);
                        if ui.add_enabled(!v.let_in.trim().is_empty(), egui::Button::new("Let in")).clicked() {
                            let_in = Some(v.let_in.trim().to_string());
                        }
                    });
                }
                ui.add_space(4.0);
                stop = ui.button("Stop").on_hover_text("Close the game").clicked();
            });
            ui.add_space(10.0);
        }
        if let (Some(addr), Some(r)) = (let_in, &mut self.online_view.running) {
            if let Some(stdin) = &mut r.stdin {
                let _ = writeln!(stdin, "punch {addr}").and_then(|_| stdin.flush());
            }
        }
        if stop {
            if let Some(r) = &mut self.online_view.running {
                // the game says goodbye when it closes its window; stopped from here it cannot: the other side
                // notices the silence
                let _ = r.child.kill();
            }
        }

        style::tile(ui, &art, |ui| {
            ui.set_width(ui.available_width());
            ui.label(style::heading("Connecting", 23.0));
            for (head, text) in [
                ("On the same network", "Join the address the host's page shows under \"On your network\"."),
                (
                    "Over the internet",
                    "Hosting asks your router to forward the game's port by itself (UPnP) and shows your address on \
                     the internet: your friend joins that. If the router will not, your friend joins anyway and sends \
                     you the address their page then shows; enter it under \"Let a player in\". Port forwarding \
                     (UDP 20030 to your computer, in the router's settings) always works.",
                ),
                (
                    "Without touching the router",
                    "Both install a virtual network such as Tailscale, ZeroTier or Radmin VPN and join it; the joiner \
                     then uses the host's address in that network.",
                ),
                (
                    "The firewall",
                    "The first time you host, your system may ask whether the game may accept connections: allow it.",
                ),
            ] {
                ui.label(RichText::new(head).strong().color(style::GOLD_LIGHT));
                ui.label(RichText::new(text).color(style::DIM));
                ui.add_space(4.0);
            }
            ui.add_space(4.0);
            let v = &mut self.online_view;
            if v.probe.is_some() {
                ui.label(RichText::new("Checking your connection...").color(style::DIM));
            } else if ui
                .button("Check my connection")
                .on_hover_text("Asks the public STUN servers and your router, changing nothing")
                .clicked()
            {
                v.probed = None;
                v.probe = Some(Job::start(&ctx, |_| Ok(qwc_net::probe())));
            }
            match &v.probed {
                Some(Ok(p)) => {
                    for (colour, text) in read_probe(p) {
                        ui.label(RichText::new(text).color(colour));
                    }
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(style::BAD));
                }
                None => {}
            }
        });
    }
}

/// A list of the teams, `label` beside it, `salt` its id.
fn team_list(ui: &mut egui::Ui, art: &qwc_style::ornate::Art, salt: &str, label: &str, value: &mut String) {
    let l = ui.label(label);
    style::dropdown(art, egui::ComboBox::from_id_salt(salt))
        .selected_text(team(value))
        .show_ui(ui, |ui| {
            for t in TEAMS {
                ui.selectable_value(value, t.to_string(), team(t));
            }
        })
        .response
        .labelled_by(l.id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lines_read_as_the_page_says_them() {
        let lines: Vec<String> =
            ["hosting 20030", "address 192.168.1.5:20030", "joined 81.2.69.160:51000", "start 2 64", "end 3 1"]
                .map(String::from)
                .to_vec();
        let said: Vec<String> = read_status(&lines, true).into_iter().map(|(_, t)| t).collect();
        assert_eq!(said.len(), 5);
        assert!(said[1].contains("192.168.1.5:20030"));
        assert!(said[3].contains("64 ms") && said[3].contains("2 frame"));
        assert_eq!(said[4], "Full time: 3 - 1.");
        assert_eq!(read_status(&["noise".into()], false), vec![(style::DIM, "Starting...".to_string())]);
    }

    #[test]
    fn the_addresses_to_share() {
        let host: Vec<String> =
            ["hosting 20030", "address 192.168.1.5:20030", "upnp-failed no router", "public 81.2.69.160:20030"]
                .map(String::from)
                .to_vec();
        assert_eq!(
            addresses(&host, true),
            [
                ("On your network", "192.168.1.5:20030".to_string()),
                ("On the internet", "81.2.69.160:20030".to_string())
            ]
        );
        let joiner = vec!["public 203.0.113.9:51000 symmetric".to_string()];
        assert_eq!(addresses(&joiner, false), [("Yours, for the host to let in", "203.0.113.9:51000".to_string())]);
        let said = read_status(&joiner, false);
        assert_eq!(said.len(), 2, "the address, then the warning");
        assert_eq!(said[1].0, style::WARN);
    }

    #[test]
    fn a_check_says_what_the_host_offers() {
        let info = |build: &str| {
            Ok(qwc_net::LobbyInfo {
                host: "127.0.0.1:20030".parse().unwrap(),
                build: build.into(),
                about: b"team England\nstadium hog_win".to_vec(),
                rtt: std::time::Duration::from_millis(3),
            })
        };
        let ours = format!("{}/1", env!("CARGO_PKG_VERSION"));
        let said = read_check(&info(&ours), "England");
        assert_eq!(
            said[0],
            (style::GOOD, "A host is waiting: England at Hogwarts, winter (the round trip 3 ms). You can join.".into())
        );
        assert_eq!(said.len(), 2, "the same team: the other one is given");
        assert_eq!(read_check(&info(&ours), "France").len(), 1);
        assert_eq!(
            (ground("england"), ground("Aus"), ground("qditch")),
            ("England Stadium".into(), "Australia Stadium".into(), "World Cup final".into())
        );
        assert_eq!(read_check(&info("0.0.1/1"), "France")[0].0, style::BAD);
        assert_eq!(read_check(&Err("no answer".into()), "France")[0].0, style::BAD);
    }

    #[test]
    fn the_bars_by_round_trip() {
        let ms = |n| Some(Duration::from_millis(n));
        assert_eq!(signal(ms(0)).0, "icon_wifi3");
        assert_eq!(signal(ms(49)).0, "icon_wifi3");
        assert_eq!(signal(ms(50)).0, "icon_wifi2");
        assert_eq!(signal(ms(149)).0, "icon_wifi2");
        assert_eq!(signal(ms(150)).0, "icon_wifi1");
        assert_eq!(signal(None).0, "icon_no_wifi");
        let lines = vec!["joined 1.2.3.4:5".to_string(), "start 2 64".to_string()];
        assert_eq!(playing_rtt(&lines), ms(64));
        assert_eq!(playing_rtt(&lines[..1]), None);
    }

    #[test]
    fn the_lists_name_every_folder() {
        assert!(TEAMS.iter().all(|t| !team(t).is_empty()));
        assert_eq!(team("Huffle"), "Hufflepuff");
        assert_eq!(stadium("qditch"), "World Cup final");
    }

    #[test]
    fn choices_read_back_and_default() {
        let c = Choices { team: "Japan".into(), stadium: "japan".into(), port: 21000, address: "1.2.3.4".into() };
        assert_eq!(ron::from_str::<Choices>(&ron::to_string(&c).unwrap()).unwrap(), c);
        assert_eq!(ron::from_str::<Choices>("(team: \"USA\")").unwrap().port, DEFAULT_PORT);
    }
}
