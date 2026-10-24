//! The Screens mode's crop tool: a picture cut out of one of the disc's textures (`qwc_frontend::crops`). Only the
//! recipe is kept: the texture's name and the corners of the polygon kept, in the pack's `screens/crops.ron`. The
//! game cuts the picture from the player's own disc when it loads, so a pack's layouts can use crops without
//! shipping any of the game's art.
//!
//! Open it from a sprite's Picture row (Crop...). The texture is shown with what is cut away faded; click to place
//! a corner (snapped to the texture's corners, edge middles and centre when near, else to whole pixels; Shift keeps
//! the side from the corner before at a multiple of 45 degrees; Alt places it anywhere), drag one to move it, and
//! right-click one to take it out. The halves are one click. Keep outside keeps the rest of the texture instead:
//! the other part of a texture cut in two, sharing no pixel with it. Every change is drawn on the screen at once;
//! the crops are saved with the layouts (Ctrl+S).

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use qwc_frontend::crops::{self, Crop};

use crate::screens::{Key, Screens};

/// The tool's state: the crop edited, by its name in the session's crops.
#[derive(Default)]
pub struct CropTool {
    pub open: bool,
    /// The crop edited (a key of the session's crops), and the name being typed for it.
    pub name: String,
    rename: String,
    /// The texture list's filter, and the disc's front-end textures (listed the first time they are shown).
    filter: String,
    textures: Option<Vec<String>>,
    /// The corner being dragged.
    drag: Option<usize>,
    /// The texture shown, and the cut drawn from it, on the GPU, with what each was made from.
    source: Option<(egui::TextureHandle, String)>,
    preview: Option<(egui::TextureHandle, Crop)>,
    pub status: String,
}

/// Snapping to a point of the texture within this many of the canvas's pixels.
const SNAP: f32 = 10.0;

/// The tool opened on sprite `key`: the crop it shows, else a new crop of the disc texture it shows.
pub fn open_for(sc: &mut Screens, key: Key) {
    let Some(s) = &mut sc.session else { return };
    let tool = &mut sc.crop;
    tool.open = true;
    let Some(i) = s.object(key) else { return };
    let Some(shown) = s.shown(i) else {
        tool.status = "The sprite shows nothing yet".into();
        return;
    };
    if let Some(name) = crops::name(&shown) {
        tool.select(name);
        return;
    }
    if qwc_frontend::layouts::is_pack_image(&shown) {
        tool.status = "This sprite shows a texture pack's PNG; a crop is cut from one of the disc's textures. Set \
                       its own picture first (Its own), then crop that."
            .into();
        return;
    }
    let stem = shown.rsplit('/').next().unwrap_or(&shown).rsplit_once('.').map_or(shown.as_str(), |(s, _)| s);
    let mut name = stem.to_ascii_lowercase();
    let mut n = 2;
    while s.crops.contains_key(&name) {
        name = format!("{}_{n}", stem.to_ascii_lowercase());
        n += 1;
    }
    s.crops.insert(name.clone(), Crop { texture: shown.clone(), ..Crop::default() });
    s.apply();
    tool.select(&name);
    tool.status = format!("New crop {name} of {shown}: place its corners, or pick a half");
}

impl CropTool {
    fn select(&mut self, name: &str) {
        self.name = name.to_string();
        self.rename = name.to_string();
        self.drag = None;
        self.status.clear();
    }
}

/// The tool's window, when it is open.
pub fn window(sc: &mut Screens, ctx: &egui::Context) {
    if !sc.crop.open {
        return;
    }
    let mut open = true;
    egui::Window::new("Crop a texture").open(&mut open).default_size([560.0, 760.0]).resizable(true).show(ctx, |ui| {
        body(sc, ui);
    });
    sc.crop.open = open;
}

