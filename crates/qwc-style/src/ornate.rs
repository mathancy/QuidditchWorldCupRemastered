//! The painted parts of the look: double gold outlines with ornaments at their corners, glows, gradients, the
//! green and navy buttons, the line icons, and the art they use when it is there ([`Art`], a folder of PNGs the
//! user paints; `docs/LAUNCHER_ART.md`). Every piece has a stand-in drawn here, so nothing needs the art.

use std::collections::HashMap;
use std::path::Path;

use eframe::egui::{self, pos2, vec2, Color32, FontId, Pos2, Rect, Stroke, StrokeKind};
use egui::epaint::{RectShape, Shape};
use egui::text::{LayoutJob, TextFormat};

use crate::{GOLD, GOLD_DEEP, GOLD_LIGHT, GREEN, GREEN_DEEP, NAVY, NAVY_LIGHT};

/// The pictures the art folder may hold (`<name>.png`); each is optional.
pub const ART: &[&str] = &[
    "backdrop",
    "hero",
    "parchment",
    "parchment_nosketch",
    "panel",
    "corner",
    "corner_simple",
    "gold_line",
    "tab_selected",
    "icon_play",
    "icon_online",
    "icon_wifi1",
    "icon_wifi2",
    "icon_wifi3",
    "icon_no_wifi",
    "icon_packs",
    "icon_settings",
    "icon_saves",
    "icon_news",
    "icon_profile",
    "icon_install",
    "icon_notes",
    "icon_up",
    "icon_down",
    "icon_delete",
    "icon_plus",
    "icon_folder",
    "icon_download",
    "icon_refresh",
    "icon_left",
    "icon_right",
    "icon_play_arrow",
    "flag_australia",
    "flag_bulgaria",
    "flag_england",
    "flag_france",
    "flag_germany",
    "flag_gryffindor",
    "flag_hufflepuff",
    "flag_japan",
    "flag_nordic",
    "flag_ravenclaw",
    "flag_slytherin",
    "flag_spain",
    "flag_usa",
];

/// The art read from a folder: the [`ART`] pictures it has.
#[derive(Default, Clone)]
pub struct Art {
    textures: HashMap<&'static str, egui::TextureHandle>,
}

impl Art {
    /// The pictures of [`ART`] in `dir` (`<name>.png`), the rest left to their stand-ins.
    pub fn load(ctx: &egui::Context, dir: &Path) -> Art {
        let mut textures = HashMap::new();
        for &name in ART {
            let Ok(bytes) = std::fs::read(dir.join(format!("{name}.png"))) else { continue };
            let Some((rgba, w, h)) = crate::decode_png(&bytes) else {
                eprintln!("{}: not a PNG this reads", dir.join(name).display());
                continue;
            };
            // a picture larger than the device's textures is scaled down to fit
            let mut most = ctx.input(|i| i.max_texture_side).max(256);
            // icons are drawn small: scaled down here, as a texture shown far below its size shimmers
            if name.starts_with("icon_") {
                most = most.min(128);
            }
            // and the teams' badges, at most a scoreboard's size on a 2x screen
            if name.starts_with("flag_") {
                most = most.min(256);
            }
            let (rgba, w, h) = shrink(&rgba, w as usize, h as usize, most);
            let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
            // mipmapped, so fine lines drawn small stay smooth; the frame's line repeats along an edge
            let options = egui::TextureOptions {
                mipmap_mode: Some(egui::TextureFilter::Linear),
                wrap_mode: if name == "gold_line" {
                    egui::TextureWrapMode::Repeat
                } else {
                    egui::TextureWrapMode::ClampToEdge
                },
                ..egui::TextureOptions::LINEAR
            };
            textures.insert(name, ctx.load_texture(format!("art {name}"), image, options));
        }
        Art { textures }
    }

    pub fn get(&self, name: &str) -> Option<&egui::TextureHandle> {
        self.textures.get(name)
    }

    /// The names it has, for the log.
    pub fn names(&self) -> Vec<&'static str> {
        let mut v: Vec<&'static str> = self.textures.keys().copied().collect();
        v.sort_unstable();
        v
    }
}

