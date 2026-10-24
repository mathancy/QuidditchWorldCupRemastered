//! The remaster's look for its egui programs, the launcher and QWC Studio: its own (no game art). Dark wood and
//! leather with gold inlay, navy for the buttons and the sky, parchment for the notes; the shipped fonts (Cinzel's
//! capitals for headings, buttons and tabs, Cardo for the text; egui's own when they are not there) and the
//! project's logo. The launcher takes it roomy, the Studio compact ([`Density`]); [`ornate`] paints the frames,
//! glows and gradients, and draws the art a pack of pictures supplies ([`ornate::Art`]) or a stand-in without it.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, RichText, TextStyle};

pub mod ornate;

/// The deepest wood, behind everything.
pub const BACKGROUND: Color32 = Color32::from_rgb(0x15, 0x10, 0x0b);
/// The panels and bars: dark wood.
pub const PANEL: Color32 = Color32::from_rgb(0x1f, 0x18, 0x12);
/// A card on the wood: leather.
pub const CARD: Color32 = Color32::from_rgb(0x2a, 0x21, 0x19);
/// The deepest fill: text fields, wells, canvases.
pub const WELL: Color32 = Color32::from_rgb(0x0e, 0x0a, 0x07);
pub const GOLD: Color32 = Color32::from_rgb(0xd4, 0xa6, 0x4a);
/// The gold's shadow: outer outlines, idle borders.
pub const GOLD_DEEP: Color32 = Color32::from_rgb(0x8c, 0x6a, 0x2a);
pub const GOLD_LIGHT: Color32 = Color32::from_rgb(0xf6, 0xe3, 0xb0);
/// The buttons' and the banner's navy, and its lighter hover.
pub const NAVY: Color32 = Color32::from_rgb(0x1b, 0x27, 0x40);
pub const NAVY_LIGHT: Color32 = Color32::from_rgb(0x28, 0x3a, 0x5e);
/// Notes on parchment, and the ink on it.
pub const PARCHMENT: Color32 = Color32::from_rgb(0xe8, 0xdb, 0xbd);
pub const INK: Color32 = Color32::from_rgb(0x2e, 0x24, 0x18);
pub const INK_DIM: Color32 = Color32::from_rgb(0x6b, 0x5a, 0x44);
/// The one button to press: the pitch's green.
pub const GREEN: Color32 = Color32::from_rgb(0x2f, 0x7d, 0x3a);
pub const GREEN_DEEP: Color32 = Color32::from_rgb(0x17, 0x47, 0x20);
pub const TEXT: Color32 = Color32::from_rgb(0xef, 0xe4, 0xcc);
pub const DIM: Color32 = Color32::from_rgb(0xa8, 0x9a, 0x82);
pub const GOOD: Color32 = Color32::from_rgb(0x86, 0xc9, 0x7f);
pub const WARN: Color32 = Color32::from_rgb(0xe8, 0xbb, 0x5c);
pub const BAD: Color32 = Color32::from_rgb(0xe3, 0x6c, 0x5c);
/// The editors' second marking colour beside the gold (anchors, hovers): a cool blue that reads on the wood.
pub const ACCENT: Color32 = Color32::from_rgb(0x7f, 0xb8, 0xf0);

/// How much room the widgets take: the launcher's pages are roomy, the Studio's panels compact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    Roomy,
    Compact,
}

/// The headings' family: Cinzel Bold when it was found.
pub fn heading_family() -> FontFamily {
    FontFamily::Name("heading".into())
}

/// The titles' family: Cinzel Regular, for large spaced capitals.
pub fn title_family() -> FontFamily {
    FontFamily::Name("title".into())
}

/// A heading in the gold, `size` points.
pub fn heading(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).font(FontId::new(size, heading_family())).color(GOLD)
}

/// The folder holding the shipped files (`remastered/fonts.txt`): the working folder or the nearest above it that
/// has them, else the program's folder or the nearest above it, else the working folder.
pub fn find_root() -> PathBuf {
    let has = |d: &Path| d.join("remastered").join("fonts.txt").is_file();
    let cwd = std::env::current_dir().unwrap_or_default();
    let exe = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    let found = cwd.ancestors().find(|d| has(d)).map(Path::to_path_buf);
    found.or_else(|| exe.as_deref()?.ancestors().find(|d| has(d)).map(Path::to_path_buf)).unwrap_or(cwd)
}

