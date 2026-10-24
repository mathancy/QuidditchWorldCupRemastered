//! The canvas: the scene drawn on the previewed screen with the game's textures, the selection's box, handles and
//! anchors over it, and the picking and dragging that edit it.

use eframe::egui::{self, Color32, Modifiers, PointerButton, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2};
use qwc_hud::{Anchors, Driver, Element, Property};

use crate::app::Editor;

/// How the screen sits in the canvas, and the drag under way.
#[derive(Default)]
pub struct View {
    /// None fits the screen to the canvas.
    zoom: Option<f32>,
    pan: Vec2,
    /// The zoom last drawn at.
    pub zoom_shown: f32,
    /// The pointer on the previewed screen, in its pixels.
    pub pointer: Option<[f32; 2]>,
    drag: Option<Drag>,
}

impl View {
    pub fn fit(&mut self) {
        (self.zoom, self.pan) = (None, Vec2::ZERO);
    }

    pub fn actual(&mut self) {
        (self.zoom, self.pan) = (Some(1.0), Vec2::ZERO);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    Move,
    /// By its edges on the screen: -1 the left (top), 1 the right (bottom), 0 neither.
    Size([i8; 2]),
    Anchor,
    Pan,
}

struct Drag {
    grab: Grab,
    path: Vec<usize>,
    /// Where the pointer was pressed, on the canvas.
    from: Pos2,
    /// The element as it was.
    element: Element,
    /// Screen pixels per unit of its position (signed: negative where its parent mirrors), its parent's size in
    /// units and its parent's box on the screen.
    units: [f32; 2],
    parent: [f32; 2],
    parent_rect: qwc_hud::Rect,
    pan: Vec2,
}

const SELECTED: Color32 = qwc_style::GOLD;
/// The screen's own area, a neutral slate.
const SCREEN: Color32 = Color32::from_rgb(0x2a, 0x2e, 0x3a);
const ANCHOR: Color32 = qwc_style::ACCENT;
const HOVER: Color32 = qwc_style::ACCENT;

fn colour(c: [f32; 4]) -> Color32 {
    let [r, g, b, a] = c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn dashed(painter: &egui::Painter, r: Rect, stroke: Stroke) {
    let path = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    painter.extend(Shape::dashed_line(&path, stroke, 5.0, 4.0));
}

/// The handles of box `r`: where each is and which edges it moves.
fn handles(r: Rect) -> [(Pos2, [i8; 2]); 8] {
    [
        (r.left_top(), [-1, -1]),
        (r.center_top(), [0, -1]),
        (r.right_top(), [1, -1]),
        (r.right_center(), [1, 0]),
        (r.right_bottom(), [1, 1]),
        (r.center_bottom(), [0, 1]),
        (r.left_bottom(), [-1, 1]),
        (r.left_center(), [-1, 0]),
    ]
}

/// Whether the element at `path` or one holding it is locked.
fn locked(ed: &Editor, path: &[usize]) -> bool {
    (1..=path.len()).any(|n| ed.scene.get(&path[..n]).is_some_and(|e| e.locked))
}

/// The element under screen pixel `p`: the topmost drawn there, else the innermost box there.
fn hit(ed: &Editor, p: [f32; 2]) -> Option<Vec<usize>> {
    let inside = |r: &qwc_hud::Rect| p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3];
    let f = &ed.frame;
    f.draws
        .iter()
        .rev()
        .filter(|d| inside(&d.rect))
        .map(|d| &f.placed[d.element])
        .chain(f.placed.iter().rev().filter(|p| p.shown && inside(&p.rect)))
        .find(|p| !locked(ed, &p.path))
        .map(|p| p.path.clone())
}

fn snapped(v: f32, step: f32) -> f32 {
    if step > 0.0 {
        (v / step).round() * step
    } else {
        v
    }
}

/// `d`'s key at value `at` moved by `by` (a new key there when it has none).
fn set_key(d: &mut Driver, at: f32, by: f32) {
    let now = d.at(at);
    match d.keys.iter_mut().find(|k| (k[0] - at).abs() < 1e-3) {
        Some(k) => k[1] += by,
        None => {
            d.keys.push([at, now + by]);
            d.keys.sort_by(|a, b| a[0].total_cmp(&b[0]));
        }
    }
}

pub fn show(ed: &mut Editor, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    let area = response.rect;
    let painter = painter.with_clip_rect(area);
    painter.rect_filled(area, 0.0, qwc_style::WELL);
    let [w, h] = ed.screen.map(|v| v as f32);
    let size = Vec2::new(w, h);
    let fit = ((area.width() - 40.0) / w).min((area.height() - 40.0) / h).max(0.01);
    let mut zoom = ed.view.zoom.unwrap_or(fit);
    // the wheel zooms about the pointer
    let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
    if let (true, Some(p)) = (scroll != 0.0, response.hover_pos()) {
        let new = (zoom * (scroll * 0.002).exp()).clamp(0.02, 16.0);
        let origin = area.center() - size * zoom / 2.0 + ed.view.pan;
        let moved = p - (p - origin) * (new / zoom);
        ed.view.pan = moved - (area.center() - size * new / 2.0);
        (zoom, ed.view.zoom) = (new, Some(new));
    }
    ed.view.zoom_shown = zoom;
    let origin = area.center() - size * zoom / 2.0 + ed.view.pan;
    let at = |x: f32, y: f32| origin + Vec2::new(x, y) * zoom;
    let rect_of = |r: qwc_hud::Rect| Rect::from_min_max(at(r[0], r[1]), at(r[2], r[3]));
    let on_screen = |p: Pos2| [(p.x - origin.x) / zoom, (p.y - origin.y) / zoom];
    let screen = rect_of([0.0, 0.0, w, h]);

    // the screen, the background, the scene
    painter.rect_filled(screen, 0.0, SCREEN);
    if let Some(bg) = &ed.background {
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(bg.texture.id(), screen, uv, Color32::from_white_alpha((bg.opacity * 255.0) as u8));
    }
    ed.frame = ed.scene.evaluate([w, h], &ed.values, true);
    for d in &ed.frame.draws {
        let r = rect_of(d.rect);
        match ed.textures.get(&ctx, &d.texture) {
            Some(t) => {
                let uv = Rect::from_min_max(Pos2::new(d.uv[0], d.uv[1]), Pos2::new(d.uv[2], d.uv[3]));
                painter.image(t.handle.id(), r, uv, colour(d.colour));
            }
            None => {
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_rgb(255, 0, 200)), StrokeKind::Inside);
                let font = egui::FontId::proportional(11.0);
                painter.text(r.left_top(), egui::Align2::LEFT_TOP, &d.texture, font, Color32::from_rgb(255, 0, 200));
            }
        }
    }
    painter.rect_stroke(screen, 0.0, Stroke::new(1.0, Color32::from_gray(110)), StrokeKind::Outside);

    // the guides
    if ed.show_safe {
        for f in [0.9, 0.8] {
            dashed(
                &painter,
                screen.shrink2(screen.size() * (1.0 - f) / 2.0),
                Stroke::new(1.0, Color32::from_gray(150)),
            );
        }
        let [kx, ky] = ed.frame.scale;
        let [dw, dh] = ed.scene.design;
        let [cw, ch] = ed.frame.canvas;
        let design = [(cw - dw) / 2.0 * kx, (ch - dh) / 2.0 * ky, (cw + dw) / 2.0 * kx, (ch + dh) / 2.0 * ky];
        dashed(&painter, rect_of(design), Stroke::new(1.0, qwc_style::GOOD));
    }
    if ed.show_boxes {
        for p in ed.frame.placed.iter().filter(|p| p.shown) {
            let stroke = Stroke::new(1.0, Color32::from_white_alpha(36));
            painter.rect_stroke(rect_of(p.rect), 0.0, stroke, StrokeKind::Middle);
        }
    }
    ed.view.pointer = response.hover_pos().map(on_screen);
    let hovered = if ed.view.drag.is_none() { ed.view.pointer.and_then(|p| hit(ed, p)) } else { None };
    if let Some(p) = hovered.as_ref().filter(|p| Some(*p) != ed.selected.as_ref()) {
        if let Some(pl) = ed.frame.placed.iter().find(|pl| pl.path == *p) {
            painter.rect_stroke(rect_of(pl.rect), 0.0, Stroke::new(1.0, HOVER), StrokeKind::Middle);
        }
    }

    // the selection: its parent, its anchors, its box and handles
    let selected = ed.selected.as_ref().and_then(|s| ed.frame.placed.iter().find(|p| p.path == *s)).cloned();
    let mut flower = None;
    if let Some(sel) = &selected {
        let e = ed.scene.get(&sel.path).cloned().unwrap_or_default();
        let r = rect_of(sel.rect);
        if ed.show_anchors {
            dashed(&painter, rect_of(sel.parent), Stroke::new(1.0, ANCHOR.gamma_multiply(0.6)));
            let a = rect_of(sel.anchors);
            if !e.anchors.stretches(0) && !e.anchors.stretches(1) {
                // the flower at the point, and a line to the element's aligned point
                let c = a.min;
                for d in [Vec2::new(-1.0, -1.0), Vec2::new(1.0, -1.0), Vec2::new(1.0, 1.0), Vec2::new(-1.0, 1.0)] {
                    painter.line_segment([c, c + d * 8.0], Stroke::new(1.5, ANCHOR));
                    painter.circle_filled(c + d * 8.0, 3.0, ANCHOR);
                }
                painter.circle_stroke(c, 3.0, Stroke::new(1.5, ANCHOR));
                let target = r.min + r.size() * Vec2::from(e.alignment);
                let target = Pos2::new(
                    if sel.units[0] < 0.0 { r.max.x - r.width() * e.alignment[0] } else { target.x },
                    if sel.units[1] < 0.0 { r.max.y - r.height() * e.alignment[1] } else { target.y },
                );
                painter.extend(Shape::dashed_line(&[c, target], Stroke::new(1.0, ANCHOR), 4.0, 3.0));
                flower = Some(c);
            } else {
                dashed(&painter, a, Stroke::new(1.0, ANCHOR));
                for c in [a.left_top(), a.right_top(), a.right_bottom(), a.left_bottom()] {
                    painter.circle_filled(c, 3.5, ANCHOR);
                }
            }
        }
        painter.rect_stroke(r, 0.0, Stroke::new(1.5, SELECTED), StrokeKind::Middle);
        if !locked(ed, &sel.path) {
            for (p, _) in handles(r) {
                let h = Rect::from_center_size(p, Vec2::splat(7.0));
                painter.rect_filled(h, 0.0, Color32::WHITE);
                painter.rect_stroke(h, 0.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Middle);
            }
        }
    }

    // picking and dragging
    let (mods, space, press) = ctx.input(|i| (i.modifiers, i.key_down(egui::Key::Space), i.pointer.press_origin()));
    if response.clicked() {
        let picked = ed.view.pointer.and_then(|p| hit(ed, p));
        ed.selected = match picked {
            Some(p) if mods.alt && p.len() > 1 => Some(p[..p.len() - 1].to_vec()),
            p => p,
        };
    }
    if response.drag_started() {
        let from = press.unwrap_or_default();
        let near = |p: Pos2| (p - from).length() <= 7.0;
        let grab = if response.dragged_by(PointerButton::Middle) || space {
            Some((Grab::Pan, Vec::new()))
        } else if let Some(sel) = selected.as_ref().filter(|s| !locked(ed, &s.path)) {
            let r = rect_of(sel.rect);
            if let Some((_, edges)) = handles(r).into_iter().find(|(p, _)| near(*p)) {
                Some((Grab::Size(edges), sel.path.clone()))
            } else if flower.is_some_and(near) {
                Some((Grab::Anchor, sel.path.clone()))
            } else if r.contains(from) {
                Some((Grab::Move, sel.path.clone()))
            } else {
                hit(ed, on_screen(from)).map(|p| (Grab::Move, p))
            }
        } else {
            hit(ed, on_screen(from)).map(|p| (Grab::Move, p))
        };
        if let Some((grab, path)) = grab {
            if grab != Grab::Pan {
                ed.selected = Some(path.clone());
            }
            let placed = ed.frame.placed.iter().find(|p| p.path == path);
            ed.view.drag = Some(Drag {
                grab,
                element: ed.scene.get(&path).cloned().unwrap_or_default(),
                units: placed.map_or([1.0; 2], |p| p.units),
                parent: placed.map_or(ed.scene.design, |p| p.parent_size),
                parent_rect: placed.map_or([0.0; 4], |p| p.parent),
                path,
                from,
                pan: ed.view.pan,
            });
        }
    }
    if let (true, true, Some(p)) = (ed.view.drag.is_some(), response.dragged(), response.interact_pointer_pos()) {
        apply(ed, p, on_screen(p), zoom, mods);
    }
    if response.drag_stopped() || !ctx.input(|i| i.pointer.any_down()) {
        ed.view.drag = None;
    }
    response.context_menu(|ui| crate::panels::context_menu(ed, ui));
}