/// RGBA rows of `w` x `h` scaled down (each pixel the mean of the ones it covers) so neither side is over `most`;
/// as they are when they fit.
fn shrink(rgba: &[u8], w: usize, h: usize, most: usize) -> (Vec<u8>, usize, usize) {
    if w <= most && h <= most {
        return (rgba.to_vec(), w, h);
    }
    let k = most as f32 / w.max(h) as f32;
    let (nw, nh) = (((w as f32 * k) as usize).max(1), ((h as f32 * k) as usize).max(1));
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        let (y0, y1) = (y * h / nh, ((y + 1) * h / nh).max(y * h / nh + 1));
        for x in 0..nw {
            let (x0, x1) = (x * w / nw, ((x + 1) * w / nw).max(x * w / nw + 1));
            let mut sum = [0u32; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = (sy * w + sx) * 4;
                    for c in 0..4 {
                        sum[c] += rgba[i + c] as u32;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let o = (y * nw + x) * 4;
            for c in 0..4 {
                out[o + c] = (sum[c] / n) as u8;
            }
        }
    }
    (out, nw, nh)
}

/// A texture of the colours `from` to `to`, top to bottom (`vertical`) or left to right, made once.
pub fn gradient(ctx: &egui::Context, from: Color32, to: Color32, vertical: bool) -> egui::TextureId {
    let id = egui::Id::unique(("qwc gradient", from.to_array(), to.to_array(), vertical));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t.id();
    }
    let n = 64;
    let pixels: Vec<Color32> = (0..n).map(|i| lerp(from, to, i as f32 / (n - 1) as f32)).collect();
    let size = if vertical { [1, n] } else { [n, 1] };
    let image = egui::ColorImage { size, source_size: vec2(size[0] as f32, size[1] as f32), pixels };
    let t = ctx.load_texture("qwc gradient", image, egui::TextureOptions::LINEAR);
    let tid = t.id();
    ctx.data_mut(|d| d.insert_temp(id, t));
    tid
}

/// Between `a` and `b` by `t` (0 `a`, 1 `b`), each channel.
pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()), m(a.a(), b.a()))
}

/// `rect` filled with the texture `tex` over `uv`, its corners rounded by `radius`, tinted `tint`.
pub fn textured(painter: &egui::Painter, rect: Rect, radius: f32, tex: egui::TextureId, uv: Rect, tint: Color32) {
    painter.add(Shape::Rect(RectShape::filled(rect, radius, tint).with_texture(tex, uv)));
}

