//! The game's textures, uploaded to egui on first use, and the lists the editor offers: the HUD's textures and the
//! teams' folders.

use std::collections::HashMap;

use eframe::egui;
use qwc_assets::Vfs;

pub struct Tex {
    pub handle: egui::TextureHandle,
    pub size: [u32; 2],
}

pub struct Textures {
    vfs: &'static Vfs,
    loaded: HashMap<String, Option<Tex>>,
    colours: HashMap<String, Option<[f32; 3]>>,
    /// The HUD's textures (`root/HUD`) by name.
    pub hud: Vec<String>,
    /// The teams' folders (`teams/<folder>`, those with a bar).
    pub teams: Vec<String>,
}

impl Textures {
    pub fn new(vfs: &'static Vfs) -> Textures {
        let mut hud: Vec<String> = vfs
            .list_dir("root/HUD")
            .filter_map(|p| {
                let file = p.rsplit('/').next()?;
                let (stem, ext) = file.rsplit_once('.')?;
                ext.eq_ignore_ascii_case("fsh").then(|| stem.to_string())
            })
            .collect();
        hud.sort_by_key(|n| n.to_ascii_lowercase());
        let mut teams: Vec<String> = vfs
            .paths()
            .filter_map(|p| match p.split('/').collect::<Vec<_>>()[..] {
                [t, folder, bar]
                    if t.eq_ignore_ascii_case("teams") && bar.eq_ignore_ascii_case("QWC_GUI_SnitchBar.fsh") =>
                {
                    Some(folder.to_string())
                }
                _ => None,
            })
            .collect();
        teams.sort_by_key(|n| n.to_ascii_lowercase());
        teams.dedup();
        Textures { vfs, loaded: HashMap::new(), colours: HashMap::new(), hud, teams }
    }

    /// The texture named `name` ([`qwc_hud::texture_path`]), none when the data has no such texture.
    pub fn get(&mut self, ctx: &egui::Context, name: &str) -> Option<&Tex> {
        if !self.loaded.contains_key(name) {
            let path = qwc_hud::texture_path(name);
            let tex = qwc_assets::texture::load(self.vfs, &path).ok().map(|t| {
                let image = egui::ColorImage::from_rgba_unmultiplied([t.width as usize, t.height as usize], t.rgba());
                Tex { handle: ctx.load_texture(name, image, egui::TextureOptions::LINEAR), size: [t.width, t.height] }
            });
            self.loaded.insert(name.to_string(), tex);
        }
        self.loaded.get(name).and_then(Option::as_ref)
    }

    /// Team `folder`'s colour, from its bar as the game takes it.
    pub fn team_colour(&mut self, folder: &str) -> Option<[f32; 3]> {
        let vfs = self.vfs;
        *self.colours.entry(folder.to_string()).or_insert_with(|| {
            let path = qwc_hud::texture_path(&format!("teams/{folder}/QWC_GUI_SnitchBar"));
            let t = qwc_assets::texture::load(vfs, &path).ok()?;
            qwc_hud::team_colour(t.rgba(), t.width)
        })
    }
}
