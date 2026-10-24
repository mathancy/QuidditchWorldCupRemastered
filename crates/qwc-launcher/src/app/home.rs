//! The Play page: the banner (the art's picture with the title over it), the card to play from, the last match
//! played and the latest release on parchment, and a strip with the save and the installation.

use std::time::{Duration, Instant};

use eframe::egui::{self, pos2, vec2, Color32, FontId, Rect, Stroke};
use qwc_style::ornate::{self, Kind};

use super::{Launcher, Page};
use crate::{saves, style, updates};

/// What the page shows from files, read again every few seconds: the save and the newest match.
#[derive(Default)]
pub(super) struct HomeView {
    read: Option<Instant>,
    save: Option<saves::Save>,
    last: Option<saves::Played>,
    /// A match shown instead of the log's newest (for the screenshots).
    shown: Option<saves::Played>,
}

/// A team's name from its folder (the houses' and Australia's folders are short).
pub(super) fn team(folder: &str) -> String {
    match folder.to_ascii_lowercase().as_str() {
        "gryff" => "Gryffindor".into(),
        "slyth" => "Slytherin".into(),
        "huff" | "huffle" => "Hufflepuff".into(),
        "raven" | "ravenc" => "Ravenclaw".into(),
        "aus" => "Australia".into(),
        "usa" => "USA".into(),
        other => {
            let mut c = other.chars();
            c.next().map_or(String::new(), |f| f.to_uppercase().collect::<String>() + c.as_str())
        }
    }
}

/// A team's badge in the art (`flag_<name>.png`), from its folder.
pub(super) fn badge(folder: &str) -> String {
    format!("flag_{}", team(folder).to_ascii_lowercase())
}

/// When a match was played, from the log's `YYYY-MM-DD HH-MM-SS` (UTC): `23 Oct 2026, 21:04 UTC`; as it is when it
/// is not that.
fn played_at(stamp: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let parts: Vec<&str> = stamp.split([' ', '-']).collect();
    let [y, mo, d, h, mi, ..] = parts.as_slice() else { return stamp.to_string() };
    let (Ok(mo), Ok(d)) = (mo.parse::<usize>(), d.parse::<u32>()) else { return stamp.to_string() };
    match MONTHS.get(mo.wrapping_sub(1)) {
        Some(month) => format!("{d} {month} {y}, {h}:{mi} UTC"),
        None => stamp.to_string(),
    }
}