fn body(sc: &mut Screens, ui: &mut egui::Ui) {
    let vfs = sc.vfs();
    let Some(s) = &mut sc.session else {
        ui.label("No screen is running");
        return;
    };
    let tool = &mut sc.crop;
    ui.weak(
        "A picture cut out of one of the disc's textures. Only the recipe is kept (the texture's name and the \
         corners), so a pack can share it: the game cuts it from the player's own disc.",
    );
    // which crop
    ui.horizontal(|ui| {
        ui.label("Crop");
        let names: Vec<String> = s.crops.keys().cloned().collect();
        let shown = if tool.name.is_empty() { "(none)".to_string() } else { tool.name.clone() };
        egui::ComboBox::from_id_salt("crop name").selected_text(shown).width(220.0).show_ui(ui, |ui| {
            for n in &names {
                if ui.selectable_label(*n == tool.name, n).clicked() {
                    tool.select(n);
                }
            }
        });
        if let Some(key) = s.selected {
            if ui.button("Use on the selected sprite").clicked() && s.crops.contains_key(&tool.name) {
                s.set_image(key, Some(crops::spec(&tool.name)));
                tool.status = format!("The selected sprite shows {}", tool.name);
            }
        }
    });
    let Some(mut crop) = s.crops.get(&tool.name).cloned() else {
        ui.label("Select a sprite showing one of the disc's textures and press Crop... beside its picture.");
        if !tool.status.is_empty() {
            ui.weak(&tool.status);
        }
        return;
    };
    ui.horizontal(|ui| {
        ui.label("Name");
        ui.add(egui::TextEdit::singleline(&mut tool.rename).desired_width(200.0));
        let new = tool.rename.trim().to_string();
        let ok = !new.is_empty() && new != tool.name && !s.crops.contains_key(&new);
        if ui.add_enabled(ok, egui::Button::new("Rename")).on_hover_text("The layouts showing it follow").clicked() {
            s.rename_crop(&tool.name, &new);
            tool.select(&new);
        }
        let users = s.crop_users(&tool.name);
        let delete = egui::Button::new("Delete");
        let hint = if users > 0 { format!("{users} sprites show it") } else { "No sprite shows it".to_string() };
        if ui.add_enabled(users == 0, delete).on_hover_text(hint).clicked() {
            s.crops.remove(&tool.name);
            s.apply();
            tool.status = format!("Deleted {}", tool.name);
            tool.name.clear();
        }
    });
    if tool.name.is_empty() {
        return;
    }
    // its texture
    let Some(size) = s.texture_size(&crop.texture) else {
        ui.colored_label(qwc_style::BAD, format!("{}: no such texture on the disc", crop.texture));
        return;
    };
    let (w, h) = (size[0] as f32, size[1] as f32);
    ui.horizontal(|ui| {
        ui.label("Texture");
        ui.monospace(format!("{} ({} x {})", crop.texture, size[0], size[1]));
    });
    egui::CollapsingHeader::new("Another texture").id_salt("crop textures").show(ui, |ui| {
        ui.add(egui::TextEdit::singleline(&mut tool.filter).hint_text("filter").desired_width(f32::INFINITY));
        let list = tool.textures.get_or_insert_with(|| {
            let all = vfs.with_extension("fsh").map(qwc_assets::vfs::normalize);
            let mut v: Vec<String> = all.filter(|p| p.starts_with("frontend/")).collect();
            v.sort();
            v
        });
        let filter = tool.filter.to_ascii_lowercase();
        egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
            for t in list.iter().filter(|t| t.contains(&filter)).take(400) {
                if ui.selectable_label(*t == crop.texture, t).clicked() {
                    // the corners kept, in the same share of the new texture
                    if let Some(new) = s.texture_size(t) {
                        let k = [new[0] as f32 / w, new[1] as f32 / h];
                        crop.points.iter_mut().for_each(|p| *p = [p[0] * k[0], p[1] * k[1]]);
                    }
                    crop.texture = t.clone();
                }
            }
        });
    });
    // the halves, and the rest
    ui.horizontal_wrapped(|ui| {
        ui.label("Keep");
        let halves = [
            ("Top left half", [[0.0, 0.0], [w, 0.0], [0.0, h]]),
            ("Top right half", [[0.0, 0.0], [w, 0.0], [w, h]]),
            ("Bottom left half", [[0.0, 0.0], [w, h], [0.0, h]]),
            ("Bottom right half", [[w, 0.0], [w, h], [0.0, h]]),
        ];
        for (label, points) in halves {
            if ui.button(label).on_hover_text("Cut along the texture's diagonal").clicked() {
                crop.points = points.to_vec();
                crop.outside = false;
            }
        }
        if ui.button("All").on_hover_text("No cut: every corner taken out").clicked() {
            crop.points.clear();
            crop.outside = false;
        }
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut crop.outside, "Keep outside").on_hover_text(
            "Keep what lies outside the corners instead: the other part of a texture cut in two, sharing no pixel \
             with it",
        );
        if ui.add_enabled(!crop.points.is_empty(), egui::Button::new("Take out the last corner")).clicked() {
            crop.points.pop();
        }
    });
    let hover = canvas(tool, s, ui, &mut crop, size);
    ui.horizontal(|ui| {
        match hover {
            Some(p) => ui.monospace(format!("{:.1}, {:.1}", p[0], p[1])),
            None => ui.monospace(" "),
        };
        ui.weak(format!("{} corners", crop.points.len()));
    });
    ui.weak(
        "Click to place a corner: it snaps to the texture's corners, the middles of its edges and its centre when \
         near, else to whole pixels; Shift keeps the side from the corner before at a multiple of 45 degrees, Alt \
         places it anywhere. Drag a corner to move it; right-click it to take it out. Fewer than three corners keep \
         the whole texture.",
    );
    if !tool.status.is_empty() {
        ui.weak(&tool.status);
    }
    // a change is drawn on the screen at once
    if s.crops.get(&tool.name) != Some(&crop) {
        s.crops.insert(tool.name.clone(), crop);
        s.apply();
    }
}

