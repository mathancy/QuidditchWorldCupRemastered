//! The panels around the canvas: the hierarchy, the selection's details, the values, and the texture browser.

use std::collections::HashSet;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use qwc_hud::{Anchors, Driver, Edge, Element, Hint, Image, Kind, Number, Param, Property, ScaleRule, Value, Values};

use crate::app::{Editor, Pick};

// ---------------------------------------------------------------------------------------------------------------
// small painted icons (the default fonts have few symbols)

#[derive(Clone, Copy)]
enum Icon {
    Fold,
    Eye,
    Lock,
}

/// A 16-pixel icon button showing `on`, which it toggles when clicked.
fn icon(ui: &mut egui::Ui, kind: Icon, on: &mut bool, tip: &str) -> egui::Response {
    let (rect, mut r) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    let p = ui.painter();
    let c = rect.center();
    let col = if r.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
    match kind {
        Icon::Fold => {
            let pts = if *on {
                vec![c + Vec2::new(-4.0, -2.0), c + Vec2::new(4.0, -2.0), c + Vec2::new(0.0, 3.0)]
            } else {
                vec![c + Vec2::new(-2.0, -4.0), c + Vec2::new(3.0, 0.0), c + Vec2::new(-2.0, 4.0)]
            };
            p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
        }
        Icon::Eye => {
            let col = if *on { col } else { col.gamma_multiply(0.35) };
            let arc = |s: f32| {
                (0..=8).map(move |i| {
                    let t = i as f32 / 8.0;
                    c + Vec2::new(-6.5 + 13.0 * t, s * 4.0 * (t * std::f32::consts::PI).sin())
                })
            };
            let outline: Vec<Pos2> = arc(-1.0).chain(arc(1.0).rev()).collect();
            p.add(egui::Shape::closed_line(outline, Stroke::new(1.2, col)));
            if *on {
                p.circle_filled(c, 2.2, col);
            } else {
                p.line_segment([c + Vec2::new(-6.0, 5.0), c + Vec2::new(6.0, -5.0)], Stroke::new(1.2, col));
            }
        }
        Icon::Lock => {
            let col = if *on { qwc_style::GOLD } else { col.gamma_multiply(0.3) };
            let body = Rect::from_center_size(c + Vec2::new(0.0, 2.5), Vec2::new(10.0, 7.0));
            p.rect_filled(body, 1.0, col);
            let lift = if *on { 0.0 } else { -2.0 };
            let shackle: Vec<Pos2> = (0..=8)
                .map(|i| {
                    let a = std::f32::consts::PI * i as f32 / 8.0;
                    c + Vec2::new(-3.0 * a.cos(), -1.0 + lift - 3.5 * a.sin())
                })
                .collect();
            p.add(egui::Shape::line(shackle, Stroke::new(1.5, col)));
        }
    }
    r.on_hover_text(tip)
}

/// A mark for an element's kind.
fn kind_mark(ui: &mut egui::Ui, kind: &Kind) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
    let p = ui.painter();
    let col = ui.visuals().weak_text_color();
    let r = rect.shrink(2.0);
    match kind {
        Kind::Group => {
            p.rect_stroke(r, 1.0, Stroke::new(1.0, col), egui::StrokeKind::Inside);
            p.line_segment(
                [r.left_top() + Vec2::new(0.0, 3.0), r.right_top() + Vec2::new(0.0, 3.0)],
                Stroke::new(1.0, col),
            );
        }
        Kind::Image(_) => {
            p.rect_filled(r, 1.0, col.gamma_multiply(0.6));
            let pts = [r.left_bottom(), r.center() + Vec2::new(-1.0, 0.0), r.right_bottom()];
            p.add(egui::Shape::convex_polygon(pts.to_vec(), col, Stroke::NONE));
        }
        Kind::Number(_) => {
            p.text(r.center(), egui::Align2::CENTER_CENTER, "#", egui::FontId::monospace(12.0), col);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the hierarchy

pub fn hierarchy(ed: &mut Editor, ui: &mut egui::Ui) {
    ui.label(qwc_style::heading("Hierarchy", 20.0));
    let some = ed.selected.is_some();
    ui.horizontal_wrapped(|ui| {
        ui.menu_button("Add", |ui| add_menu(ed, ui));
        if ui.add_enabled(some, egui::Button::new("Duplicate")).clicked() {
            ed.duplicate(false);
        }
        if ui.add_enabled(some, egui::Button::new("Mirror")).on_hover_text(MIRROR_HELP).clicked() {
            ed.duplicate(true);
        }
        if ui.add_enabled(some, egui::Button::new("Delete")).clicked() {
            ed.delete();
        }
    });
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(some, egui::Button::new("Up")).on_hover_text("Earlier: drawn sooner in its layer").clicked() {
            ed.reorder(-1);
        }
        if ui.add_enabled(some, egui::Button::new("Down")).clicked() {
            ed.reorder(1);
        }
        if ui.add_enabled(some, egui::Button::new("Into group")).on_hover_text(INDENT_HELP).clicked() {
            ed.indent();
        }
        if ui.add_enabled(some, egui::Button::new("Out of group")).clicked() {
            ed.outdent();
        }
    });
    ui.separator();
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        let mut clicked = None;
        let mut path = Vec::new();
        rows(ui, &mut ed.scene.elements, &mut path, ed.selected.as_deref(), &mut ed.folded, &mut clicked);
        if let Some(p) = clicked {
            ed.selected = Some(p);
        }
    });
}

