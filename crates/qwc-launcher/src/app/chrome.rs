//! The window's own frame (it has no system title bar): the backdrop, the top bar that is part of it (the logo's
//! plate, the pages as tabs, the window's buttons; dragged to move the window, double-clicked to maximise it), the
//! gold outline round it, and its edges, which resize it.

use eframe::egui::{self, pos2, vec2, Color32, FontId, Rect, Sense, Stroke, StrokeKind};
use qwc_style::ornate::{self, Art};

use super::{Launcher, Page};
use crate::style;

/// The top bar's height.
pub(super) const BAR: f32 = 84.0;

/// How the selected tab's art (`tab_selected.png`, 230 x 108, drawn at half its pixels) sits: its `height` drawn,
/// how far the middle of its gold foot is `lift`ed above its bottom (its glow below), its `body` (the tab's own
/// height, foot to top), and the pixels of its ends (`cap`) and middle (`middle`, the diamonds) that keep their shape
/// when it stretches.
struct TabArt {
    height: f32,
    lift: f32,
    body: f32,
    cap: f32,
    middle: f32,
}

const TAB_ART: TabArt = TabArt { height: 54.0, lift: 5.25, body: 44.0, cap: 12.0, middle: 34.0 };

/// The pages as tabs: the page, its label, its icon.
const TABS: [(Page, &str, &str); 6] = [
    (Page::Home, "Play", "icon_play"),
    (Page::Online, "Online", "icon_online"),
    (Page::Packs, "Packs", "icon_packs"),
    (Page::Settings, "Settings", "icon_settings"),
    (Page::Saves, "Saves", "icon_saves"),
    (Page::News, "News", "icon_news"),
];

/// The backdrop behind everything: the art's (darkened) or the wood's gradient.
pub(super) fn backdrop(ctx: &egui::Context, art: &Art) {
    let rect = ctx.content_rect();
    let p = ctx.layer_painter(egui::LayerId::background());
    if !ornate::picture(&p, art, "backdrop", rect, 0.0, Color32::from_gray(150)) {
        ornate::fill_gradient(&p, rect, 0.0, Color32::from_rgb(0x22, 0x19, 0x11), style::BACKGROUND, true);
    }
}

/// The gold outline round the window, over everything but popups, and its edges' resizing (not when maximised).
pub(super) fn frame(ctx: &egui::Context, art: &Art) {
    let rect = ctx.content_rect();
    let maximised = ctx.input(|i| i.viewport().maximized).unwrap_or(false);
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::unique("qwc window frame")));
    // the band along the window's edge, its corners cut as the window is (`super::shape`)
    let cut = if super::shape::cut(ctx) { super::shape::CUT } else { 0.0 };
    ornate::border(&p, rect, cut, art, 44.0 / 128.0);
    if maximised {
        return;
    }
    // within this many points of an edge the pointer resizes the window
    let edge = 6.0;
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
    let (l, r) = (pos.x - rect.left() < edge, rect.right() - pos.x < edge);
    let (t, b) = (pos.y - rect.top() < edge, rect.bottom() - pos.y < edge);
    use egui::ResizeDirection as D;
    let dir = match (l, r, t, b) {
        (true, _, true, _) => D::NorthWest,
        (_, true, true, _) => D::NorthEast,
        (true, _, _, true) => D::SouthWest,
        (_, true, _, true) => D::SouthEast,
        (true, ..) => D::West,
        (_, true, ..) => D::East,
        (_, _, true, _) => D::North,
        (.., true) => D::South,
        _ => return,
    };
    let cursor = match dir {
        D::NorthWest | D::SouthEast => egui::CursorIcon::ResizeNwSe,
        D::NorthEast | D::SouthWest => egui::CursorIcon::ResizeNeSw,
        D::West | D::East => egui::CursorIcon::ResizeHorizontal,
        D::North | D::South => egui::CursorIcon::ResizeVertical,
    };
    ctx.set_cursor_icon(cursor);
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
    }
}

/// How far the window buttons' cut-out reaches in from the right, and down from the top.
const CUT: (f32, f32) = (172.0, 54.0);