/// The last match as a scoreboard in `rect`: what it was and where along the top, then each side's badge with its
/// name under it and its score beside it, facing the other's across the middle; the winner's badge in a gold glow and
/// its score in full ink, the loser's paler.
fn scoreboard(ui: &mut egui::Ui, art: &ornate::Art, rect: Rect, m: &saves::Played) {
    let p = ui.painter().clone();
    let names = m.teams.clone().map(|t| team(&t));
    // for screen readers, the whole board as a line
    let summary =
        format!("{}: {} {}, {} {}, {}", kind(&m.kind), names[0], m.score[0], names[1], m.score[1], stadium(&m.stadium));
    ui.allocate_rect(rect, egui::Sense::hover())
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &summary));

    // the board grows with the card past the size it was laid out for (k), its small print by half as much (t)
    let k = (rect.width() / 320.0).min(rect.height() / 210.0).clamp(1.0, 3.0);
    let t = 1.0 + (k - 1.0) * 0.5;

    // what and where, over a pencil line
    let small = |text: &str, size: f32, colour: Color32| {
        ornate::spaced(&p, &text.to_uppercase(), FontId::new(size * t, style::title_family()), colour, 1.2 * t)
    };
    let mut head = small(&format!("{} \u{b7} {}", kind(&m.kind), stadium(&m.stadium)), 11.5, style::INK_DIM);
    if head.size().x > rect.width() {
        head = small(&kind(&m.kind), 11.5, style::INK_DIM);
    }
    p.galley(pos2(rect.center().x - head.size().x / 2.0, rect.top()), head.clone(), style::INK_DIM);
    let rule = rect.top() + head.size().y + 5.0 * t;
    ornate::faded_line(&p, rect.left() + 24.0, rect.right() - 24.0, rule, 1.0, PENCIL);

    // the date along the foot, when the badges keep room enough with it
    let name_h = 16.0 * k;
    let room_to = |foot: f32| foot - (rule + 10.0 * t) - name_h - 4.0 * k;
    let date = small(&played_at(&m.when), 10.5, style::INK_DIM);
    let date_top = rect.bottom() - date.size().y;
    let room = if room_to(date_top - 8.0 * t) >= 48.0 {
        p.galley(pos2(rect.center().x - date.size().x / 2.0, date_top), date, style::INK_DIM);
        room_to(date_top - 8.0 * t)
    } else {
        room_to(rect.bottom())
    };

    // each side's column: the badge as large as the room allows (and the column), its name under it
    let col = (rect.width() * 0.33).min(118.0 * k);
    let size = room.min(col * 0.8).clamp(34.0, 92.0 * k);
    let top = rule + 10.0 * t + ((room - size) / 2.0).max(0.0);
    let middle = top + size / 2.0;
    let won = [m.score[0] > m.score[1], m.score[1] > m.score[0]];
    for side in 0..2 {
        let cx = if side == 0 { rect.left() + col / 2.0 } else { rect.right() - col / 2.0 };
        let shield = Rect::from_center_size(pos2(cx, middle), vec2(size, size));
        if won[side] {
            // a warm glow behind the winner
            for k in 0..12 {
                let t = k as f32 / 11.0;
                let r = size * (0.78 - 0.3 * t);
                p.circle_filled(shield.center(), r, Color32::from_rgba_unmultiplied(0xe8, 0xb4, 0x4a, 14));
            }
        }
        if !ornate::picture(&p, art, &badge(&m.teams[side]), shield, 0.0, Color32::WHITE) {
            stand_in_badge(&p, shield, &names[side]);
        }
        let mut size_name = 13.0 * k;
        let mut name = ornate::spaced(
            &p,
            &names[side].to_uppercase(),
            FontId::new(size_name, style::heading_family()),
            style::INK,
            1.0 * k,
        );
        while name.size().x > col && size_name > 9.0 {
            size_name -= 0.5;
            name = ornate::spaced(
                &p,
                &names[side].to_uppercase(),
                FontId::new(size_name, style::heading_family()),
                style::INK,
                1.0 * k,
            );
        }
        p.galley(pos2(cx - name.size().x / 2.0, shield.bottom() + 4.0 * k), name, style::INK);
    }

    // the scores, facing each other across the middle
    let gap = 11.0 * k;
    // each score in its half of the middle, clear of the badges' columns
    let half = (rect.width() / 2.0 - col - gap - 4.0 * k).max(0.0);
    let mut font = (size * 0.55).clamp(22.0, 36.0 * k);
    let score = |n: i64, font: f32, colour: Color32| {
        p.layout_no_wrap(n.to_string(), FontId::new(font, style::heading_family()), colour)
    };
    let colour = |side: usize| if won[1 - side] { style::INK_DIM } else { style::INK };
    let mut sides = [score(m.score[0], font, colour(0)), score(m.score[1], font, colour(1))];
    while sides[0].size().x.max(sides[1].size().x) > half && font > 14.0 {
        font -= 1.0;
        sides = [score(m.score[0], font, colour(0)), score(m.score[1], font, colour(1))];
    }
    let mid_x = rect.center().x;
    let y = middle - sides[0].size().y / 2.0;
    p.galley(pos2(mid_x - gap - sides[0].size().x, y), sides[0].clone(), colour(0));
    p.galley(pos2(mid_x + gap, y), sides[1].clone(), colour(1));
    let dash = 4.0 * k;
    p.line_segment([pos2(mid_x - dash, middle), pos2(mid_x + dash, middle)], Stroke::new(2.0 * k, style::INK_DIM));
    let full = small("Full time", 9.5, style::INK_DIM);
    p.galley(pos2(mid_x - full.size().x / 2.0, y + sides[0].size().y + 2.0 * k), full, style::INK_DIM);
}