const MIRROR_HELP: &str = "A mirrored copy after it: its anchors, alignment and position across its parent mirrored, \
its scale across turned over (mirroring what it draws, though numbers still read left to right), and left and \
right swapped in the values it names (`power.left` becomes `power.right`)";

const INDENT_HELP: &str = "Into the group just above it (its position is then in the group's box)";

fn rows(
    ui: &mut egui::Ui,
    list: &mut [Element],
    path: &mut Vec<usize>,
    selected: Option<&[usize]>,
    folded: &mut HashSet<Vec<usize>>,
    clicked: &mut Option<Vec<usize>>,
) {
    for (i, e) in list.iter_mut().enumerate() {
        path.push(i);
        let mut open = !folded.contains(path.as_slice());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.add_space((path.len() - 1) as f32 * 14.0);
            if e.children.is_empty() {
                ui.add_space(16.0);
            } else if icon(ui, Icon::Fold, &mut open, "Fold").clicked() {
                if open {
                    folded.remove(path.as_slice());
                } else {
                    folded.insert(path.clone());
                }
            }
            let mut shown = !e.hidden;
            if icon(ui, Icon::Eye, &mut shown, "Shown in the editor (the game draws it anyway)").changed() {
                e.hidden = !shown;
            }
            icon(ui, Icon::Lock, &mut e.locked, "Locked: not picked or dragged on the canvas");
            kind_mark(ui, &e.kind);
            let text = egui::RichText::new(&e.name);
            let text = if e.hidden { text.weak() } else { text };
            if ui.selectable_label(selected == Some(path.as_slice()), text).clicked() {
                *clicked = Some(path.clone());
            }
            if !e.repeat.count.is_empty() {
                ui.weak(format!("x {}", e.repeat.count));
            }
            if !e.show_if.is_empty() {
                ui.weak(format!("if {}", e.show_if));
            }
            if !e.drivers.is_empty() {
                ui.weak(format!("~ {}", e.drivers.iter().map(|d| d.value.as_str()).collect::<Vec<_>>().join(", ")));
            }
        });
        if open && !e.children.is_empty() {
            rows(ui, &mut e.children, path, selected, folded, clicked);
        }
        path.pop();
    }
}

/// The size of what a new element goes into: the selected group, else the selection's parent, else the canvas.
fn new_parent(ed: &Editor) -> [f32; 2] {
    let Some(p) = &ed.selected else { return ed.frame.canvas };
    match ed.scene.get(p) {
        Some(e) if matches!(e.kind, Kind::Group) => {
            let b = e.local_box(ed.parent_size(p));
            [b[2] - b[0], b[3] - b[1]]
        }
        Some(_) => ed.parent_size(p),
        None => ed.frame.canvas,
    }
}

/// `e` at `size`, in the middle of what it goes into.
fn centred(ed: &Editor, mut e: Element, size: [f32; 2]) -> Element {
    let parent = new_parent(ed);
    e.size = size;
    e.position = [(parent[0] - size[0]) / 2.0, (parent[1] - size[1]) / 2.0].map(|v| v.round());
    e
}

pub fn add_menu(ed: &mut Editor, ui: &mut egui::Ui) {
    if ui.button("Group").on_hover_text("A box holding others").clicked() {
        let e = centred(ed, Element { name: "Group".into(), ..Default::default() }, [200.0, 100.0]);
        ed.add(e);
        ui.close();
    }
    if ui.button("Image...").on_hover_text("Pick one of the game's textures").clicked() {
        ed.pick = Some(Pick::Add);
        ui.close();
    }
    if ui.button("Number").on_hover_text("A value as a row of numerals").clicked() {
        let n = Number::default();
        let size = [n.step * (n.digits - 1) as f32 + n.glyph[0], n.glyph[1]];
        let e = Element { name: "Number".into(), kind: Kind::Number(n), layer: 0x36, ..Default::default() };
        let e = centred(ed, e, size);
        ed.add(e);
        ui.close();
    }
}