/// `rect` filled from `from` to `to`, top to bottom or left to right.
pub fn fill_gradient(painter: &egui::Painter, rect: Rect, radius: f32, from: Color32, to: Color32, vertical: bool) {
    let tex = gradient(painter.ctx(), from, to, vertical);
    textured(painter, rect, radius, tex, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
}

/// The part of a picture of `size` that covers `rect` without stretching it, centred (`focus` moves the centre:
/// 0 to 1 across and down).
pub fn cover_uv(size: [usize; 2], rect: Rect, focus: egui::Vec2) -> Rect {
    let (iw, ih) = (size[0] as f32, size[1] as f32);
    let (rw, rh) = (rect.width().max(1.0), rect.height().max(1.0));
    let scale = (rw / iw).max(rh / ih);
    let (uw, uh) = (rw / scale / iw, rh / scale / ih);
    let x = (1.0 - uw) * focus.x;
    let y = (1.0 - uh) * focus.y;
    Rect::from_min_size(pos2(x, y), vec2(uw, uh))
}

/// A picture of the art covering `rect` (cropped, not stretched), corners rounded; none without it.
pub fn picture(painter: &egui::Painter, art: &Art, name: &str, rect: Rect, radius: f32, tint: Color32) -> bool {
    let Some(t) = art.get(name) else { return false };
    textured(painter, rect, radius, t.id(), cover_uv(t.size(), rect, vec2(0.5, 0.5)), tint);
    true
}

/// A line from `x0` to `x1` at height `y`, `width` thick, in `color`, fading out over a quarter of its length at
/// each end.
pub fn faded_line(painter: &egui::Painter, x0: f32, x1: f32, y: f32, width: f32, color: Color32) {
    let (a, b) = (y - width / 2.0, y + width / 2.0);
    let xs = [x0, x0 + (x1 - x0) * 0.25, x0 + (x1 - x0) * 0.75, x1];
    let colours = [Color32::TRANSPARENT, color, color, Color32::TRANSPARENT];
    let mut mesh = egui::Mesh::default();
    for (x, c) in xs.into_iter().zip(colours) {
        mesh.colored_vertex(pos2(x, a), c);
        mesh.colored_vertex(pos2(x, b), c);
    }
    for i in 0..3u32 {
        let k = i * 2;
        mesh.add_triangle(k, k + 1, k + 2);
        mesh.add_triangle(k + 1, k + 3, k + 2);
    }
    painter.add(Shape::mesh(mesh));
}

/// How far along each side the art's corners cut a frame, for each point of their drawn size: their band's outer edge
/// crosses the corner at 45 degrees, 22 of their 128 pixels from it.
const CORNER_CUT: f32 = 22.0 / 128.0;

/// The part of a rect that a frame shows: the rect with its corners cut as the frame's art cuts them, so what is
/// painted under the frame stays within it. Painting through it clips to it.
#[derive(Clone, Copy, Debug)]
pub struct Inside([Pos2; 8]);

impl Inside {
    /// Within a frame round `rect` of the art's `corner` drawn `ornament` points across; `rect` whole without the art.
    pub fn frame(rect: Rect, art: &Art, ornament: f32, corner: Corner) -> Inside {
        let cut = if ornament > 0.0 && has_frame(art, corner) { ornament * CORNER_CUT } else { 0.0 };
        Inside(octagon(rect, cut, 0.0))
    }

    /// `rect` whole.
    pub fn rect(rect: Rect) -> Inside {
        Inside(octagon(rect, 0.0, 0.0))
    }

    /// The outline, clockwise from the top edge's left end.
    pub fn points(&self) -> [Pos2; 8] {
        self.0
    }

    /// `tex` over `rect` (its `uv` part), tinted, within this.
    pub fn textured(&self, painter: &egui::Painter, rect: Rect, tex: egui::TextureId, uv: Rect, tint: Color32) {
        self.mesh(painter, rect, tex, uv, |_| tint);
    }

    /// [`fill_gradient`] within this.
    pub fn fill_gradient(&self, painter: &egui::Painter, rect: Rect, from: Color32, to: Color32, vertical: bool) {
        let tex = gradient(painter.ctx(), from, to, vertical);
        self.textured(painter, rect, tex, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
    }

    /// [`picture`] within this.
    pub fn picture(&self, painter: &egui::Painter, art: &Art, name: &str, rect: Rect, tint: Color32) -> bool {
        let Some(t) = art.get(name) else { return false };
        self.textured(painter, rect, t.id(), cover_uv(t.size(), rect, vec2(0.5, 0.5)), tint);
        true
    }

    /// The picture `tex` as tall as `rect`, its right edge on `rect`'s (the rest of it to the left, cropped where
    /// `rect` is narrower), its left edge fading in over `fade` of its drawn width where `rect` is wider, so it lies
    /// over a plain paper without a seam; within this.
    pub fn right_fitted(&self, painter: &egui::Painter, tex: &egui::TextureHandle, rect: Rect, fade: f32) {
        let [iw, ih] = tex.size();
        let w = iw as f32 * rect.height() / ih as f32;
        let (left, u0, faded) = if w >= rect.width() {
            (rect.left(), 1.0 - rect.width() / w, 0.0)
        } else {
            (rect.right() - w, 0.0, w * fade)
        };
        let mid = left + faded;
        let u_mid = u0 + (1.0 - u0) * (faded / (rect.right() - left));
        // the fade and the rest apart, the colour changing evenly across each
        if faded > 0.0 {
            let fading = Rect::from_min_max(pos2(left, rect.top()), pos2(mid, rect.bottom()));
            let uv = Rect::from_min_max(pos2(u0, 0.0), pos2(u_mid, 1.0));
            self.mesh(painter, fading, tex.id(), uv, |x| {
                lerp(Color32::TRANSPARENT, Color32::WHITE, ((x - left) / faded).clamp(0.0, 1.0))
            });
        }
        let solid = Rect::from_min_max(pos2(mid, rect.top()), rect.max);
        let uv = Rect::from_min_max(pos2(u_mid, 0.0), pos2(1.0, 1.0));
        self.mesh(painter, solid, tex.id(), uv, |_| Color32::WHITE);
    }

    /// `rect` cut to this (both convex), textured with `tex` (`uv` over `rect`), each corner coloured by `color` of
    /// its x (right where the colour changes evenly across `rect`).
    fn mesh(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        tex: egui::TextureId,
        uv: Rect,
        color: impl Fn(f32) -> Color32,
    ) {
        let mut poly = vec![rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
        let edges = self.0.len();
        for i in 0..edges {
            let (a, b) = (self.0[i], self.0[(i + 1) % edges]);
            if a == b {
                continue;
            }
            // which side of the edge a point is: not below 0 within (the outline runs clockwise on screen)
            let side = |p: Pos2| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
            let mut kept = Vec::with_capacity(poly.len() + 1);
            for j in 0..poly.len() {
                let (p, q) = (poly[j], poly[(j + 1) % poly.len()]);
                let (sp, sq) = (side(p), side(q));
                if sp >= 0.0 {
                    kept.push(p);
                }
                if (sp >= 0.0) != (sq >= 0.0) {
                    kept.push(p + (q - p) * (sp / (sp - sq)));
                }
            }
            poly = kept;
            if poly.len() < 3 {
                return;
            }
        }
        let mut mesh = egui::Mesh::with_texture(tex);
        let size = rect.size().max(vec2(1e-3, 1e-3));
        for p in &poly {
            let t = (*p - rect.min) / size;
            let at = pos2(uv.min.x + t.x * uv.width(), uv.min.y + t.y * uv.height());
            mesh.vertices.push(egui::epaint::Vertex { pos: *p, uv: at, color: color(p.x) });
        }
        for k in 1..poly.len() as u32 - 1 {
            mesh.add_triangle(0, k, k + 1);
        }
        painter.add(Shape::mesh(mesh));
    }
}

/// The picture `tex` filling `rect`, stretched across only: its ends (`cap` pixels each) and its middle (`middle`
/// pixels) kept in proportion at `rect`'s height, the two stretches between them taking the rest. Where `rect` is too
/// narrow for that, the whole picture squeezed to it. Tinted by `tint` (white for as it is).
pub fn sliced(painter: &egui::Painter, tex: &egui::TextureHandle, rect: Rect, cap: f32, middle: f32, tint: Color32) {
    let [iw, ih] = tex.size().map(|v| v as f32);
    let s = rect.height() / ih;
    let (cx, cu) = (rect.center().x, 0.5);
    let columns: Vec<(f32, f32)> = if rect.width() >= (2.0 * cap + middle) * s {
        vec![
            (rect.left(), 0.0),
            (rect.left() + cap * s, cap / iw),
            (cx - middle * s / 2.0, cu - middle / iw / 2.0),
            (cx + middle * s / 2.0, cu + middle / iw / 2.0),
            (rect.right() - cap * s, 1.0 - cap / iw),
            (rect.right(), 1.0),
        ]
    } else {
        vec![(rect.left(), 0.0), (rect.right(), 1.0)]
    };
    let mut mesh = egui::Mesh::with_texture(tex.id());
    for &(x, u) in &columns {
        mesh.vertices.push(egui::epaint::Vertex { pos: pos2(x, rect.top()), uv: pos2(u, 0.0), color: tint });
        mesh.vertices.push(egui::epaint::Vertex { pos: pos2(x, rect.bottom()), uv: pos2(u, 1.0), color: tint });
    }
    for k in (0..columns.len() as u32 - 1).map(|i| i * 2) {
        mesh.add_triangle(k, k + 1, k + 2);
        mesh.add_triangle(k + 1, k + 3, k + 2);
    }
    painter.add(Shape::mesh(mesh));
}

/// A small diamond at `c`, `r` from its middle to a point.
pub fn diamond(painter: &egui::Painter, c: Pos2, r: f32, fill: Color32) {
    let pts = vec![pos2(c.x, c.y - r), pos2(c.x + r, c.y), pos2(c.x, c.y + r), pos2(c.x - r, c.y)];
    painter.add(Shape::convex_polygon(pts, fill, Stroke::NONE));
}

/// A glow round `rect`: `color` fading out over `spread` points.
pub fn glow(painter: &egui::Painter, rect: Rect, radius: f32, color: Color32, spread: f32) {
    let steps = 8;
    for i in 0..steps {
        let t = (i as f32 + 0.5) / steps as f32;
        let alpha = (1.0 - t).powi(2) * 0.55;
        let r = rect.expand(t * spread);
        painter.rect_stroke(
            r,
            radius + t * spread,
            Stroke::new(spread / steps as f32 + 0.6, color.gamma_multiply(alpha)),
            StrokeKind::Middle,
        );
    }
}

/// A quad of `tex`: the corners `pos` (clockwise from the first) take the texture coordinates `uv`.
fn quad(painter: &egui::Painter, tex: egui::TextureId, pos: [Pos2; 4], uv: [Pos2; 4]) {
    let mut mesh = egui::Mesh::with_texture(tex);
    for (pos, uv) in pos.into_iter().zip(uv) {
        mesh.vertices.push(egui::epaint::Vertex { pos, uv, color: Color32::WHITE });
    }
    mesh.indices.extend([0, 1, 2, 0, 2, 3]);
    painter.add(Shape::mesh(mesh));
}

/// The art's frame round `rect`: `corner` at each corner (drawn for the top left, mirrored for the others) `size`
/// points across, and `gold_line` repeated along each edge between them, its first row outermost. Both are drawn
/// at the corner's scale, so their bands meet: the corner's band and the line lie the same pixel in from the edge.
fn art_frame(painter: &egui::Painter, rect: Rect, corner: &egui::TextureHandle, line: &egui::TextureHandle, size: f32) {
    let s = size / corner.size()[0] as f32;
    let (t, span) = (line.size()[1] as f32 * s, line.size()[0] as f32 * s);
    let (o, (l, r, top, b)) = (s, (rect.left(), rect.right(), rect.top(), rect.bottom()));
    // the edges between the corners: u along the edge (repeating), v from the outer row inwards
    let along_x = (r - l - 2.0 * size).max(0.0) / span;
    let along_y = (b - top - 2.0 * size).max(0.0) / span;
    let (x0, x1, y0, y1) = (l + size, r - size, top + size, b - size);
    let id = line.id();
    if along_x > 0.0 {
        let (u0, u1) = (0.0, along_x);
        quad(
            painter,
            id,
            [pos2(x0, top + o), pos2(x1, top + o), pos2(x1, top + o + t), pos2(x0, top + o + t)],
            [pos2(u0, 0.0), pos2(u1, 0.0), pos2(u1, 1.0), pos2(u0, 1.0)],
        );
        quad(
            painter,
            id,
            [pos2(x0, b - o), pos2(x1, b - o), pos2(x1, b - o - t), pos2(x0, b - o - t)],
            [pos2(u0, 0.0), pos2(u1, 0.0), pos2(u1, 1.0), pos2(u0, 1.0)],
        );
    }
    if along_y > 0.0 {
        let (u0, u1) = (0.0, along_y);
        quad(
            painter,
            id,
            [pos2(l + o, y0), pos2(l + o + t, y0), pos2(l + o + t, y1), pos2(l + o, y1)],
            [pos2(u0, 0.0), pos2(u0, 1.0), pos2(u1, 1.0), pos2(u1, 0.0)],
        );
        quad(
            painter,
            id,
            [pos2(r - o, y0), pos2(r - o - t, y0), pos2(r - o - t, y1), pos2(r - o, y1)],
            [pos2(u0, 0.0), pos2(u0, 1.0), pos2(u1, 1.0), pos2(u1, 0.0)],
        );
    }
    // the corners: the art's corner point at the frame's, the rest reaching inwards
    for (p, sx, sy) in [
        (rect.left_top(), 1.0, 1.0),
        (rect.right_top(), -1.0, 1.0),
        (rect.left_bottom(), 1.0, -1.0),
        (rect.right_bottom(), -1.0, -1.0),
    ] {
        let (dx, dy) = (vec2(sx * size, 0.0), vec2(0.0, sy * size));
        quad(
            painter,
            corner.id(),
            [p, p + dx, p + dx + dy, p + dy],
            [pos2(0.0, 0.0), pos2(1.0, 0.0), pos2(1.0, 1.0), pos2(0.0, 1.0)],
        );
    }
}

/// The eight points of `rect` with its corners cut off at 45 degrees, `cut` points along each side, moved `inset`
/// points inwards (the cuts keeping their angle), clockwise from the top edge's left end. A `cut` of 0 is the
/// rectangle (its corners doubled).
pub fn octagon(rect: Rect, cut: f32, inset: f32) -> [Pos2; 8] {
    let (l, r, t, b) = (rect.left() + inset, rect.right() - inset, rect.top() + inset, rect.bottom() - inset);
    // a cut line moved in by `inset` meets the moved sides this far from their corner
    let c = if cut > 0.0 { (cut + inset * (std::f32::consts::SQRT_2 - 2.0)).max(0.0) } else { 0.0 };
    [
        pos2(l + c, t),
        pos2(r - c, t),
        pos2(r, t + c),
        pos2(r, b - c),
        pos2(r - c, b),
        pos2(l + c, b),
        pos2(l, b - c),
        pos2(l, t + c),
    ]
}

/// The window's own border round `rect`, its corners cut `cut` points: the art's `gold_line` as a band all the way
/// round (one pixel in, at `scale` points a pixel of the art, mitred at the cuts), else the double gold line.
pub fn border(painter: &egui::Painter, rect: Rect, cut: f32, art: &Art, scale: f32) {
    if let Some(line) = art.get("gold_line") {
        let (o, t, span) = (scale, line.size()[1] as f32 * scale, line.size()[0] as f32 * scale);
        let (outer, inner) = (octagon(rect, cut, o), octagon(rect, cut, o + t));
        let mut mesh = egui::Mesh::with_texture(line.id());
        let mut along = 0.0;
        for i in 0..8 {
            let j = (i + 1) % 8;
            let len = outer[i].distance(outer[j]);
            if len < 0.01 {
                continue;
            }
            let (u0, u1) = (along / span, (along + len) / span);
            let base = mesh.vertices.len() as u32;
            for (pos, uv) in [
                (outer[i], pos2(u0, 0.0)),
                (outer[j], pos2(u1, 0.0)),
                (inner[j], pos2(u1, 1.0)),
                (inner[i], pos2(u0, 1.0)),
            ] {
                mesh.vertices.push(egui::epaint::Vertex { pos, uv, color: Color32::WHITE });
            }
            mesh.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            along += len;
        }
        painter.add(Shape::mesh(mesh));
        return;
    }
    painter.add(Shape::closed_line(octagon(rect, cut, 0.75).to_vec(), Stroke::new(1.5, GOLD_DEEP)));
    painter.add(Shape::closed_line(octagon(rect, cut, 4.5).to_vec(), Stroke::new(1.0, GOLD.gamma_multiply(0.85))));
}

/// Which of the art's corners a frame takes: the ornate one (the window, the banner, the main cards) or the simple
/// one (the smaller panels: the parchment cards and the pages' tiles).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    Ornate,
    Simple,
}

impl Corner {
    fn art(self) -> &'static str {
        match self {
            Corner::Ornate => "corner",
            Corner::Simple => "corner_simple",
        }
    }
}

/// [`outline_with`] the ornate corner.
pub fn outline(painter: &egui::Painter, rect: Rect, radius: f32, art: &Art, ornament: f32) {
    outline_with(painter, rect, radius, art, ornament, Corner::Ornate);
}

/// Whether the art can frame with `corner` (it has that corner and the line).
pub fn has_frame(art: &Art, corner: Corner) -> bool {
    art.get(corner.art()).is_some() && art.get("gold_line").is_some()
}

/// The frame round `rect`. With the art's `corner` (of [`Corner`]) and `gold_line` and an `ornament` size (the
/// corner's, in points), the art's frame ([`art_frame`]). Otherwise the double gold outline: the gold's shadow
/// outside, the gold a little inside, and at each corner (when `ornament` is not 0) a gold bracket with a diamond.
pub fn outline_with(painter: &egui::Painter, rect: Rect, radius: f32, art: &Art, ornament: f32, corner: Corner) {
    if let (true, Some(c), Some(l)) = (ornament > 0.0, art.get(corner.art()), art.get("gold_line")) {
        art_frame(painter, rect, c, l, ornament.min(rect.width() / 2.0).min(rect.height() / 2.0));
        return;
    }
    painter.rect_stroke(rect, radius, Stroke::new(1.5, GOLD_DEEP), StrokeKind::Inside);
    let inner = rect.shrink(4.0);
    painter.rect_stroke(
        inner,
        (radius - 3.0).max(0.0),
        Stroke::new(1.0, GOLD.gamma_multiply(0.85)),
        StrokeKind::Inside,
    );
    if ornament <= 0.0 {
        return;
    }
    let corners = [
        (rect.left_top(), [1.0, 1.0]),
        (rect.right_top(), [-1.0, 1.0]),
        (rect.left_bottom(), [1.0, -1.0]),
        (rect.right_bottom(), [-1.0, -1.0]),
    ];
    for (p, [sx, sy]) in corners {
        if let Some(t) = art.get("corner") {
            // the art's top left corner: its corner point at this corner, the rest reaching inwards (mirrored)
            let mut mesh = egui::Mesh::with_texture(t.id());
            let (dx, dy) = (vec2(sx * ornament, 0.0), vec2(0.0, sy * ornament));
            for (pos, uv) in
                [(p, pos2(0.0, 0.0)), (p + dx, pos2(1.0, 0.0)), (p + dx + dy, pos2(1.0, 1.0)), (p + dy, pos2(0.0, 1.0))]
            {
                mesh.vertices.push(egui::epaint::Vertex { pos, uv, color: Color32::WHITE });
            }
            mesh.indices.extend([0, 1, 2, 0, 2, 3]);
            painter.add(Shape::mesh(mesh));
            continue;
        }
        // the stand-in: a bracket along both sides from the corner, and a diamond at its heart
        let d = ornament * 0.5;
        let o = p + vec2(sx * 7.0, sy * 7.0);
        let stroke = Stroke::new(1.5, GOLD);
        painter.line_segment([o, o + vec2(sx * d, 0.0)], stroke);
        painter.line_segment([o, o + vec2(0.0, sy * d)], stroke);
        diamond(painter, o + vec2(sx * 5.0, sy * 5.0), 3.5, GOLD_LIGHT);
    }
}

/// `text` in `font`, each letter `spacing` points apart beyond its own width: the titles' spaced capitals.
pub fn spaced(
    painter: &egui::Painter,
    text: &str,
    font: FontId,
    color: Color32,
    spacing: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::default();
    job.append(text, 0.0, TextFormat { font_id: font, color, extra_letter_spacing: spacing, ..Default::default() });
    painter.layout_job(job)
}

/// Which button: the green one to play, a navy one, or a navy one on parchment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Play,
    Plain,
}