/// The window buttons' cut-out in the bar's top right: a little darker than the wood, its left side curving down
/// from the window's top edge into a line under the buttons, edged in a deep brown with a faint light under it, as
/// a groove in the wood rather than an inlay.
fn cutout(p: &egui::Painter, bar: Rect) {
    let (x0, top, bottom) = (bar.right() - CUT.0, bar.top(), bar.top() + CUT.1);
    let curve = egui::epaint::CubicBezierShape::from_points_stroke(
        // down from the top edge, then bending into the line under the buttons
        [pos2(x0, top), pos2(x0 + 1.0, top + 30.0), pos2(x0 + 4.0, bottom), pos2(x0 + 30.0, bottom)],
        false,
        Color32::TRANSPARENT,
        Stroke::NONE,
    );
    let mut edge = curve.flatten(Some(0.3));
    edge.push(pos2(bar.right(), bottom));
    // the fill: a fan from the window's top right corner over the curve and the line
    let fill = Color32::from_black_alpha(70);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(pos2(bar.right(), top), fill);
    for q in &edge {
        mesh.colored_vertex(*q, fill);
    }
    for i in 1..edge.len() as u32 {
        mesh.add_triangle(0, i, i + 1);
    }
    p.add(egui::Shape::mesh(mesh));
    // a soft shade just inside the edge, so the cut-out reads as sunk into the bar
    let inside: Vec<_> = edge.iter().map(|q| *q + vec2(3.0, -3.0)).collect();
    p.add(egui::Shape::line(inside, Stroke::new(6.0, Color32::from_black_alpha(45))));
    // the groove: a faint light catching its lower lip, then the deep brown line itself
    let lip: Vec<_> = edge.iter().map(|q| *q + vec2(-0.8, 1.6)).collect();
    p.add(egui::Shape::line(lip, Stroke::new(1.0, Color32::from_rgba_unmultiplied(0x8a, 0x6a, 0x48, 70))));
    p.add(egui::Shape::line(edge, Stroke::new(2.2, Color32::from_rgb(0x1c, 0x13, 0x0c))));
}