pub fn context_menu(ed: &mut Editor, ui: &mut egui::Ui) {
    ui.menu_button("Add", |ui| add_menu(ed, ui));
    let some = ed.selected.is_some();
    if ui.add_enabled(some, egui::Button::new("Duplicate")).clicked() {
        ed.duplicate(false);
        ui.close();
    }
    if ui.add_enabled(some, egui::Button::new("Mirrored copy")).clicked() {
        ed.duplicate(true);
        ui.close();
    }
    if ui.add_enabled(some, egui::Button::new("Delete")).clicked() {
        ed.delete();
        ui.close();
    }
    let parent = ed.selected.as_ref().filter(|p| p.len() > 1).map(|p| p[..p.len() - 1].to_vec());
    if ui.add_enabled(parent.is_some(), egui::Button::new("Select its group")).clicked() {
        ed.selected = parent;
        ui.close();
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the details

pub fn details(ed: &mut Editor, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| match ed.selected.clone() {
        Some(p) if ed.scene.get(&p).is_some() => element(ed, ui, &p),
        _ => scene_settings(ed, ui),
    });
}

fn scale_help(rule: ScaleRule) -> &'static str {
    match rule {
        ScaleRule::ShortestSide => {
            "A unit is the screen's shorter side over the design's (UMG's default): a 640 x 480 \
            design on a 1920 x 1080 screen is 2.25 pixels a unit, and the canvas 853 units across"
        }
        ScaleRule::LongestSide => "A unit is the screen's longer side over the design's",
        ScaleRule::Width => "A unit is the screen's width over the design's",
        ScaleRule::Height => "A unit is the screen's height over the design's",
        ScaleRule::Fit => "The largest scale at which the whole design fits on the screen",
        ScaleRule::Fill => "The smallest scale at which the design covers the screen",
        ScaleRule::Stretch => {
            "The design stretched over the screen, across and down apart, as the original drew \
            its 640 x 480 HUD (wide screens stretch the textures)"
        }
    }
}

fn scene_settings(ed: &mut Editor, ui: &mut egui::Ui) {
    ui.label(qwc_style::heading("Scene", 20.0));
    ui.label(format!("{}.ron in {}", ed.name, ed.dir_label()));
    ui.add_space(6.0);
    egui::Grid::new("scene").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Design size").on_hover_text("The canvas in units at a scale of 1");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut ed.scene.design[0]).range(1.0..=8192.0));
            ui.add(egui::DragValue::new(&mut ed.scene.design[1]).range(1.0..=8192.0));
        });
        ui.end_row();
        ui.label("Scale rule");
        egui::ComboBox::from_id_salt("scale rule").selected_text(ed.scene.scale.label()).show_ui(ui, |ui| {
            for r in ScaleRule::ALL {
                ui.selectable_value(&mut ed.scene.scale, r, r.label()).on_hover_text(scale_help(r));
            }
        });
        ui.end_row();
    });
    ui.weak(scale_help(ed.scene.scale));
    ui.add_space(8.0);
    let count = ed.frame.placed.len();
    ui.label(format!("{count} elements, {} values", ed.scene.params.len()));
    ui.weak("Select an element on the canvas or in the hierarchy to edit it.");
}

/// The names of the values, of the numbers among them, and of the colours.
fn names(ed: &Editor) -> (Vec<String>, Vec<String>, Vec<String>) {
    let all: Vec<String> = ed.scene.params.iter().map(|p| p.name.clone()).collect();
    let of = |f: fn(&Value) -> bool| ed.scene.params.iter().filter(|p| f(&p.value)).map(|p| p.name.clone()).collect();
    (all, of(|v| matches!(v, Value::Scalar(_) | Value::Flag(_))), of(|v| matches!(v, Value::Colour(_))))
}

/// A choice of `names` (or none, labelled `none`) for `value`.
fn value_combo(ui: &mut egui::Ui, id: &str, value: &mut String, names: &[String], none: &str) -> bool {
    let mut changed = false;
    let shown = if value.is_empty() { none.to_string() } else { value.clone() };
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(170.0).show_ui(ui, |ui| {
        if !none.is_empty() && ui.selectable_label(value.is_empty(), none).clicked() {
            value.clear();
            changed = true;
        }
        for n in names {
            if ui.selectable_label(value == n, n).clicked() {
                *value = n.clone();
                changed = true;
            }
        }
    });
    changed
}

/// The 4 x 4 anchor presets: left, middle, right, stretched across by top, middle, bottom, stretched down.
fn anchor_presets(ui: &mut egui::Ui, e: &mut Element, parent: [f32; 2]) {
    let spans = [(0.0, 0.0), (0.5, 0.5), (1.0, 1.0), (0.0, 1.0)];
    ui.label("Anchors").on_hover_text(
        "The presets keep the element where it is; Shift-click to also set its alignment and put it at the anchor",
    );
    egui::Grid::new("anchor presets").spacing([3.0, 3.0]).show(ui, |ui| {
        for (y0, y1) in spans {
            for (x0, x1) in spans {
                let to = Anchors { min: [x0, y0], max: [x1, y1] };
                let (rect, r) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
                let p = ui.painter();
                let on = e.anchors == to;
                let bg = if on {
                    ui.visuals().selection.bg_fill
                } else if r.hovered() {
                    ui.visuals().widgets.hovered.bg_fill
                } else {
                    ui.visuals().widgets.inactive.bg_fill
                };
                p.rect_filled(rect, 2.0, bg);
                let inner = rect.shrink(5.0);
                p.rect_stroke(inner, 0.0, Stroke::new(1.0, Color32::from_gray(140)), egui::StrokeKind::Inside);
                let at = |x: f32, y: f32| inner.min + inner.size() * Vec2::new(x, y);
                let mark = Rect::from_min_max(at(x0, y0), at(x1, y1)).expand(1.5);
                p.rect_filled(mark, 0.0, qwc_style::ACCENT);
                if r.clicked() {
                    e.set_anchors_keeping_box(to, parent);
                    if ui.input(|i| i.modifiers.shift) {
                        let align = [if x0 == x1 { x0 } else { 0.0 }, if y0 == y1 { y0 } else { 0.0 }];
                        e.alignment = align;
                        for axis in 0..2 {
                            if !to.stretches(axis) {
                                e.position[axis] = 0.0;
                            }
                        }
                    }
                }
            }
            ui.end_row();
        }
    });
}