/// A button of `kind`, `size` points, its label in the capitals at `font` points (`arrow` an arrow after it: the
/// art's `icon_play_arrow` on Play, `icon_right` otherwise, else one drawn): a gradient body edged in gold, glowing
/// when the pointer is on it.
#[allow(clippy::too_many_arguments)]
pub fn button(
    ui: &mut egui::Ui,
    art: &Art,
    label: &str,
    size: egui::Vec2,
    kind: Kind,
    font: f32,
    arrow: bool,
    enabled: bool,
) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(size, if enabled { egui::Sense::click() } else { egui::Sense::hover() });
    let r = r.on_hover_cursor(if enabled { egui::CursorIcon::PointingHand } else { egui::CursorIcon::Default });
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    if !ui.is_rect_visible(rect) {
        return r;
    }
    let p = ui.painter();
    let hot = enabled && (r.hovered() || r.has_focus());
    let down = enabled && r.is_pointer_button_down_on();
    let (top, bottom, text) = match kind {
        Kind::Play => (GREEN, GREEN_DEEP, GOLD_LIGHT),
        Kind::Plain => (NAVY_LIGHT, NAVY, GOLD_LIGHT),
    };
    let (top, bottom) = if down { (bottom, top) } else { (top, bottom) };
    let fade = |c: Color32| if enabled { c } else { lerp(c, Color32::from_gray(40), 0.6) };
    let radius = if kind == Kind::Play { 10.0 } else { 5.0 };
    if hot {
        glow(
            p,
            rect,
            radius,
            if kind == Kind::Play { GOLD } else { GOLD.gamma_multiply(0.7) },
            if kind == Kind::Play { 14.0 } else { 8.0 },
        );
    }
    fill_gradient(p, rect, radius, fade(top), fade(bottom), true);
    let edge = if hot { GOLD_LIGHT } else { GOLD };
    p.rect_stroke(
        rect,
        radius,
        Stroke::new(if kind == Kind::Play { 2.5 } else { 1.2 }, fade(edge)),
        StrokeKind::Inside,
    );
    if kind == Kind::Play {
        p.rect_stroke(
            rect.shrink(4.0),
            radius - 3.0,
            Stroke::new(1.0, GOLD_DEEP.gamma_multiply(0.8)),
            StrokeKind::Inside,
        );
    }
    let family = crate::heading_family();
    let g = p.layout_no_wrap(label.to_string(), FontId::new(font, family), fade(text));
    let drawn = if arrow { art.get(if kind == Kind::Play { "icon_play_arrow" } else { "icon_right" }) } else { None };
    let chevron = match (arrow, drawn) {
        (false, _) => 0.0,
        (true, Some(_)) => font * if kind == Kind::Play { 0.7 } else { 0.75 },
        (true, None) => font * 0.55,
    };
    let gap = if drawn.is_some() { font * 0.4 } else { font * 0.5 };
    let w = g.size().x + if arrow { chevron + gap } else { 0.0 };
    let start = pos2(rect.center().x - w / 2.0, rect.center().y - g.size().y / 2.0);
    p.galley(start, g.clone(), text);
    if let Some(t) = drawn {
        let c = pos2(start.x + g.size().x + gap + chevron / 2.0, rect.center().y);
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        p.image(t.id(), Rect::from_center_size(c, vec2(chevron, chevron)), uv, fade(text));
    } else if arrow {
        let c = pos2(start.x + g.size().x + font * 0.5 + chevron * 0.35, rect.center().y);
        if kind == Kind::Play {
            let h = chevron * 0.9;
            let pts = vec![pos2(c.x - h * 0.45, c.y - h), pos2(c.x + h * 0.75, c.y), pos2(c.x - h * 0.45, c.y + h)];
            p.add(Shape::convex_polygon(pts, fade(text), Stroke::NONE));
        } else {
            let h = chevron * 0.5;
            p.line(
                vec![pos2(c.x - h * 0.4, c.y - h), pos2(c.x + h * 0.4, c.y), pos2(c.x - h * 0.4, c.y + h)],
                Stroke::new(1.6, fade(text)),
            );
        }
    }
    r
}