/// The texture with the cut drawn over it, and the corners to place and drag; answers the pointer in the texture's
/// pixels.
fn canvas(
    tool: &mut CropTool,
    s: &mut crate::screens::Session,
    ui: &mut egui::Ui,
    crop: &mut Crop,
    size: [u16; 2],
) -> Option<[f32; 2]> {
    let ctx = ui.ctx().clone();
    let (w, h) = (size[0] as f32, size[1] as f32);
    let side = ui.available_width().clamp(200.0, 720.0);
    let k = (side / w).min(side / h);
    let (response, painter) = ui.allocate_painter(Vec2::new(w * k, h * k), Sense::click_and_drag());
    let area = response.rect;
    // a checkerboard under the transparent parts
    let cell = 12.0;
    let painter = painter.with_clip_rect(area);
    for (iy, y) in (0..((area.height() / cell).ceil() as usize)).map(|i| (i, area.top() + i as f32 * cell)) {
        for (ix, x) in (0..((area.width() / cell).ceil() as usize)).map(|i| (i, area.left() + i as f32 * cell)) {
            let grey = if (ix + iy) % 2 == 0 { 70 } else { 90 };
            painter.rect_filled(Rect::from_min_size(Pos2::new(x, y), Vec2::splat(cell)), 0.0, Color32::from_gray(grey));
        }
    }
    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    if let Some(src) = source(tool, s, &ctx, &crop.texture) {
        painter.image(src, area, uv, Color32::from_white_alpha(70));
    }
    if let Some(cut) = preview(tool, s, &ctx, crop) {
        painter.image(cut, area, uv, Color32::WHITE);
    }
    painter.rect_stroke(area, 0.0, Stroke::new(1.0, Color32::from_gray(140)), StrokeKind::Outside);
    let to_canvas = |p: [f32; 2]| Pos2::new(area.left() + p[0] * k, area.top() + p[1] * k);
    let to_texture = |p: Pos2| [(p.x - area.left()) / k, (p.y - area.top()) / k];
    // the polygon
    let n = crop.points.len();
    for i in 0..n {
        let (a, b) = (crop.points[i], crop.points[(i + 1) % n]);
        if n >= 2 && (i + 1 < n || n >= 3) {
            painter.line_segment([to_canvas(a), to_canvas(b)], Stroke::new(1.5, qwc_style::GOLD));
        }
    }
    let near = |points: &[[f32; 2]], p: Pos2| points.iter().position(|&c| to_canvas(c).distance(p) <= 7.0);
    let hovered = response.hover_pos().and_then(|p| near(&crop.points, p));
    for (i, &c) in crop.points.iter().enumerate() {
        let on = hovered == Some(i) || tool.drag == Some(i);
        painter.circle_filled(to_canvas(c), if on { 6.0 } else { 4.5 }, qwc_style::GOLD);
        painter.circle_stroke(to_canvas(c), if on { 6.0 } else { 4.5 }, Stroke::new(1.0, Color32::BLACK));
    }
    // placing, dragging, taking out
    let mods = ctx.input(|i| i.modifiers);
    let snap = |p: Pos2, before: Option<[f32; 2]>| -> [f32; 2] {
        let mut t = to_texture(p);
        if let (true, Some(b)) = (mods.shift, before) {
            t = at_45(b, t);
        } else if !mods.alt {
            let marks = [0.0, 0.5, 1.0];
            let candidates = marks.iter().flat_map(|&u| marks.iter().map(move |&v| [u * w, v * h]));
            match candidates.min_by(|a, b| to_canvas(*a).distance(p).total_cmp(&to_canvas(*b).distance(p))) {
                Some(c) if to_canvas(c).distance(p) <= SNAP => t = c,
                _ => t = t.map(f32::round),
            }
        }
        [t[0].clamp(0.0, w), t[1].clamp(0.0, h)].map(|v| (v * 100.0).round() / 100.0)
    };
    if response.drag_started() {
        tool.drag = ctx.input(|i| i.pointer.press_origin()).and_then(|p| near(&crop.points, p));
    }
    if let (Some(i), true, Some(p)) = (tool.drag, response.dragged(), response.interact_pointer_pos()) {
        let before = (n > 1).then(|| crop.points[(i + n - 1) % n]);
        crop.points[i] = snap(p, before);
    }
    if response.drag_stopped() {
        tool.drag = None;
    }
    if response.clicked() {
        if let Some(p) = response.interact_pointer_pos().filter(|&p| near(&crop.points, p).is_none()) {
            let before = crop.points.last().copied();
            crop.points.push(snap(p, before));
        }
    }
    if response.secondary_clicked() {
        if let Some(i) = response.interact_pointer_pos().and_then(|p| near(&crop.points, p)) {
            crop.points.remove(i);
        }
    }
    response.hover_pos().map(to_texture)
}