/// A plain shield with a team's initial, for a badge the art does not have.
fn stand_in_badge(p: &egui::Painter, rect: Rect, name: &str) {
    let r = rect.shrink(rect.width() * 0.1);
    let (l, t, rt, b, c) = (r.left(), r.top(), r.right(), r.bottom(), r.center().x);
    let mut pts = vec![pos2(l, t), pos2(rt, t), pos2(rt, t + r.height() * 0.45)];
    for k in 1..8 {
        let a = k as f32 / 8.0;
        pts.push(pos2(rt + (c - rt) * a, t + r.height() * (0.45 + 0.55 * a * a.sqrt())));
    }
    pts.push(pos2(c, b));
    for k in (1..8).rev() {
        let a = k as f32 / 8.0;
        pts.push(pos2(l + (c - l) * a, t + r.height() * (0.45 + 0.55 * a * a.sqrt())));
    }
    pts.push(pos2(l, t + r.height() * 0.45));
    p.add(egui::Shape::convex_polygon(pts, Color32::from_rgb(0x8a, 0x6a, 0x48), Stroke::new(1.5, style::INK)));
    let initial = name.chars().next().map(String::from).unwrap_or_default();
    p.text(
        pos2(c, t + r.height() * 0.42),
        egui::Align2::CENTER_CENTER,
        initial,
        FontId::new(r.height() * 0.45, style::heading_family()),
        style::PARCHMENT,
    );
}

/// A stadium's name from its folder.
pub(super) fn stadium(folder: &str) -> String {
    match folder.to_ascii_lowercase().as_str() {
        "hog_sum" => "Hogwarts, summer".into(),
        "hog_win" => "Hogwarts, winter".into(),
        "sum_cha" | "chal_01" | "chal_02" => "Hogwarts practice pitch".into(),
        "qditch" => "World Cup final".into(),
        // a nation's ground: its name as its team's (`Aus` is Australia, `usa` USA)
        other => team(other),
    }
}

/// A match's kind as the log keeps it, named.
fn kind(k: &str) -> String {
    match k.to_ascii_lowercase().as_str() {
        "quidd" | "" => "Quidditch match".into(),
        other if other.starts_with("chal") => "House challenge".into(),
        _ => k.to_string(),
    }
}

/// The note lines of a release's text: its list items, without their marks.
fn bullets(body: &str, n: usize) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .filter_map(|l| l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")))
        .map(|l| l.replace("**", ""))
        .take(n)
        .collect()
}

/// How far down a parchment card its title sits.
const TITLE: f32 = 16.0;

/// The pencil of the parchment's sketch, for the lines drawn on the paper.
const PENCIL: Color32 = Color32::from_rgb(0x72, 0x60, 0x4f);

/// How a parchment card looks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Paper {
    /// The plain paper under a dark band with the title (Last played).
    Band,
    /// The sketched paper, its sketch whole at the right, the title on the paper over a pencil line (Latest update).
    Sketch,
}

/// A parchment card with `title`, in the art's papers or their colours; answers the room for its contents.
fn parchment(ui: &mut egui::Ui, art: &ornate::Art, rect: Rect, title: &str, paper: Paper) -> Rect {
    let p = ui.painter();
    let inside = ornate::Inside::frame(rect, art, 32.0, ornate::Corner::Simple);
    // the plain paper under all of it
    if !inside.picture(p, art, "parchment_nosketch", rect, Color32::WHITE) {
        inside.fill_gradient(p, rect, style::PARCHMENT, Color32::from_rgb(0xd6, 0xc4, 0x9c), true);
    }
    let font = FontId::new(14.0, style::heading_family());
    let body = match paper {
        Paper::Band => {
            // the title where the sketched card has its own, and the band's foot level with that card's line
            let t = ornate::spaced(p, &title.to_uppercase(), font, style::GOLD_LIGHT, 1.5);
            let (top, foot) = (rect.top() + TITLE, rect.top() + TITLE + t.size().y + 7.0);
            let band = Rect::from_min_max(rect.min, pos2(rect.right(), foot));
            let dark = (Color32::from_rgb(0x33, 0x27, 0x1a), Color32::from_rgb(0x24, 0x1b, 0x12));
            inside.fill_gradient(p, band, dark.0, dark.1, true);
            p.line_segment([band.left_bottom(), band.right_bottom()], Stroke::new(1.5, style::GOLD_DEEP));
            p.galley(pos2(rect.left() + 18.0, top), t, style::GOLD_LIGHT);
            Rect::from_min_max(pos2(rect.left(), band.bottom()), rect.max)
        }
        Paper::Sketch => {
            // the sketch whole, at the right; its left edge fading into the plain paper
            if let Some(t) = art.get("parchment") {
                inside.right_fitted(p, t, rect, 0.2);
            }
            let t = ornate::spaced(p, &title.to_uppercase(), font, style::INK, 1.5);
            let top = rect.top() + TITLE;
            let line = top + t.size().y + 7.0;
            p.galley(pos2(rect.left() + 18.0, top), t, style::INK);
            ornate::faded_line(p, rect.left() + 10.0, rect.right() - 10.0, line, 1.2, PENCIL);
            Rect::from_min_max(pos2(rect.left(), line), rect.max)
        }
    };
    ornate::outline_with(p, rect, 4.0, art, 32.0, ornate::Corner::Simple);
    body.shrink2(vec2(18.0, 12.0))
}