fn pair(ui: &mut egui::Ui, v: &mut [f32; 2], speed: f64) {
    ui.add(egui::DragValue::new(&mut v[0]).speed(speed).max_decimals(4));
    ui.add(egui::DragValue::new(&mut v[1]).speed(speed).max_decimals(4));
}

fn element(ed: &mut Editor, ui: &mut egui::Ui, path: &[usize]) {
    let parent = ed.parent_size(path);
    let (all, numbers, colours) = names(ed);
    let ctx = ui.ctx().clone();
    let Editor { scene, values, textures, pick, .. } = ed;
    let Some(e) = scene.get_mut(path) else { return };
    ui.horizontal(|ui| {
        ui.label(qwc_style::heading(
            match e.kind {
                Kind::Group => "Group",
                Kind::Image(_) => "Image",
                Kind::Number(_) => "Number",
            },
            20.0,
        ));
        ui.text_edit_singleline(&mut e.name);
    });
    ui.horizontal(|ui| {
        ui.label("Kind");
        let kinds = [
            ("Group", Kind::Group),
            ("Image", Kind::Image(Image::default())),
            ("Number", Kind::Number(Number::default())),
        ];
        for (label, k) in kinds {
            let on = std::mem::discriminant(&e.kind) == std::mem::discriminant(&k);
            if ui.selectable_label(on, label).clicked() && !on {
                e.kind = k;
            }
        }
    });

    egui::CollapsingHeader::new("Slot").default_open(true).show(ui, |ui| {
        anchor_presets(ui, e, parent);
        let (x, y) = (e.anchors.stretches(0), e.anchors.stretches(1));
        egui::Grid::new("slot").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("Anchors min");
            ui.add(egui::DragValue::new(&mut e.anchors.min[0]).speed(0.005).range(0.0..=1.0));
            ui.add(egui::DragValue::new(&mut e.anchors.min[1]).speed(0.005).range(0.0..=1.0));
            ui.end_row();
            ui.label("Anchors max");
            ui.add(egui::DragValue::new(&mut e.anchors.max[0]).speed(0.005).range(0.0..=1.0));
            ui.add(egui::DragValue::new(&mut e.anchors.max[1]).speed(0.005).range(0.0..=1.0));
            ui.end_row();
            ui.label(match (x, y) {
                (false, false) => "Position",
                (true, false) => "Left / Position Y",
                (false, true) => "Position X / Top",
                (true, true) => "Left / Top",
            })
            .on_hover_text("From the anchor; where the anchors are apart, the inset from the first");
            pair(ui, &mut e.position, 0.5);
            ui.end_row();
            ui.label(match (x, y) {
                (false, false) => "Size",
                (true, false) => "Right / Size Y",
                (false, true) => "Size X / Bottom",
                (true, true) => "Right / Bottom",
            })
            .on_hover_text("Its size; where the anchors are apart, the inset from the second");
            pair(ui, &mut e.size, 0.5);
            ui.end_row();
            ui.label("Alignment").on_hover_text("The point of it at its position (0.5, 0.5 its middle)");
            pair(ui, &mut e.alignment, 0.01);
            ui.end_row();
            ui.label("Pivot").on_hover_text("The point of it it scales about");
            pair(ui, &mut e.pivot, 0.01);
            ui.end_row();
            ui.label("Scale").on_hover_text("Drawn scaled about its pivot, its children too; -1 across mirrors");
            pair(ui, &mut e.scale, 0.01);
            ui.end_row();
            ui.label("Layer").on_hover_text(
                "The overlay's material layer it is drawn in (the original's HUD uses 50 to 54); in the \
                 scene's order within a layer",
            );
            ui.add(egui::DragValue::new(&mut e.layer).range(0..=255));
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if ui.button("Mirror across").clicked() {
                e.scale[0] = -e.scale[0];
            }
            if ui.button("Mirror down").clicked() {
                e.scale[1] = -e.scale[1];
            }
            if ui.button("Centre the alignment").on_hover_text("Keeping it where it is").clicked() {
                e.set_alignment_keeping_box([0.5, 0.5], parent);
            }
        });
    });

    egui::CollapsingHeader::new("Appearance").default_open(true).show(ui, |ui| {
        egui::Grid::new("look").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("Colour").on_hover_text("Multiplies its texture (white leaves it as it is)");
            ui.color_edit_button_rgba_unmultiplied(&mut e.colour);
            ui.end_row();
            ui.label("Tint by").on_hover_text("A colour value multiplying the colour (a team's, say)");
            value_combo(ui, "tint", &mut e.tint, &colours, "(none)");
            ui.end_row();
            ui.label("Opacity").on_hover_text("Its children's too");
            ui.add(egui::Slider::new(&mut e.opacity, 0.0..=1.0));
            ui.end_row();
            ui.label("Shows").on_hover_text("While a value is true (a flag on, a number not 0, a text not empty)");
            let mut negated = e.show_if.starts_with('!');
            let mut name = e.show_if.trim_start_matches('!').to_string();
            ui.horizontal(|ui| {
                let a = value_combo(ui, "show if", &mut name, &all, "always");
                let b = ui.add_enabled(!name.is_empty(), egui::Checkbox::new(&mut negated, "when false")).changed();
                if a || b {
                    e.show_if = if name.is_empty() {
                        String::new()
                    } else {
                        format!("{}{name}", if negated { "!" } else { "" })
                    };
                }
            });
            ui.end_row();
        });
        ui.checkbox(&mut e.locked, "Locked on the canvas");
        ui.checkbox(&mut e.hidden, "Hidden in the editor (the game draws it)");
    });

    let repeated = !e.repeat.count.is_empty();
    egui::CollapsingHeader::new(if repeated { format!("Repeat (x {})", e.repeat.count) } else { "Repeat".into() })
        .default_open(repeated)
        .show(ui, |ui| {
            ui.weak(
                "Drawn once per count of a value, each copy a step on from the last. A # in its names (textures, \
                 values) is the copy's index: {button.#} is the value button.0 in the first copy, button.1 in the \
                 next.",
            );
            egui::Grid::new("repeat").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                ui.label("Count").on_hover_text("The value saying how many copies");
                value_combo(ui, "repeat count", &mut e.repeat.count, &numbers, "(not repeated)");
                ui.end_row();
                ui.label("At most");
                ui.add(egui::DragValue::new(&mut e.repeat.max).range(1..=64));
                ui.end_row();
                ui.label("Step").on_hover_text("From one copy to the next, in units");
                ui.horizontal(|ui| pair(ui, &mut e.repeat.step, 0.5));
                ui.end_row();
            });
        });

    let mut size_to = None;
    match &mut e.kind {
        Kind::Group => {}
        Kind::Image(img) => {
            egui::CollapsingHeader::new("Image").default_open(true).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Texture");
                    ui.add(egui::TextEdit::singleline(&mut img.texture).desired_width(200.0))
                        .on_hover_text(TEXTURE_HELP);
                    if ui.button("Pick").clicked() {
                        *pick = Some(Pick::Image(path.to_vec()));
                    }
                });
                // a repeated element's first copy
                let name = values.substitute(&img.texture.replace('#', "0"));
                let size = textures.get(&ctx, &name).map(|t| t.size);
                match size {
                    Some([w, h]) => ui.weak(format!("{} ({w} x {h})", qwc_hud::texture_path(&name))),
                    None => ui.colored_label(qwc_style::BAD, format!("No {}", qwc_hud::texture_path(&name))),
                };
                egui::Grid::new("uv").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                    ui.label("UV from").on_hover_text("The part of the texture drawn, 0 to 1");
                    ui.add(egui::DragValue::new(&mut img.uv[0]).speed(0.002).max_decimals(5));
                    ui.add(egui::DragValue::new(&mut img.uv[1]).speed(0.002).max_decimals(5));
                    ui.end_row();
                    ui.label("UV to");
                    ui.add(egui::DragValue::new(&mut img.uv[2]).speed(0.002).max_decimals(5));
                    ui.add(egui::DragValue::new(&mut img.uv[3]).speed(0.002).max_decimals(5));
                    ui.end_row();
                    if let Some([w, h]) = size {
                        ui.label("In pixels");
                        ui.weak(format!("{:.1}, {:.1}", img.uv[0] * w as f32, img.uv[1] * h as f32));
                        ui.weak(format!("{:.1}, {:.1}", img.uv[2] * w as f32, img.uv[3] * h as f32));
                        ui.end_row();
                    }
                    ui.label("Fill").on_hover_text("How much of it shows, from the edge (a driver on Fill moves it)");
                    ui.add(egui::Slider::new(&mut img.fill, 0.0..=1.0));
                    egui::ComboBox::from_id_salt("fill from")
                        .selected_text(format!("from {:?}", img.fill_from))
                        .show_ui(ui, |ui| {
                            for edge in Edge::ALL {
                                ui.selectable_value(&mut img.fill_from, edge, format!("{edge:?}"));
                            }
                        });
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut img.crop, "Crop")
                        .on_hover_text("The part shown shows that part of the texture (else all of it, squeezed)");
                    ui.checkbox(&mut img.flip[0], "Flip across");
                    ui.checkbox(&mut img.flip[1], "Flip down");
                });
                ui.horizontal(|ui| {
                    if ui.button("Whole texture").clicked() {
                        img.uv = [0.0, 0.0, 1.0, 1.0];
                    }
                    if let Some([w, h]) = size {
                        if ui
                            .button("Size to its pixels")
                            .on_hover_text("A unit for each of the part's pixels")
                            .clicked()
                        {
                            size_to = Some([
                                ((img.uv[2] - img.uv[0]) * w as f32).abs(),
                                ((img.uv[3] - img.uv[1]) * h as f32).abs(),
                            ]);
                        }
                    }
                });
            });
        }
        Kind::Number(n) => {
            egui::CollapsingHeader::new("Number").default_open(true).show(ui, |ui| {
                egui::Grid::new("number").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                    ui.label("Value");
                    value_combo(ui, "number value", &mut n.value, &numbers, "");
                    ui.end_row();
                    ui.label("Digits").on_hover_text("Zeros fill the front; it holds at the largest that fits");
                    ui.add(egui::DragValue::new(&mut n.digits).range(1..=9));
                    ui.end_row();
                    ui.label("Numerals").on_hover_text("Their textures' names, {d} the digit");
                    ui.text_edit_singleline(&mut n.numerals);
                    ui.end_row();
                    ui.label("Numeral size");
                    ui.horizontal(|ui| pair(ui, &mut n.glyph, 0.25));
                    ui.end_row();
                    ui.label("Step").on_hover_text("From one numeral to the next");
                    ui.add(egui::DragValue::new(&mut n.step).speed(0.25));
                    ui.end_row();
                    ui.label("Justify").on_hover_text("0 at the box's left, 0.5 its middle, 1 its right");
                    ui.add(egui::Slider::new(&mut n.justify, 0.0..=1.0));
                    ui.end_row();
                });
                if ui.button("Box to the numerals").clicked() {
                    size_to = Some([n.step * (n.digits.max(1) - 1) as f32 + n.glyph[0], n.glyph[1]]);
                }
            });
        }
    }
    if let Some(s) = size_to {
        let b = e.local_box(parent);
        e.set_local_box([b[0], b[1], b[0] + s[0], b[1] + s[1]], parent);
    }

    egui::CollapsingHeader::new(format!("Drivers ({})", e.drivers.len())).default_open(true).show(ui, |ui| {
        ui.weak("A value moving a property: its keys (value, amount) are added to it, linear between them.");
        let mut remove = None;
        for (i, d) in e.drivers.iter_mut().enumerate() {
            ui.push_id(i, |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    driver(ui, d, values, &numbers);
                    if ui.small_button("Remove the driver").clicked() {
                        remove = Some(i);
                    }
                });
            });
        }
        if let Some(i) = remove {
            e.drivers.remove(i);
        }
        if ui.button("Add a driver").clicked() {
            let value = numbers.first().cloned().unwrap_or_default();
            e.drivers.push(Driver { value, ..Default::default() });
        }
    });
}