/// The drag under way, the pointer at canvas point `p` (screen pixel `screen_at`).
fn apply(ed: &mut Editor, p: Pos2, screen_at: [f32; 2], zoom: f32, mods: Modifiers) {
    let Some(drag) = &ed.view.drag else { return };
    let mut delta = (p - drag.from) / zoom;
    if drag.grab == Grab::Pan {
        ed.view.pan = drag.pan + (p - drag.from);
        return;
    }
    let snap = if mods.alt { 0.0 } else { ed.snap };
    let mut e = drag.element.clone();
    let parent = drag.parent;
    match drag.grab {
        Grab::Move => {
            if mods.shift {
                if delta.x.abs() > delta.y.abs() {
                    delta.y = 0.0;
                } else {
                    delta.x = 0.0;
                }
            }
            let mut d = [delta.x / drag.units[0], delta.y / drag.units[1]];
            if ed.record {
                for (axis, property) in [(0, Property::X), (1, Property::Y)] {
                    if let Some(driver) = e.drivers.iter_mut().find(|d| d.property == property) {
                        let at = ed.values.number(&driver.value);
                        set_key(driver, at, d[axis]);
                        d[axis] = 0.0;
                    }
                }
            }
            let b = e.local_box(parent);
            for axis in 0..2 {
                if d[axis] != 0.0 {
                    d[axis] = snapped(b[axis] + d[axis], snap) - b[axis];
                }
            }
            e.set_local_box([b[0] + d[0], b[1] + d[1], b[2] + d[0], b[3] + d[1]], parent);
        }
        Grab::Size(edges) => {
            let mut b = e.local_box(parent);
            for axis in 0..2 {
                let k = drag.units[axis] * e.scale[axis];
                if edges[axis] == 0 || k == 0.0 {
                    continue;
                }
                // the screen's far edge is the box's far edge unless its frame mirrors
                let far = (edges[axis] > 0) == (k > 0.0);
                let i = if far { axis + 2 } else { axis };
                b[i] = snapped(b[i] + [delta.x, delta.y][axis] / k, snap);
                if b[axis + 2] < b[axis] {
                    b[i] = b[if far { axis } else { axis + 2 }];
                }
            }
            e.set_local_box(b, parent);
        }
        Grab::Anchor => {
            let r = drag.parent_rect;
            let mut f = [0.0; 2];
            for axis in 0..2 {
                let span = r[axis + 2] - r[axis];
                let mut v = if span > 0.0 { (screen_at[axis] - r[axis]) / span } else { 0.0 };
                if drag.units[axis] < 0.0 {
                    v = 1.0 - v;
                }
                v = v.clamp(0.0, 1.0);
                // held at the edges and the middle
                for edge in [0.0, 0.5, 1.0] {
                    if (v - edge).abs() < 0.03 {
                        v = edge;
                    }
                }
                f[axis] = if snap > 0.0 && !mods.alt { snapped(v, 0.05) } else { v };
            }
            e.set_anchors_keeping_box(Anchors::point(f[0], f[1]), parent);
        }
        Grab::Pan => {}
    }
    let path = drag.path.clone();
    if let Some(t) = ed.scene.get_mut(&path) {
        *t = e;
    }
}
