//! The launcher's look: the remaster's (`qwc_style`), roomy.

pub use qwc_style::*;

/// The fonts, from the shipped `remastered/fonts/` under `root`, and the colours and spacing.
pub fn install(ctx: &eframe::egui::Context, root: &std::path::Path) {
    qwc_style::install(ctx, root, Density::Roomy);
}