const TEXTURE_HELP: &str = "A HUD texture's name (root/HUD/<name>.fsh), or a path in the game data without its .fsh \
(teams/England/QWC_GUI_SnitchBar); {name} puts in a value's text, as in teams/{team.left}/QWC_GUI_SnitchBar";

fn driver(ui: &mut egui::Ui, d: &mut Driver, values: &Values, numbers: &[String]) {
    ui.horizontal(|ui| {
        value_combo(ui, "driver value", &mut d.value, numbers, "");
        ui.label("moves");
        egui::ComboBox::from_id_salt("property").selected_text(d.property.label()).show_ui(ui, |ui| {
            for p in Property::ALL {
                ui.selectable_value(&mut d.property, p, p.label());
            }
        });
    });
    let now = values.number(&d.value);
    ui.weak(format!("now {} = {now:.3}: {:+.3}", d.value, d.at(now)));
    let mut remove = None;
    egui::Grid::new("keys").num_columns(3).spacing([8.0, 2.0]).show(ui, |ui| {
        ui.weak("value");
        ui.weak("amount");
        ui.end_row();
        for (i, k) in d.keys.iter_mut().enumerate() {
            ui.add(egui::DragValue::new(&mut k[0]).speed(0.01).max_decimals(4));
            ui.add(egui::DragValue::new(&mut k[1]).speed(0.25).max_decimals(4));
            if ui.small_button("x").on_hover_text("Remove the key").clicked() {
                remove = Some(i);
            }
            ui.end_row();
        }
    });
    if let Some(i) = remove {
        d.keys.remove(i);
    }
    ui.horizontal(|ui| {
        if ui.small_button(format!("Key at {now:.3}")).on_hover_text("A key at the value's current value").clicked() {
            let amount = d.at(now);
            if !d.keys.iter().any(|k| (k[0] - now).abs() < 1e-3) {
                d.keys.push([now, amount]);
            }
            d.keys.sort_by(|a, b| a[0].total_cmp(&b[0]));
        }
        if ui.small_button("Sort").clicked() {
            d.keys.sort_by(|a, b| a[0].total_cmp(&b[0]));
        }
    });
}