/// The fonts, from the shipped `remastered/fonts/` under `root`, and the colours and spacing for `density`.
pub fn install(ctx: &egui::Context, root: &Path, density: Density) {
    let mut fonts = FontDefinitions::default();
    let mut load = |name: &str, file: &str| {
        let bytes = std::fs::read(root.join("remastered/fonts").join(file)).ok()?;
        fonts.font_data.insert(name.into(), std::sync::Arc::new(FontData::from_owned(bytes)));
        Some(name.to_string())
    };
    let cinzel_bold = load("cinzel-bold", "Cinzel-Bold.ttf");
    let cinzel = load("cinzel", "Cinzel-Regular.ttf");
    let cardo = load("cardo", "Cardo-Regular.ttf");
    if let Some(c) = cardo {
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, c);
    }
    // the capitals fall back to the text's fonts, Cardo first (it has the arrows and shapes Cinzel lacks)
    let text = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for (family, first) in [(heading_family(), cinzel_bold), (title_family(), cinzel)] {
        let mut list = text.clone();
        if let Some(f) = first {
            list.insert(0, f);
        }
        fonts.families.insert(family, list);
    }
    ctx.set_fonts(fonts);

    // (body, button, heading, small, monospace) points; item spacing, button padding, slider width
    let (sizes, item, button, slider) = match density {
        Density::Roomy => ([17.0, 16.0, 30.0, 14.0, 14.0], (10.0, 9.0), (16.0, 7.0), 280.0),
        Density::Compact => ([15.0, 13.5, 21.0, 12.5, 13.0], (8.0, 5.0), (10.0, 4.0), 180.0),
    };
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|s| {
        s.text_styles.insert(TextStyle::Body, FontId::proportional(sizes[0]));
        s.text_styles.insert(TextStyle::Button, FontId::new(sizes[1], heading_family()));
        s.text_styles.insert(TextStyle::Heading, FontId::new(sizes[2], heading_family()));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(sizes[3]));
        s.text_styles.insert(TextStyle::Monospace, FontId::monospace(sizes[4]));
        s.spacing.item_spacing = egui::vec2(item.0, item.1);
        s.spacing.button_padding = egui::vec2(button.0, button.1);
        s.spacing.slider_width = slider;
        if density == Density::Roomy {
            s.spacing.icon_width = 22.0;
            s.spacing.icon_width_inner = 13.0;
        }
        let v = &mut s.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = if density == Density::Roomy { BACKGROUND } else { PANEL };
        v.window_fill = PANEL;
        v.window_stroke = egui::Stroke::new(1.0, GOLD_DEEP);
        v.faint_bg_color = CARD.gamma_multiply(0.7);
        v.extreme_bg_color = WELL;
        v.code_bg_color = WELL;
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = GOLD.gamma_multiply(0.45);
        v.selection.stroke = egui::Stroke::new(1.0, GOLD_LIGHT);
        v.hyperlink_color = GOLD_LIGHT;
        v.warn_fg_color = WARN;
        v.error_fg_color = BAD;
        let w = &mut v.widgets;
        w.noninteractive.bg_stroke = egui::Stroke::new(1.0, GOLD_DEEP.gamma_multiply(0.7));
        // the buttons navy, edged in the gold's shadow, lit gold when the pointer is on them
        w.inactive.weak_bg_fill = NAVY;
        w.inactive.bg_fill = NAVY;
        w.inactive.bg_stroke = egui::Stroke::new(1.0, GOLD_DEEP);
        w.hovered.weak_bg_fill = NAVY_LIGHT;
        w.hovered.bg_fill = NAVY_LIGHT;
        w.hovered.bg_stroke = egui::Stroke::new(1.0, GOLD);
        w.active.weak_bg_fill = Color32::from_rgb(0x34, 0x4a, 0x76);
        w.active.bg_fill = Color32::from_rgb(0x34, 0x4a, 0x76);
        w.active.bg_stroke = egui::Stroke::new(1.0, GOLD_LIGHT);
        w.open.weak_bg_fill = NAVY_LIGHT;
        for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.noninteractive, &mut w.open] {
            state.corner_radius = egui::CornerRadius::same(4);
        }
    });
}

/// A gold button for the one thing to do next.
pub fn primary(text: &str, size: f32) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).font(FontId::new(size, heading_family())).color(BACKGROUND))
        .fill(GOLD)
        .corner_radius(6)
}

/// The art's icon `name` (`icon_...`), `size` points across and tinted `tint`, for a button; none without the art.
fn icon_image(art: &ornate::Art, name: &str, size: f32, tint: Color32) -> Option<egui::Image<'static>> {
    art.get(name).map(|t| egui::Image::new((t.id(), egui::vec2(size, size))).tint(tint))
}

/// [`primary`] with the art's icon `icon` before its text (the text alone without the art).
pub fn primary_with(art: &ornate::Art, icon: &str, text: &str, size: f32) -> egui::Button<'static> {
    let label = RichText::new(text).font(FontId::new(size, heading_family())).color(BACKGROUND);
    match icon_image(art, icon, size, BACKGROUND) {
        Some(image) => egui::Button::image_and_text(image, label),
        None => egui::Button::new(label),
    }
    .fill(GOLD)
    .corner_radius(6)
}