impl Launcher {
    fn refresh_home(&mut self) {
        if self.home_view.read.is_some_and(|t| t.elapsed() < Duration::from_secs(3)) {
            return;
        }
        let v = &mut self.home_view;
        v.read = Some(Instant::now());
        v.save = saves::save_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| saves::Save::parse(&b).ok());
        v.last = v.shown.clone().or_else(|| {
            self.library.as_ref().and_then(|l| saves::recent_matches(&l.join("matches.ron"), 1).into_iter().next())
        });
    }

    /// Show `m` as the last match played, whatever the log says (for the screenshots).
    pub fn show_match(&mut self, m: saves::Played) {
        self.home_view.shown = Some(m.clone());
        self.home_view.last = Some(m);
    }

    /// Start the game (its save backed up first).
    fn play(&mut self) {
        self.back_up_before_playing();
        match self
            .profile_path()
            .ok_or_else(|| "no library folder".to_string())
            .and_then(|p| crate::launch::play(&p, &self.log_path()))
        {
            Ok(child) => {
                self.running = Some(child);
                self.status = String::new();
            }
            Err(e) => self.status = e,
        }
    }

    pub(super) fn home(&mut self, ui: &mut egui::Ui) {
        self.refresh_home();
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, egui::Sense::hover());
        let gap = 16.0;
        let strip_h = 88.0;
        let rest = full.height() - strip_h - 2.0 * gap;
        let hero_h = (rest * 0.55).clamp(190.0, 400.0);
        let hero = Rect::from_min_size(full.min, vec2(full.width(), hero_h));
        let row = Rect::from_min_max(
            pos2(full.left(), hero.bottom() + gap),
            pos2(full.right(), full.bottom() - strip_h - gap),
        );
        let strip = Rect::from_min_max(pos2(full.left(), row.bottom() + gap), full.max);
        self.hero(ui, hero);
        let w = (row.width() - 2.0 * gap) * [0.44, 0.28, 0.28][0];
        let side = (row.width() - 2.0 * gap - w) / 2.0;
        let play = Rect::from_min_size(row.min, vec2(w, row.height()));
        let last = Rect::from_min_size(pos2(play.right() + gap, row.top()), vec2(side, row.height()));
        let news = Rect::from_min_size(pos2(last.right() + gap, row.top()), vec2(side, row.height()));
        self.play_card(ui, play);
        self.last_card(ui, last);
        self.news_card(ui, news);
        self.strip(ui, strip);
    }

    fn hero(&self, ui: &mut egui::Ui, rect: Rect) {
        let p = ui.painter();
        let inside = ornate::Inside::frame(rect, &self.art, 44.0, ornate::Corner::Ornate);
        if !inside.picture(p, &self.art, "hero", rect, Color32::WHITE) {
            // the stand-in: a night sky over the pitch, three hoops
            inside.fill_gradient(
                p,
                rect,
                Color32::from_rgb(0x2c, 0x44, 0x70),
                Color32::from_rgb(0x10, 0x18, 0x2c),
                true,
            );
            let ground = Rect::from_min_max(pos2(rect.left(), rect.bottom() - rect.height() * 0.16), rect.max);
            inside.fill_gradient(
                p,
                ground,
                Color32::from_rgb(0x1e, 0x3a, 0x22),
                Color32::from_rgb(0x10, 0x22, 0x14),
                true,
            );
            let base = ground.top();
            for (k, h) in [(0.62, 0.55), (0.72, 0.68), (0.82, 0.5)] {
                let x = rect.left() + rect.width() * k;
                let top = base - rect.height() * h;
                let ring = rect.height() * 0.07;
                let s = Stroke::new(3.0, style::GOLD.gamma_multiply(0.85));
                p.line_segment([pos2(x, base), pos2(x, top + ring)], Stroke::new(3.0, style::GOLD_DEEP));
                p.circle_stroke(pos2(x, top), ring, s);
            }
        }
        // a shade over the left for the title
        let shade = Rect::from_min_size(rect.min, vec2(rect.width() * 0.6, rect.height()));
        inside.fill_gradient(
            p,
            shade,
            Color32::from_rgba_unmultiplied(8, 12, 22, 225),
            Color32::from_rgba_unmultiplied(8, 12, 22, 0),
            false,
        );
        let k = (rect.height() / 320.0).clamp(0.65, 1.25);
        let cx = rect.left() + rect.width() * 0.25;
        let line = Stroke::new(1.0, style::GOLD.gamma_multiply(0.8));
        let top =
            ornate::spaced(p, "QUIDDITCH", FontId::new(20.0 * k, style::title_family()), style::GOLD_LIGHT, 6.0 * k);
        let big =
            p.layout_no_wrap("WORLD CUP".into(), FontId::new(66.0 * k, style::heading_family()), style::GOLD_LIGHT);
        let sub =
            ornate::spaced(p, "REMASTERED", FontId::new(30.0 * k, style::title_family()), style::GOLD_LIGHT, 9.0 * k);
        // the three lines as one block, in the banner's middle
        let height = top.size().y + 2.0 * k + big.size().y - 4.0 * k + sub.size().y;
        let mut y = rect.center().y - height / 2.0;
        // a short rule either side of a line of `w` across at `y`, its middle `h` down
        let rules = |y: f32, w: f32, h: f32, reach: f32, gap: f32| {
            let m = y + h / 2.0;
            p.line_segment([pos2(cx - w / 2.0 - reach, m), pos2(cx - w / 2.0 - gap, m)], line);
            p.line_segment([pos2(cx + w / 2.0 + gap, m), pos2(cx + w / 2.0 + reach, m)], line);
        };
        rules(y, top.size().x, top.size().y, 70.0 * k, 14.0 * k);
        p.galley(pos2(cx - top.size().x / 2.0, y), top.clone(), style::GOLD_LIGHT);
        y += top.size().y + 2.0 * k;
        p.galley(pos2(cx - big.size().x / 2.0, y), big.clone(), style::GOLD_LIGHT);
        y += big.size().y - 4.0 * k;
        rules(y, sub.size().x, sub.size().y, 60.0 * k, 12.0 * k);
        p.galley(pos2(cx - sub.size().x / 2.0, y), sub, style::GOLD_LIGHT);
        ornate::outline(p, rect, 4.0, &self.art, 44.0);
    }

    fn play_card(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let p = ui.painter().clone();
        let inside = ornate::Inside::frame(rect, &self.art, 36.0, ornate::Corner::Ornate);
        if !inside.picture(&p, &self.art, "panel", rect, Color32::WHITE) {
            inside.fill_gradient(
                &p,
                rect,
                Color32::from_rgb(0x30, 0x25, 0x1a),
                Color32::from_rgb(0x1c, 0x15, 0x0e),
                true,
            );
        }
        ornate::outline(&p, rect, 4.0, &self.art, 36.0);
        let inner = rect.shrink2(vec2(34.0, 22.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            ui.vertical_centered(|ui| {
                let t = ornate::spaced(
                    ui.painter(),
                    "READY TO JUMP IN?",
                    FontId::new(26.0, style::heading_family()),
                    style::GOLD_LIGHT,
                    1.5,
                );
                // the title, the button and the line under it as one block in the card's middle (the button lower
                // when the card is short)
                let playing = self.running.is_some();
                let note = !self.status.is_empty() || self.news_view.newer().is_some();
                let gap = (inner.height() * 0.1).clamp(10.0, 24.0);
                let under = if note { 8.0 + 22.0 } else { 0.0 };
                let height = (inner.height() - t.size().y - gap - under).clamp(48.0, 74.0);
                let block = t.size().y + gap + height + under;
                ui.add_space(((inner.height() - block) / 2.0).max(0.0));
                let (r, _) = ui.allocate_exact_size(t.size(), egui::Sense::hover());
                ui.painter().galley(r.min, t, style::GOLD_LIGHT);
                ui.add_space(gap);
                let size = vec2((inner.width() - 40.0).clamp(220.0, 440.0), height);
                let label = if playing { "Playing" } else { "Play" };
                if ornate::button(ui, &self.art, label, size, Kind::Play, 38.0, !playing, !playing).clicked() {
                    self.play();
                }
                ui.add_space(8.0);
                if !self.status.is_empty() {
                    ui.label(egui::RichText::new(&self.status).color(if playing { style::DIM } else { style::WARN }));
                } else if let Some(new) = self.news_view.newer() {
                    let v = new.version().map_or(new.tag_name.clone(), |v| v.to_string());
                    if ui.link(format!("Version {v} is out: see what's new")).clicked() {
                        self.page = Page::News;
                    }
                }
            });
        });
    }

    fn last_card(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let body = parchment(ui, &self.art, rect, "Last played", Paper::Band);
        let last = self.home_view.last.clone();
        ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
            let Some(m) = last else {
                ui.label(
                    egui::RichText::new("No match played yet")
                        .font(FontId::new(19.0, style::heading_family()))
                        .color(style::INK),
                );
                ui.label(
                    egui::RichText::new("Your last match shows here once you've played one.").color(style::INK_DIM),
                );
                return;
            };
            scoreboard(ui, &self.art, body, &m);
        });
    }

    fn news_card(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let body = parchment(ui, &self.art, rect, "Latest update", Paper::Sketch);
        let ctx = ui.ctx().clone();
        ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let latest =
                self.news_view.releases.as_ref().and_then(|r| r.as_ref().ok()).and_then(|r| r.first()).cloned();
            match latest {
                Some(r) => {
                    let v = r.version().map_or(r.tag_name.clone(), |v| format!("v{v}"));
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(v).font(FontId::new(19.0, style::heading_family())).color(style::INK),
                        );
                        ui.label(egui::RichText::new(format!("  {}", r.date())).color(style::INK_DIM));
                    });
                    for b in bullets(r.body.as_deref().unwrap_or(""), 5) {
                        ui.label(egui::RichText::new(format!("\u{2022} {b}")).color(style::INK));
                    }
                }
                None => {
                    let v = updates::current();
                    ui.label(
                        egui::RichText::new(format!("v{v}"))
                            .font(FontId::new(19.0, style::heading_family()))
                            .color(style::INK),
                    );
                    let what = match &self.news_view.releases {
                        None if self.news_view.busy() => "Looking for updates.".to_string(),
                        None => "Not checked for updates yet.".to_string(),
                        Some(Err(e)) => format!("Couldn't check: {e}"),
                        Some(Ok(_)) => "No releases are published yet.".to_string(),
                    };
                    ui.label(egui::RichText::new(what).color(style::INK_DIM));
                }
            }
            let bottom = Rect::from_min_max(pos2(body.left(), body.bottom() - 40.0), body.max);
            ui.scope_builder(egui::UiBuilder::new().max_rect(bottom), |ui| {
                let w = ui.available_width();
                let has = self.news_view.releases.as_ref().is_some_and(|r| r.as_ref().is_ok_and(|r| !r.is_empty()));
                let label = if has { "View patch notes" } else { "Check for updates" };
                if ornate::button(ui, &self.art, label, vec2(w, 40.0), Kind::Plain, 16.0, true, !self.news_view.busy())
                    .clicked()
                {
                    if has {
                        self.page = Page::News;
                    } else {
                        self.check_releases(&ctx);
                    }
                }
            });
        });
    }

    fn strip(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let p = ui.painter().clone();
        ornate::Inside::frame(rect, &self.art, 30.0, ornate::Corner::Ornate).fill_gradient(
            &p,
            rect,
            Color32::from_rgb(0x2a, 0x20, 0x16),
            Color32::from_rgb(0x1a, 0x13, 0x0d),
            true,
        );
        ornate::outline(&p, rect, 4.0, &self.art, 30.0);
        // a framed icon and three lines
        let section = |p: &egui::Painter, x: f32, icon: &str, lines: [(String, Color32, f32); 3]| {
            let b = Rect::from_center_size(pos2(x + 30.0, rect.center().y), vec2(52.0, 52.0));
            p.rect_filled(b, 4.0, style::WELL.gamma_multiply(0.7));
            p.rect_stroke(b, 4.0, Stroke::new(1.0, style::GOLD_DEEP), egui::StrokeKind::Inside);
            ornate::icon(p, &self.art, icon, b.shrink(12.0), style::GOLD);
            let mut y = rect.top() + 14.0;
            for (t, c, s) in lines {
                let g = p.layout_no_wrap(t, FontId::proportional(s), c);
                p.galley(pos2(b.right() + 16.0, y), g.clone(), c);
                y += g.size().y + 1.0;
            }
        };
        let save_lines = match &self.home_view.save {
            Some(s) => {
                let used = s.slots.iter().filter(|x| x.used).count();
                let cards = s.slots.iter().map(|x| x.cards).max().unwrap_or(0);
                [
                    ("Your save".to_string(), style::DIM, 14.0),
                    (format!("{used} of {} slots in use", s.slots.len()), style::TEXT, 19.0),
                    (format!("Up to {cards} Quidditch cards collected"), style::DIM, 14.0),
                ]
            }
            None => [
                ("Your save".to_string(), style::DIM, 14.0),
                ("No save yet".to_string(), style::TEXT, 19.0),
                ("The game makes one when you start".to_string(), style::DIM, 14.0),
            ],
        };
        section(&p, rect.left() + 20.0, "icon_profile", save_lines);
        let mid = rect.left() + rect.width() * 0.36;
        p.line_segment(
            [pos2(mid - 20.0, rect.top() + 14.0), pos2(mid - 20.0, rect.bottom() - 14.0)],
            Stroke::new(1.0, style::GOLD_DEEP),
        );
        let on = self.profile.packs.iter().filter(|e| e.enabled).count();
        let (state, colour) = match (&self.news_view.releases, self.news_view.newer()) {
            (_, Some(n)) => (
                format!("Version {} is available", n.version().map_or(n.tag_name.clone(), |v| v.to_string())),
                style::WARN,
            ),
            (Some(Ok(_)), None) => ("Up to date".to_string(), style::TEXT),
            _ => ("Installed".to_string(), style::TEXT),
        };
        section(
            &p,
            mid,
            "icon_install",
            [
                ("Installation".to_string(), style::DIM, 14.0),
                (state, colour, 19.0),
                (
                    format!("Version {} \u{b7} {on} pack{} on", updates::current(), if on == 1 { "" } else { "s" }),
                    style::DIM,
                    14.0,
                ),
            ],
        );
        let button = Rect::from_min_size(pos2(rect.right() - 290.0, rect.center().y - 22.0), vec2(266.0, 44.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(button), |ui| {
            if ornate::button(ui, &self.art, "Game files", button.size(), Kind::Plain, 17.0, true, true).clicked() {
                if let Some(d) = &self.profile.data {
                    self.game_path = d.display().to_string();
                }
                if let Some(e) = self.profile.exe.clone() {
                    self.set_exe(e);
                }
                self.page = Page::Setup;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_for_the_log() {
        assert_eq!(super::team("Gryff"), "Gryffindor");
        assert_eq!(super::team("England"), "England");
        assert_eq!(super::stadium("hog_sum"), "Hogwarts, summer");
        assert_eq!(super::stadium("japan"), "Japan");
        assert_eq!(super::bullets("Hi\n- one\n* **two**\nx", 5), ["one", "two"]);
    }
}