// ---------------------------------------------------------------------------------------------------------------
// the values

pub fn values(ed: &mut Editor, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(qwc_style::heading("Values", 20.0));
        ui.weak("what the game provides: slide them to see what they move");
        ui.separator();
        if ui.button("Reset").on_hover_text("Back to the scene's own values").clicked() {
            ed.values = Values::defaults(&ed.scene);
            ed.sweeping.clear();
        }
        if ui.button("Keep as the scene's").on_hover_text("These become the scene's own values (its preview)").clicked()
        {
            for p in &mut ed.scene.params {
                if let Some(v) = ed.values.get(&p.name) {
                    p.value = v.clone();
                }
            }
        }
        let sweepable: Vec<String> =
            ed.scene.params.iter().filter(|p| matches!(p.value, Value::Scalar(_))).map(|p| p.name.clone()).collect();
        let all = !sweepable.is_empty() && sweepable.iter().all(|n| ed.sweeping.contains_key(n));
        if ui.selectable_label(all, "Sweep all numbers").clicked() {
            let now = ui.input(|i| i.time);
            if all {
                ed.sweeping.clear();
            } else {
                ed.sweeping = sweepable.into_iter().map(|n| (n, now)).collect();
            }
        }
        ui.toggle_value(&mut ed.edit_values, "Edit the list");
    });
    ui.separator();
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        if ed.edit_values {
            definitions(ed, ui);
        } else {
            controls(ed, ui);
        }
    });
}