/// `p` moved onto the nearest line through `from` at a multiple of 45 degrees.
fn at_45(from: [f32; 2], p: [f32; 2]) -> [f32; 2] {
    let d = [p[0] - from[0], p[1] - from[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if len == 0.0 {
        return p;
    }
    let angle = (d[1].atan2(d[0]) / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
    let (dx, dy) = (angle.cos(), angle.sin());
    let along = d[0] * dx + d[1] * dy;
    [from[0] + dx * along, from[1] + dy * along].map(f32::round)
}

/// The texture at `path` on the GPU, as the disc decoded it.
fn source(
    tool: &mut CropTool,
    s: &crate::screens::Session,
    ctx: &egui::Context,
    path: &str,
) -> Option<egui::TextureId> {
    if tool.source.as_ref().is_none_or(|(_, p)| p != path) {
        let shown = s.disc().shown(path)?;
        let p = &shown.pixels;
        let image = egui::ColorImage::from_rgba_unmultiplied([p.width as usize, p.height as usize], &p.levels[0]);
        tool.source = Some((ctx.load_texture("crop source", image, egui::TextureOptions::LINEAR), path.to_string()));
    }
    tool.source.as_ref().map(|(t, _)| t.id())
}

/// The cut, on the GPU: cut again when the crop changed.
fn preview(
    tool: &mut CropTool,
    s: &crate::screens::Session,
    ctx: &egui::Context,
    crop: &Crop,
) -> Option<egui::TextureId> {
    if tool.preview.as_ref().is_none_or(|(_, c)| c != crop) {
        let shown = s.disc().shown(&qwc_assets::vfs::normalize(&crop.texture))?;
        let cut = crop.apply(&shown.pixels, shown.size);
        let image = egui::ColorImage::from_rgba_unmultiplied([cut.width as usize, cut.height as usize], &cut.levels[0]);
        tool.preview = Some((ctx.load_texture("crop preview", image, egui::TextureOptions::LINEAR), crop.clone()));
    }
    tool.preview.as_ref().map(|(t, _)| t.id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_keeps_a_side_at_45_degrees() {
        assert_eq!(at_45([0.0, 0.0], [11.0, 9.0]), [10.0, 10.0]);
        assert_eq!(at_45([0.0, 0.0], [10.0, 1.0]), [10.0, 0.0]);
        assert_eq!(at_45([100.0, 0.0], [1.0, 99.0]), [1.0, 99.0]);
        assert_eq!(at_45([100.0, 0.0], [3.0, 97.0]), [3.0, 97.0]);
        assert_eq!(at_45([5.0, 5.0], [5.0, 5.0]), [5.0, 5.0]);
    }
}