/// A button with the art's icon `icon` before `text` (the text alone without the art).
pub fn button_with(art: &ornate::Art, icon: &str, text: &str) -> egui::Button<'static> {
    match icon_image(art, icon, 16.0, TEXT) {
        Some(image) => egui::Button::image_and_text(image, text),
        None => egui::Button::new(text),
    }
}

/// `combo` with the art's arrows (`icon_down`, `icon_up` while open) for its own, in the buttons' text colour; as
/// it is without them.
pub fn dropdown<'a>(art: &ornate::Art, combo: egui::ComboBox<'a>) -> egui::ComboBox<'a> {
    let (Some(down), Some(up)) = (art.get("icon_down").cloned(), art.get("icon_up").cloned()) else { return combo };
    combo.icon(move |ui, rect, _visuals, open| {
        let t = if open { &up } else { &down };
        let side = rect.width().min(rect.height()) * 0.6;
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        ui.painter().image(t.id(), egui::Rect::from_center_size(rect.center(), egui::vec2(side, side)), uv, TEXT);
    })
}

/// A button showing only the art's icon `icon`, `label` its name for screen readers (its text without the art),
/// `hint` its hover text.
pub fn icon_button(
    ui: &mut egui::Ui,
    art: &ornate::Art,
    icon: &str,
    label: &str,
    hint: &str,
    enabled: bool,
) -> egui::Response {
    let Some(image) = icon_image(art, icon, 18.0, TEXT) else {
        return ui.add_enabled(enabled, egui::Button::new(label)).on_hover_text(hint);
    };
    let r = ui.add_enabled(enabled, egui::Button::image(image)).on_hover_text(hint);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    r
}

/// A tab of a row of them (the Studio's modes and tabs): the gold fill when `selected`. Its response clicks as a
/// button's.
pub fn tab(ui: &mut egui::Ui, selected: bool, text: &str, size: f32) -> egui::Response {
    let label = RichText::new(text).font(FontId::new(size, heading_family()));
    let button = if selected {
        egui::Button::new(label.color(BACKGROUND)).fill(GOLD)
    } else {
        egui::Button::new(label.color(GOLD_LIGHT)).fill(Color32::TRANSPARENT).stroke(egui::Stroke::NONE)
    };
    ui.add(button.corner_radius(4).selected(selected))
}

/// A card: leather, edged in the gold's shadow, padded.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(egui::Stroke::new(1.0, GOLD_DEEP))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(18, 14))
}

/// A tile of a page (a pack, a group of settings, a save): a [`card`] framed by the art's simple corner and line
/// (`ornate::Corner::Simple`), or the plain card without them.
pub fn tile<R>(ui: &mut egui::Ui, art: &ornate::Art, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    let framed = ornate::has_frame(art, ornate::Corner::Simple);
    // framed, the card leaves room inside for the corners' scrolls
    // and its leather is cut at the corners as the frame is, painted behind it once its size is known
    let frame = if framed {
        card().fill(Color32::TRANSPARENT).stroke(egui::Stroke::NONE).inner_margin(egui::Margin::symmetric(26, 20))
    } else {
        card()
    };
    let behind = ui.painter().add(egui::Shape::Noop);
    let r = frame.show(ui, add);
    if framed {
        let rect = r.response.rect;
        let inside = ornate::Inside::frame(rect, art, 30.0, ornate::Corner::Simple);
        ui.painter().set(behind, egui::Shape::convex_polygon(inside.points().to_vec(), CARD, egui::Stroke::NONE));
        ornate::outline_with(ui.painter(), rect, 6.0, art, 30.0, ornate::Corner::Simple);
    }
    r
}

/// A bar across the top or bottom of a window: the panel colour, a little padding.
pub fn bar() -> egui::Frame {
    egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(10, 6))
}

/// The logo's pixels: RGBA rows, its width and height.
fn logo_rgba() -> Option<(Vec<u8>, u32, u32)> {
    decode_png(include_bytes!("../assets/qwc_logo.png"))
}

/// A PNG's pixels as RGBA rows, with its width and height (RGB and grey PNGs given full alpha).
pub fn decode_png(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let px = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.as_chunks::<3>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.as_chunks::<2>().0.iter().flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((rgba, info.width, info.height))
}

/// The project's logo (`assets/qwc_logo.png`, the user's own art).
pub fn logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let (rgba, w, h) = logo_rgba()?;
    let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
    Some(ctx.load_texture("logo", image, egui::TextureOptions::LINEAR))
}

/// The logo as a window's icon.
pub fn icon() -> Option<egui::IconData> {
    let (rgba, width, height) = logo_rgba()?;
    Some(egui::IconData { rgba, width, height })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_logo_reads() {
        let icon = super::icon().unwrap();
        assert_eq!((icon.width, icon.height, icon.rgba.len()), (512, 512, 512 * 512 * 4));
    }
}