fn controls(ed: &mut Editor, ui: &mut egui::Ui) {
    let params = ed.scene.params.clone();
    let now = ui.input(|i| i.time);
    let columns = ((ui.available_width() / 430.0).floor() as usize).clamp(1, 4);
    egui::Grid::new("values").num_columns(columns * 3).spacing([8.0, 4.0]).show(ui, |ui| {
        for (i, p) in params.iter().enumerate() {
            let label = ui.label(&p.name);
            if !p.note.is_empty() {
                label.on_hover_text(&p.note);
            }
            ui.push_id(&p.name, |ui| value_control(ed, ui, p));
            if matches!(p.value, Value::Scalar(_) | Value::Flag(_)) {
                let on = ed.sweeping.contains_key(&p.name);
                let r = ui
                    .selectable_label(on, if on { "stop" } else { "sweep" })
                    .on_hover_text("To and fro over its range");
                if r.clicked() {
                    if on {
                        ed.sweeping.remove(&p.name);
                    } else {
                        ed.sweeping.insert(p.name.clone(), now);
                    }
                }
            } else {
                ui.label("");
            }
            if i % columns == columns - 1 {
                ui.end_row();
            }
        }
    });
}

fn value_control(ed: &mut Editor, ui: &mut egui::Ui, p: &Param) {
    let name = &p.name;
    match ed.values.get(name).cloned().unwrap_or_else(|| p.value.clone()) {
        Value::Scalar(mut v) => {
            let mut s = egui::Slider::new(&mut v, p.range[0]..=p.range[1]);
            if p.range[1] - p.range[0] > 10.0 {
                s = s.step_by(1.0);
            }
            if ui.add(s).changed() {
                ed.values.set(name, Value::Scalar(v));
                ed.sweeping.remove(name);
            }
        }
        Value::Flag(mut b) => {
            if ui.checkbox(&mut b, "").changed() {
                ed.values.set(name, Value::Flag(b));
                ed.sweeping.remove(name);
            }
        }
        Value::Colour(mut c) => {
            if let Hint::TeamColour(team) = &p.hint {
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(false, |ui| ui.color_edit_button_rgba_unmultiplied(&mut c));
                    ui.weak(format!("{team}'s"));
                });
            } else if ui.color_edit_button_rgba_unmultiplied(&mut c).changed() {
                ed.values.set(name, Value::Colour(c));
            }
        }
        Value::Text(mut t) => match p.hint {
            Hint::Team => {
                egui::ComboBox::from_id_salt("team").selected_text(&t).width(150.0).show_ui(ui, |ui| {
                    for team in &ed.textures.teams {
                        if ui.selectable_label(&t == team, team).clicked() {
                            ed.values.set(name, Value::Text(team.clone()));
                        }
                    }
                });
            }
            _ => {
                ui.horizontal(|ui| {
                    if ui.add(egui::TextEdit::singleline(&mut t).desired_width(170.0)).changed() {
                        ed.values.set(name, Value::Text(t.clone()));
                    }
                    if p.hint == Hint::Texture && ui.small_button("Pick").clicked() {
                        ed.pick = Some(Pick::Value(name.clone()));
                    }
                });
            }
        },
    }
}