impl Launcher {
    /// The top bar: the logo's plate, the pages as tabs once the game is set up, the window's buttons.
    pub(super) fn top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let bar = ui.max_rect();
        // the bar itself moves the window (the widgets on it take their clicks first)
        let drag = ui.interact(bar, ui.scope_id().with("drag bar"), Sense::click_and_drag());
        if drag.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            let max = ctx.input(|i| i.viewport().maximized).unwrap_or(false);
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
        }
        let p = ui.painter().clone();
        // the backdrop's wood shows through the bar, shaded a little towards the top for depth (without the
        // backdrop's art, the wood's colours)
        if self.art.get("backdrop").is_some() {
            ornate::fill_gradient(&p, bar, 0.0, Color32::from_black_alpha(90), Color32::from_black_alpha(25), true);
        } else {
            ornate::fill_gradient(&p, bar, 0.0, Color32::from_rgb(0x2a, 0x20, 0x17), style::PANEL, true);
        }
        // the bar's foot: the gold over its shadow
        p.line_segment(
            [bar.left_bottom() + vec2(0.0, -1.0), bar.right_bottom() + vec2(0.0, -1.0)],
            Stroke::new(2.0, style::GOLD_DEEP),
        );
        p.line_segment(
            [bar.left_bottom() + vec2(0.0, -5.0), bar.right_bottom() + vec2(0.0, -5.0)],
            Stroke::new(1.0, style::GOLD.gamma_multiply(0.7)),
        );

        // the logo's plate, the logo alone
        let plate = Rect::from_min_size(bar.min + vec2(12.0, 9.0), vec2(80.0, BAR - 20.0));
        ornate::fill_gradient(&p, plate, 3.0, style::WELL, style::PANEL, true);
        ornate::outline(&p, plate, 3.0, &self.art, 26.0);
        if let Some(logo) = &self.logo {
            let r = Rect::from_center_size(plate.center(), vec2(54.0, 54.0));
            p.image(logo.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }

        // the window's buttons, right, in their cut-out
        cutout(&p, bar);
        let mut right = bar.right() - 10.0;
        for which in ["close", "max", "min"] {
            let r = Rect::from_min_size(pos2(right - 46.0, bar.top() + 12.0), vec2(46.0, 32.0));
            right -= 48.0;
            let resp = ui.interact(r, ui.scope_id().with(which), Sense::click());
            let hot = resp.hovered();
            if hot {
                let fill = if which == "close" { Color32::from_rgb(0x9a, 0x2c, 0x22) } else { style::NAVY_LIGHT };
                p.rect_filled(r, 4.0, fill);
            }
            let c = r.center();
            let s = Stroke::new(1.4, if hot { style::GOLD_LIGHT } else { style::GOLD });
            match which {
                "close" => {
                    p.line_segment([c + vec2(-7.0, -7.0), c + vec2(7.0, 7.0)], s);
                    p.line_segment([c + vec2(-7.0, 7.0), c + vec2(7.0, -7.0)], s);
                }
                "max" => {
                    p.rect_stroke(Rect::from_center_size(c, vec2(14.0, 12.0)), 1.0, s, StrokeKind::Middle);
                }
                // level with the others' middles
                _ => {
                    p.line_segment([c + vec2(-7.0, 0.0), c + vec2(7.0, 0.0)], s);
                }
            }
            let label = match which {
                "close" => "Close",
                "max" => "Maximise",
                _ => "Minimise",
            };
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
            if resp.clicked() {
                match which {
                    "close" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    "max" => {
                        let max = ctx.input(|i| i.viewport().maximized).unwrap_or(false);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                    }
                    _ => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                }
            }
        }

        // the pages as tabs, once the game is set up
        if !self.ready() {
            return;
        }
        let mut x = plate.right() + 26.0;
        let font = FontId::new(19.0, style::heading_family());
        // the tabs stand on the foot's upper line
        let foot = bar.bottom() - 5.0;
        for (i, (page, label, icon)) in TABS.into_iter().enumerate() {
            let g = p.layout_no_wrap(label.to_string(), font.clone(), style::GOLD_LIGHT);
            let w = 22.0 + 26.0 + 10.0 + g.size().x + 22.0;
            let r = Rect::from_min_max(pos2(x, foot - TAB_ART.body), pos2(x + w, foot));
            if r.right() > right - 8.0 {
                break;
            }
            let resp = ui.interact(r, ui.scope_id().with(("tab", i)), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, self.page == page, label));
            let selected = self.page == page;
            let hovered = resp.hovered() && !selected;
            if let (true, Some(tex)) = (selected || hovered, self.art.get("tab_selected")) {
                // the art's gold foot over the line; hovered, the same shape faintly
                let art = Rect::from_min_max(
                    pos2(r.left(), foot + TAB_ART.lift - TAB_ART.height),
                    pos2(r.right(), foot + TAB_ART.lift),
                );
                let tint = if selected { Color32::WHITE } else { Color32::WHITE.gamma_multiply(0.35) };
                ornate::sliced(&p, tex, art, TAB_ART.cap, TAB_ART.middle, tint);
            } else if selected {
                ornate::glow(&p, r, 4.0, style::GOLD, 10.0);
                ornate::fill_gradient(
                    &p,
                    r,
                    4.0,
                    Color32::from_rgb(0x3a, 0x2c, 0x1c),
                    Color32::from_rgb(0x22, 0x1a, 0x12),
                    true,
                );
                ornate::outline(&p, r, 4.0, &self.art, 20.0);
                ornate::diamond(&p, pos2(r.center().x, r.top()), 4.5, style::GOLD_LIGHT);
                ornate::diamond(&p, pos2(r.center().x, r.bottom()), 4.5, style::GOLD_LIGHT);
            } else if hovered {
                p.rect_filled(r, 4.0, style::GOLD.gamma_multiply(0.08));
            }
            let tint = if selected || resp.hovered() { style::GOLD_LIGHT } else { style::GOLD };
            let icon_rect = Rect::from_center_size(pos2(r.left() + 22.0 + 13.0, r.center().y), vec2(26.0, 26.0));
            ornate::icon(&p, &self.art, icon, icon_rect, tint);
            p.galley(pos2(icon_rect.right() + 10.0, r.center().y - g.size().y / 2.0), g, tint);
            if resp.clicked() {
                self.go(page);
            }
            // a thin rule between tabs
            if i + 1 < TABS.len() {
                let sx = r.right() + 4.0;
                p.line_segment(
                    [pos2(sx, r.top() + 12.0), pos2(sx, r.bottom() - 12.0)],
                    Stroke::new(1.0, style::GOLD_DEEP.gamma_multiply(0.6)),
                );
            }
            x = r.right() + 8.0;
        }
    }

    /// Go to `page`, its view read afresh.
    pub(super) fn go(&mut self, page: Page) {
        self.page = page;
        match page {
            Page::Packs => self.packs_view = super::packs::PacksView::default(),
            Page::Settings => self.settings_view = super::settings::SettingsView::load(),
            Page::Saves => self.saves_view = super::saves::SavesView::default(),
            _ => {}
        }
    }
}