/// The icon `name` (`icon_play`, ...) in `rect`: the art's, tinted, else a line drawing of it.
pub fn icon(painter: &egui::Painter, art: &Art, name: &str, rect: Rect, tint: Color32) {
    if let Some(t) = art.get(name) {
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        painter.image(t.id(), rect, uv, tint);
        return;
    }
    let s = Stroke::new((rect.width() / 14.0).max(1.2), tint);
    let c = rect.center();
    let u = rect.width() / 2.0;
    let at = |x: f32, y: f32| pos2(c.x + x * u, c.y + y * u);
    match name {
        // a quill
        "icon_play" => {
            painter.line(vec![at(-0.75, 0.8), at(0.7, -0.75)], s);
            let vane = vec![at(0.7, -0.75), at(-0.1, -0.35), at(-0.35, 0.3), at(0.15, 0.05), at(0.55, -0.2)];
            painter.add(Shape::closed_line(vane, s));
        }
        // a globe: its rim, a meridian and the equator
        "icon_online" => {
            painter.circle_stroke(c, u * 0.8, s);
            painter.add(egui::epaint::EllipseShape::stroke(c, vec2(u * 0.35, u * 0.8), s));
            painter.line_segment([at(-0.8, 0.0), at(0.8, 0.0)], s);
        }
        // a crate seen from a corner
        "icon_packs" => {
            let top = vec![at(0.0, -0.8), at(0.7, -0.45), at(0.0, -0.1), at(-0.7, -0.45)];
            painter.add(Shape::closed_line(top, s));
            painter.line(vec![at(-0.7, -0.45), at(-0.7, 0.4), at(0.0, 0.8), at(0.7, 0.4), at(0.7, -0.45)], s);
            painter.line_segment([at(0.0, -0.1), at(0.0, 0.8)], s);
        }
        // a cog
        "icon_settings" => {
            painter.circle_stroke(c, u * 0.5, s);
            painter.circle_stroke(c, u * 0.2, s);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::TAU / 8.0;
                let d = vec2(a.cos(), a.sin());
                painter.line_segment([c + d * u * 0.5, c + d * u * 0.8], Stroke::new(s.width * 2.0, tint));
            }
        }
        // an open book
        "icon_saves" => {
            painter.line(
                vec![
                    at(0.0, -0.5),
                    at(-0.8, -0.7),
                    at(-0.8, 0.6),
                    at(0.0, 0.8),
                    at(0.8, 0.6),
                    at(0.8, -0.7),
                    at(0.0, -0.5),
                    at(0.0, 0.8),
                ],
                s,
            );
        }
        // a newspaper
        "icon_news" | "icon_notes" => {
            painter.rect_stroke(Rect::from_min_max(at(-0.6, -0.8), at(0.6, 0.8)), 1.0, s, StrokeKind::Middle);
            for y in [-0.45, -0.15, 0.15, 0.45] {
                painter.line_segment([at(-0.35, y), at(0.35, y)], Stroke::new(s.width * 0.8, tint));
            }
        }
        // the connection's bars: a dot and up to two arcs over it; none, the arcs struck through
        "icon_wifi1" | "icon_wifi2" | "icon_wifi3" | "icon_no_wifi" => {
            let base = at(0.0, 0.55);
            let arcs = match name {
                "icon_wifi1" => 0,
                "icon_wifi2" => 1,
                _ => 2,
            };
            painter.circle_filled(base, u * 0.14, tint);
            for k in 1..=arcs {
                let r = u * (0.45 + 0.4 * (k - 1) as f32);
                let points = (0..=12)
                    .map(|i| {
                        let a = std::f32::consts::PI * (1.25 + 0.5 * i as f32 / 12.0);
                        base + vec2(a.cos(), a.sin()) * r
                    })
                    .collect();
                painter.line(points, s);
            }
            if name == "icon_no_wifi" {
                painter.line_segment([at(-0.7, -0.75), at(0.7, 0.75)], s);
            }
        }
        // a head and shoulders
        "icon_profile" => {
            painter.circle_stroke(at(0.0, -0.35), u * 0.35, s);
            painter.line(
                vec![at(-0.7, 0.8), at(-0.6, 0.35), at(-0.3, 0.12), at(0.3, 0.12), at(0.6, 0.35), at(0.7, 0.8)],
                s,
            );
        }
        // a drive
        "icon_install" => {
            painter.rect_stroke(Rect::from_min_max(at(-0.8, -0.35), at(0.8, 0.45)), 3.0, s, StrokeKind::Middle);
            painter.line_segment([at(-0.8, 0.1), at(0.8, 0.1)], s);
            painter.circle_filled(at(0.5, 0.28), s.width, tint);
        }
        _ => {
            diamond(painter, c, u * 0.5, tint);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_octagons_cuts_keep_their_angle_when_moved_in() {
        let r = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 50.0));
        let o = octagon(r, 10.0, 0.0);
        assert_eq!((o[0], o[2], o[7]), (pos2(10.0, 0.0), pos2(100.0, 10.0), pos2(0.0, 10.0)));
        // moved in by 2: on the line x + y = 10 + 2 * sqrt 2, the top-left cut's, still
        let i = octagon(r, 10.0, 2.0);
        for p in [i[0], i[7]] {
            assert!((p.x + p.y - (10.0 + 2.0 * std::f32::consts::SQRT_2)).abs() < 1e-4, "{p:?}");
        }
        assert_eq!(octagon(r, 0.0, 0.0)[0], octagon(r, 0.0, 0.0)[7]);
    }

    #[test]
    fn a_cover_keeps_the_pictures_shape() {
        // a wide picture in a square: the middle third across, all of it down
        let uv = cover_uv([300, 100], Rect::from_min_size(Pos2::ZERO, vec2(50.0, 50.0)), vec2(0.5, 0.5));
        assert!((uv.width() - 1.0 / 3.0).abs() < 1e-5 && (uv.height() - 1.0).abs() < 1e-5);
        assert!((uv.min.x - 1.0 / 3.0).abs() < 1e-5);
        assert_eq!(lerp(Color32::BLACK, Color32::WHITE, 0.5), Color32::from_gray(128));
    }
}