fn definitions(ed: &mut Editor, ui: &mut egui::Ui) {
    ui.weak(
        "The values the scene reads, as the game must provide them. Renaming one doesn't rename it where it is used.",
    );
    let texts: Vec<String> =
        ed.scene.params.iter().filter(|p| matches!(p.value, Value::Text(_))).map(|p| p.name.clone()).collect();
    let mut remove = None;
    egui::Grid::new("definitions").num_columns(7).spacing([8.0, 4.0]).striped(true).show(ui, |ui| {
        for h in ["name", "kind", "value", "range", "for", "note", ""] {
            ui.weak(h);
        }
        ui.end_row();
        for (i, p) in ed.scene.params.iter_mut().enumerate() {
            ui.push_id(i, |ui| {
                ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(140.0));
            });
            let kinds = [
                ("Number", Value::Scalar(0.0)),
                ("Flag", Value::Flag(false)),
                ("Colour", Value::Colour([1.0; 4])),
                ("Text", Value::Text(String::new())),
            ];
            let current = kinds.iter().find(|(_, v)| std::mem::discriminant(v) == std::mem::discriminant(&p.value));
            egui::ComboBox::from_id_salt(("kind", i)).selected_text(current.map_or("", |k| k.0)).width(80.0).show_ui(
                ui,
                |ui| {
                    for (label, v) in &kinds {
                        if ui.selectable_label(current.is_some_and(|c| c.0 == *label), *label).clicked() {
                            p.value = v.clone();
                        }
                    }
                },
            );
            ui.push_id(("value", i), |ui| match &mut p.value {
                Value::Scalar(v) => {
                    ui.add(egui::DragValue::new(v).speed(0.01));
                }
                Value::Flag(b) => {
                    ui.checkbox(b, "");
                }
                Value::Colour(c) => {
                    ui.color_edit_button_rgba_unmultiplied(c);
                }
                Value::Text(t) => {
                    ui.add(egui::TextEdit::singleline(t).desired_width(140.0));
                }
            });
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut p.range[0]).speed(0.1));
                ui.add(egui::DragValue::new(&mut p.range[1]).speed(0.1));
            });
            let hint = match &p.hint {
                Hint::None => "-".to_string(),
                Hint::Team => "a team".to_string(),
                Hint::TeamColour(t) => format!("{t}'s colour"),
                Hint::Texture => "a texture".to_string(),
            };
            egui::ComboBox::from_id_salt(("hint", i)).selected_text(hint).width(130.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut p.hint, Hint::None, "-");
                ui.selectable_value(&mut p.hint, Hint::Team, "a team");
                ui.selectable_value(&mut p.hint, Hint::Texture, "a texture");
                for t in &texts {
                    ui.selectable_value(&mut p.hint, Hint::TeamColour(t.clone()), format!("{t}'s colour"));
                }
            });
            ui.push_id(("note", i), |ui| {
                ui.add(egui::TextEdit::singleline(&mut p.note).desired_width(260.0));
            });
            if ui.small_button("x").on_hover_text("Remove the value").clicked() {
                remove = Some(i);
            }
            ui.end_row();
        }
    });
    if let Some(i) = remove {
        ed.scene.params.remove(i);
    }
    if ui.button("Add a value").clicked() {
        let n = ed.scene.params.len();
        ed.scene.params.push(Param { name: format!("value{n}"), ..Default::default() });
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the texture browser

pub fn browser(ed: &mut Editor, ctx: &egui::Context) {
    let Some(pick) = ed.pick.clone() else { return };
    let mut open = true;
    let mut chosen = None;
    egui::Window::new("Textures").open(&mut open).default_size([600.0, 620.0]).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Search");
            ui.text_edit_singleline(&mut ed.filter).request_focus();
            ui.weak(match &pick {
                Pick::Add => "a new image".to_string(),
                Pick::Image(_) => "for the image".to_string(),
                Pick::Value(n) => format!("for {n}"),
            });
        });
        ui.weak("root/HUD's textures; type any other path in the texture's box.");
        let filter = ed.filter.to_ascii_lowercase();
        let names: Vec<String> =
            ed.textures.hud.iter().filter(|n| n.to_ascii_lowercase().contains(&filter)).cloned().collect();
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for name in names {
                    if tile(ui, ed, ctx, &name) {
                        chosen = Some(name);
                    }
                }
            });
        });
    });
    match chosen {
        Some(name) => {
            ed.pick = None;
            match pick {
                Pick::Image(path) => {
                    if let Some(Kind::Image(img)) = ed.scene.get_mut(&path).map(|e| &mut e.kind) {
                        img.texture = name;
                    }
                }
                Pick::Value(v) => ed.values.set(&v, Value::Text(name)),
                Pick::Add => {
                    let size = ed.textures.get(ctx, &name).map_or([64.0, 64.0], |t| t.size.map(|v| v as f32));
                    let kind = Kind::Image(Image { texture: name.clone(), ..Default::default() });
                    let e = centred(ed, Element { name, kind, ..Default::default() }, size);
                    ed.add(e);
                }
            }
        }
        None if !open => ed.pick = None,
        None => {}
    }
}

/// A texture's tile in the browser; whether it was clicked.
fn tile(ui: &mut egui::Ui, ed: &mut Editor, ctx: &egui::Context, name: &str) -> bool {
    let (rect, r) = ui.allocate_exact_size(Vec2::new(100.0, 112.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if r.hovered() {
            p.rect_filled(rect, 3.0, ui.visuals().widgets.hovered.bg_fill);
        }
        let well = Rect::from_min_size(rect.min + Vec2::new(6.0, 4.0), Vec2::new(88.0, 84.0));
        p.rect_filled(well, 0.0, qwc_style::WELL);
        if let Some(t) = ed.textures.get(ctx, name) {
            let [w, h] = t.size.map(|v| v as f32);
            let k = (well.width() / w).min(well.height() / h);
            let shown = Rect::from_center_size(well.center(), Vec2::new(w, h) * k);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            p.image(t.handle.id(), shown, uv, Color32::WHITE);
        }
        let short: String =
            if name.chars().count() > 16 { name.chars().take(15).chain(['~']).collect() } else { name.into() };
        let font = egui::FontId::proportional(11.0);
        p.text(
            rect.center_bottom() - Vec2::new(0.0, 4.0),
            egui::Align2::CENTER_BOTTOM,
            short,
            font,
            ui.visuals().text_color(),
        );
    }
    r.on_hover_text(name).clicked()
}
