//! The remaster's text: the game's bitmap fonts (`.ffn`) drawn with vector fonts (OTF/TTF) at the size they are
//! shown, in their own proportions. Not the original's.
//!
//! The list (`remastered/fonts.txt`, [`FontList`]) names a font file, upright and italic, for each of the game's
//! families; a family without an italic file can have its italic styles drawn as the upright font leaned over
//! (`slant`). An original font's name gives its family, size and style ([`Style::of`]: `TiepoloBlkITCTT30i_ds` is
//! Tiepolo at 30, italic, with a drop shadow); its line height (the `.ffn` header's) sets the vector font's size, so
//! the game's line spacing and wrapping stay. [`TextFont`] answers the layout's metrics in the front end's
//! 512 x 512 units for the window it is drawn on (a glyph's width on screen is its own, not stretched with the
//! screen). [`Renderer`] shapes a run (kerning, ligatures, marks: swash) and draws its glyphs, hinted, at the pixel
//! size: the fill (white for the game to tint, or the gold gradient of the large headings) and, for `_ds` styles,
//! a soft shadow drawn under all the fills.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use swash::scale::{Render, ScaleContext, Source};
use swash::shape::ShapeContext;
use swash::text::{Codepoint, Script};
use swash::zeno::{Angle, Format, Transform, Vector};
use swash::FontRef;

/// The sizes of the game's fonts.
const SIZES: [u16; 4] = [12, 16, 24, 30];

/// An original font's family, size and style, from its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Style {
    pub family: String,
    pub size: u16,
    pub italic: bool,
    pub shadow: bool,
}

impl Style {
    /// `TiepoloBlkITCTT30i_ds` (a path or a file name; any extension): the family, then 12, 16, 24 or 30, then `i`
    /// for italic, then `_ds` for the drop shadow.
    pub fn of(name: &str) -> Option<Style> {
        let file = name.rsplit(['/', '\\']).next()?;
        let mut stem = file.split_once('.').map_or(file, |(s, _)| s);
        let shadow = stem.to_ascii_lowercase().ends_with("_ds");
        if shadow {
            stem = &stem[..stem.len() - 3];
        }
        let italic = stem.ends_with('i');
        if italic {
            stem = &stem[..stem.len() - 1];
        }
        let size = *SIZES.iter().find(|s| stem.ends_with(&s.to_string()))?;
        let family = &stem[..stem.len() - size.to_string().len()];
        (!family.is_empty()).then(|| Style { family: family.to_string(), size, italic, shadow })
    }
}

/// An RGB colour from `rrggbb`.
fn hex(s: &str) -> Option<[f32; 3]> {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).ok().filter(|_| s.trim_start_matches('#').len() == 6)?;
    Some([16, 8, 0].map(|k| ((v >> k) & 0xff) as f32 / 255.0))
}

/// The drop shadow of the `_ds` styles, in fractions of the font's size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    pub dx: f32,
    pub dy: f32,
    pub opacity: f32,
    pub blur: f32,
}

/// The list of vector fonts and how the text looks.
#[derive(Debug, Clone, PartialEq)]
pub struct FontList {
    /// Family -> (upright, italic) files.
    pub families: HashMap<String, (PathBuf, Option<PathBuf>)>,
    /// Family -> a size factor (1 by default).
    pub scales: HashMap<String, f32>,
    /// Family -> how far its italic styles lean, in degrees to the right (0 by default: as the italic file draws).
    pub slants: HashMap<String, f32>,
    pub shadow: Shadow,
    /// The gold of the large sizes, from the capitals' top to the baseline, and the sizes it is for.
    pub gold: ([f32; 3], [f32; 3]),
    pub gold_sizes: Vec<u16>,
    /// Fonts for the characters a family's own lacks, tried in turn.
    pub fallback: Vec<PathBuf>,
}

impl Default for FontList {
    fn default() -> Self {
        FontList {
            families: HashMap::new(),
            scales: HashMap::new(),
            slants: HashMap::new(),
            shadow: Shadow { dx: 0.05, dy: 0.05, opacity: 0.85, blur: 0.02 },
            gold: (hex("fff7cd").unwrap(), hex("de9f34").unwrap()),
            gold_sizes: vec![24, 30],
            fallback: Vec::new(),
        }
    }
}

impl FontList {
    pub fn read(path: &Path) -> Result<FontList, String> {
        FontList::read_layered(&[path.to_path_buf()])
    }

    /// The lists at `paths` laid over one another in order (the stock list, then the packs', the highest last): each
    /// line changes only what it names, so a later list's family, scale, slant, shadow, gold or fallback wins and
    /// the rest stays the earlier lists'.
    pub fn read_layered(paths: &[PathBuf]) -> Result<FontList, String> {
        let mut l = FontList::default();
        for path in paths {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            l = FontList::parse_onto(l, &text, path.parent().unwrap_or(Path::new(".")))
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(l)
    }

    /// The list's text, its files relative to `base`. Lines (`#` starts a comment):
    /// `FAMILY = UPRIGHT [ITALIC]`, `scale FAMILY = N`, `slant FAMILY = DEGREES`, `shadow = DX DY OPACITY BLUR`,
    /// `gold = TOP BOTTOM [SIZES...]`, `fallback = FILE...`.
    pub fn parse(text: &str, base: &Path) -> Result<FontList, String> {
        FontList::parse_onto(FontList::default(), text, base)
    }

    /// [`FontList::parse`] over `l`: what the text names is changed, the rest kept.
    pub fn parse_onto(mut l: FontList, text: &str, base: &Path) -> Result<FontList, String> {
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let bad = |what: &str| format!("line {}: {what}", n + 1);
            let (key, value) = line.split_once('=').ok_or_else(|| bad("no `=`"))?;
            let (key, words) = (key.trim(), value.split_whitespace().collect::<Vec<_>>());
            let nums = |k: usize| -> Result<Vec<f32>, String> {
                let v: Vec<f32> = words.iter().filter_map(|w| w.parse().ok()).collect();
                (v.len() == k && words.len() == k).then_some(v).ok_or_else(|| bad(&format!("expected {k} numbers")))
            };
            if key == "shadow" {
                let v = nums(4)?;
                l.shadow = Shadow { dx: v[0], dy: v[1], opacity: v[2], blur: v[3] };
            } else if key == "gold" {
                let (Some(top), Some(bottom)) = (words.first().and_then(|w| hex(w)), words.get(1).and_then(|w| hex(w)))
                else {
                    return Err(bad("expected two rrggbb colours"));
                };
                l.gold = (top, bottom);
                if words.len() > 2 {
                    l.gold_sizes =
                        words[2..].iter().map(|w| w.parse().map_err(|_| bad("bad size"))).collect::<Result<_, _>>()?;
                }
            } else if key == "fallback" {
                l.fallback = words.iter().map(|w| base.join(w)).collect();
            } else if let Some(family) = key.strip_prefix("scale ") {
                l.scales.insert(family.trim().to_string(), nums(1)?[0]);
            } else if let Some(family) = key.strip_prefix("slant ") {
                let degrees = nums(1)?[0];
                if degrees.abs() >= 45.0 {
                    return Err(bad("a slant is less than 45 degrees"));
                }
                l.slants.insert(family.trim().to_string(), degrees);
            } else {
                let (Some(upright), italic) = (words.first(), words.get(1)) else { return Err(bad("no font file")) };
                l.families.insert(key.to_string(), (base.join(upright), italic.map(|i| base.join(i))));
            }
        }
        Ok(l)
    }
}

/// A font file loaded.
pub struct Face {
    pub path: PathBuf,
    data: Vec<u8>,
    /// Its id in the glyph cache.
    id: usize,
}

impl Face {
    fn font(&self) -> FontRef<'_> {
        FontRef::from_index(&self.data, 0).expect("checked when loaded")
    }
}

/// The faces, each file loaded once.
#[derive(Default)]
pub struct Faces {
    loaded: HashMap<PathBuf, Arc<Face>>,
}

impl Faces {
    pub fn load(&mut self, path: &Path) -> Result<Arc<Face>, String> {
        if let Some(f) = self.loaded.get(path) {
            return Ok(f.clone());
        }
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        FontRef::from_index(&data, 0).ok_or_else(|| format!("{}: not a font swash reads", path.display()))?;
        let face = Arc::new(Face { path: path.to_path_buf(), data, id: self.loaded.len() });
        self.loaded.insert(path.to_path_buf(), face.clone());
        Ok(face)
    }
}

/// How a font's glyphs are filled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fill {
    /// White, for the game's colour to tint.
    White,
    /// From the first colour at the capitals' top to the second at the baseline.
    Gradient([f32; 3], [f32; 3]),
}

/// An original font drawn with a vector font.
pub struct TextFont {
    pub face: Arc<Face>,
    /// For the characters `face` lacks (the list's `fallback`), at the same size.
    pub fallbacks: Vec<Arc<Face>>,
    pub style: Style,
    /// The font's size (its em) in the front end's units, and the original's line height.
    pub em: f32,
    pub line_height: u16,
    pub fill: Fill,
    pub shadow: Option<Shadow>,
    /// How far the glyphs lean, in degrees to the right: the list's `slant` for an italic style, else 0.
    pub slant: f32,
    /// The window (pixels) the metrics are for, and the metrics in units for it.
    screen: Cell<(u32, u32)>,
    advances: RefCell<HashMap<u16, i32>>,
    kerns: RefCell<HashMap<(u16, u16), i32>>,
}

impl TextFont {
    /// The original font named `name` (its path), `line_height` (its header's) high, if the list has its family.
    pub fn new(list: &FontList, faces: &mut Faces, name: &str, line_height: u16) -> Option<Result<TextFont, String>> {
        let style = Style::of(name)?;
        let (upright, italic) = list.families.get(&style.family)?;
        let path = if style.italic { italic.as_ref().unwrap_or(upright) } else { upright };
        let face = match faces.load(path) {
            Ok(f) => f,
            Err(e) => return Some(Err(e)),
        };
        let mut fallbacks = Vec::new();
        for p in list.fallback.iter().filter(|p| p.as_path() != path) {
            match faces.load(p) {
                Ok(f) => fallbacks.push(f),
                Err(e) => return Some(Err(e)),
            }
        }
        // the vector font's natural line height becomes the original's
        let m = face.font().metrics(&[]);
        let natural = (m.ascent + m.descent.abs() + m.leading).max(1.0) / m.units_per_em.max(1) as f32;
        let em = line_height as f32 / natural * list.scales.get(&style.family).copied().unwrap_or(1.0);
        let fill =
            if list.gold_sizes.contains(&style.size) { Fill::Gradient(list.gold.0, list.gold.1) } else { Fill::White };
        let shadow = style.shadow.then_some(list.shadow);
        let slant = if style.italic { list.slants.get(&style.family).copied().unwrap_or(0.0) } else { 0.0 };
        Some(Ok(TextFont {
            face,
            fallbacks,
            style,
            em,
            line_height,
            fill,
            shadow,
            slant,
            screen: Cell::new((1280, 720)),
            advances: RefCell::default(),
            kerns: RefCell::default(),
        }))
    }

    /// The window the text is drawn on, for the metrics (changing it forgets them).
    pub fn set_screen(&self, screen: (u32, u32)) {
        let screen = (screen.0.max(1), screen.1.max(1));
        if self.screen.replace(screen) != screen {
            self.advances.borrow_mut().clear();
            self.kerns.borrow_mut().clear();
        }
    }

    /// Pixels per unit, across and down.
    fn scale(&self) -> (f32, f32) {
        let (w, h) = self.screen.get();
        (w as f32 / 512.0, h as f32 / 512.0)
    }

    /// The font's size in pixels.
    pub fn pixels(&self) -> f32 {
        self.em * self.scale().1
    }

    /// The baseline below a line's top, in units.
    pub fn ascent(&self) -> f32 {
        let m = self.face.font().metrics(&[]);
        self.em * m.ascent / m.units_per_em.max(1) as f32
    }

    /// Which face draws `c` (0 the font's own, then the fallbacks), and its glyph there.
    fn glyph(&self, c: u16) -> Option<(usize, u16)> {
        let ch = char::from_u32(c as u32)?;
        std::iter::once(&self.face).chain(&self.fallbacks).enumerate().find_map(|(k, f)| {
            let g = f.font().charmap().map(ch);
            (g != 0).then_some((k, g))
        })
    }

    /// The face `k` of [`TextFont::glyph`].
    fn face_at(&self, k: usize) -> &Arc<Face> {
        if k == 0 {
            &self.face
        } else {
            &self.fallbacks[k - 1]
        }
    }

    /// A character's advance in pixels (0 when no face has a glyph for it).
    fn advance_px(&self, c: u16) -> f32 {
        self.glyph(c)
            .map_or(0.0, |(k, g)| self.face_at(k).font().glyph_metrics(&[]).scale(self.pixels()).advance_width(g))
    }

    /// The layout's advance (units across, rounded as the layout counts).
    pub fn advance(&self, c: u16) -> i32 {
        *self.advances.borrow_mut().entry(c).or_insert_with(|| (self.advance_px(c) / self.scale().0).round() as i32)
    }

    /// The layout's kerning of `c` after `prev` (units across): their shaped advance less their own.
    pub fn kern(&self, c: u16, prev: u16) -> i32 {
        if let Some(&k) = self.kerns.borrow().get(&(prev, c)) {
            return k;
        }
        let k = match (char::from_u32(prev as u32), char::from_u32(c as u32)) {
            // pairs in the font's own face (a fallback's glyph does not kern with its neighbours)
            (Some(a), Some(b))
                if self.glyph(prev).is_some_and(|g| g.0 == 0) && self.glyph(c).is_some_and(|g| g.0 == 0) =>
            {
                let mut ctx = ShapeContext::new();
                let mut shaper = ctx.builder(self.face.font()).size(self.pixels()).build();
                shaper.add_str(&format!("{a}{b}"));
                let mut first = None;
                shaper.shape_with(|cl| {
                    if first.is_none() {
                        first = Some(cl.glyphs.iter().map(|g| g.advance).sum::<f32>());
                    }
                });
                let px = first.unwrap_or(0.0) - self.advance_px(prev);
                (px / self.scale().0).round() as i32
            }
            _ => 0,
        };
        self.kerns.borrow_mut().insert((prev, c), k);
        k
    }
}

/// A glyph image's cache key: the face, the size in quarter pixels, the glyph, the quarter-pixel offset, the fill
/// or the shadow, and the look (fill colours, shadow, slant).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    face: usize,
    quarter_px: u32,
    glyph: u16,
    sub: u8,
    shadow: bool,
    look: u64,
}

impl GlyphKey {
    /// A number for the key (for the overlay's texture keys).
    pub fn hash64(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

/// A glyph drawn: RGBA8, `width` x `height`, its top left `left` right of and `top` above the pen.
#[derive(Debug, Clone, Default)]
pub struct GlyphImage {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A glyph placed on the window: its image and its top left in pixels.
#[derive(Debug, Clone, Copy)]
pub struct Placed {
    pub key: GlyphKey,
    pub x: f32,
    pub y: f32,
    pub width: u32,
    pub height: u32,
}

/// Shapes runs and draws glyphs, keeping the images.
#[derive(Default)]
pub struct Renderer {
    shape: ShapeContext,
    scale: ScaleContext,
    images: HashMap<GlyphKey, GlyphImage>,
}

/// The script a line is shaped as: its first character's that has one.
fn script_of(s: &str) -> Script {
    s.chars()
        .map(|c| c.script())
        .find(|s| !matches!(s, Script::Common | Script::Inherited | Script::Unknown))
        .unwrap_or(Script::Latin)
}

impl Renderer {
    pub fn new() -> Renderer {
        Renderer::default()
    }

    /// Forget every image (the window's size changed: new sizes come).
    pub fn clear(&mut self) {
        self.images.clear();
    }

    pub fn image(&self, key: &GlyphKey) -> Option<&GlyphImage> {
        self.images.get(key)
    }

    /// A run of `text` (the layout's characters: newline moves down a line, space and the code 0x40 advance by the
    /// layout's space, 0 ends) from `origin` (units: the line's top left) on `screen`: its shadows, then its fills.
    /// Each line's glyphs are spread to the layout's width for it, so the run sits where the layout put it (the
    /// layout rounds each advance to a unit).
    pub fn run(&mut self, font: &TextFont, text: &[u16], origin: [f32; 2], screen: (u32, u32)) -> Vec<Placed> {
        font.set_screen(screen);
        let (kx, ky) = font.scale();
        let px = font.pixels();
        let space_px = font.advance(b's' as u16) as f32 * kx;
        let (mut shadows, mut fills) = (Vec::new(), Vec::new());
        let mut line_top = origin[1];
        for (k, line) in text.split(|&c| c == 10).enumerate() {
            if k > 0 {
                line_top += font.line_height as f32;
            }
            // the characters drawn (control codes are the layout's), and the layout's width for them
            let (mut s, mut layout_w, mut prev) = (String::new(), 0i32, 0u16);
            for (i, &c) in line.iter().enumerate() {
                if c == 0 {
                    break;
                }
                if c == 0x20 || (c == 0x40 && (k > 0 || i > 0)) {
                    s.push(' ');
                    layout_w += font.advance(b's' as u16);
                    prev = c;
                } else if c >= 0x20 {
                    s.extend(char::from_u32(c as u32));
                    layout_w += font.advance(c) + if prev != 0 { font.kern(c, prev) } else { 0 };
                    prev = c;
                }
            }
            if s.is_empty() {
                continue;
            }
            // shaped in runs of one face each (spaces and characters no face has stay with the run they are in),
            // spaces as wide as the layout's
            let mut segments: Vec<(usize, String)> = Vec::new();
            for ch in s.chars() {
                let face = font.glyph(ch as u32 as u16).map(|g| g.0).filter(|_| ch != ' ');
                match (segments.last_mut(), face) {
                    (Some((k, run)), f) if f.is_none_or(|f| f == *k) => run.push(ch),
                    _ => segments.push((face.unwrap_or(0), ch.to_string())),
                }
            }
            let mut glyphs: Vec<(usize, u16, f32, f32)> = Vec::new();
            let mut pen = 0.0f32;
            for (k, part) in &segments {
                let f = font.face_at(*k).font();
                let mut shaper = self.shape.builder(f).size(px).script(script_of(part)).build();
                shaper.add_str(part);
                let space = f.charmap().map(' ');
                shaper.shape_with(|cl| {
                    for g in cl.glyphs {
                        let advance = if g.id == space && space != 0 { space_px } else { g.advance };
                        glyphs.push((*k, g.id, pen + g.x, g.y));
                        pen += advance;
                    }
                });
            }
            let spread = if pen > 0.0 { layout_w as f32 * kx / pen } else { 1.0 };
            let baseline = ((line_top + font.ascent()) * ky).round();
            let x0 = origin[0] * kx;
            for (face, g, gx, gy) in glyphs {
                let x = x0 + gx * spread;
                let (xi, sub) = (x.floor(), ((x - x.floor()) * 4.0).round() as u8);
                let (xi, sub) = if sub == 4 { (xi + 1.0, 0) } else { (xi, sub) };
                let y = baseline - gy;
                for (shadow, out) in [(true, &mut shadows), (false, &mut fills)] {
                    if shadow && font.shadow.is_none() {
                        continue;
                    }
                    let key = self.draw(font, face, g, px, sub, shadow);
                    let im = &self.images[&key];
                    if im.width > 0 && im.height > 0 {
                        out.push(Placed {
                            key,
                            x: xi + im.left as f32,
                            y: y - im.top as f32,
                            width: im.width,
                            height: im.height,
                        });
                    }
                }
            }
        }
        shadows.extend(fills);
        shadows
    }

    /// The glyph's image (drawn the first time).
    fn draw(&mut self, font: &TextFont, face: usize, glyph: u16, px: f32, sub: u8, shadow: bool) -> GlyphKey {
        let face = font.face_at(face).clone();
        let look = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            format!("{:?}{:?}{}", font.fill, font.shadow, font.slant).hash(&mut h);
            h.finish()
        };
        let key = GlyphKey { face: face.id, quarter_px: (px * 4.0).round() as u32, glyph, sub, shadow, look };
        if self.images.contains_key(&key) {
            return key;
        }
        let mut scaler = self.scale.builder(face.font()).size(px).hint(true).build();
        // the slant shears the outline about its baseline (y up), so the pen and the advances stay the upright's
        let lean = (font.slant != 0.0).then(|| Transform::skew(Angle::from_degrees(font.slant), Angle::ZERO));
        let mask = Render::new(&[Source::Outline])
            .format(Format::Alpha)
            .offset(Vector::new(sub as f32 / 4.0, 0.0))
            .transform(lean)
            .render(&mut scaler, glyph);
        let image = match mask {
            Some(m) if m.placement.width > 0 && m.placement.height > 0 => {
                let (w, h) = (m.placement.width as usize, m.placement.height as usize);
                let (left, top) = (m.placement.left, m.placement.top);
                if shadow {
                    let s = font.shadow.expect("only asked for with a shadow");
                    shadow_image(&m.data, w, h, left, top, px, s)
                } else {
                    let metrics = face.font().metrics(&[]);
                    let cap = px * metrics.cap_height.max(metrics.ascent * 0.7) / metrics.units_per_em.max(1) as f32;
                    fill_image(&m.data, w, h, left, top, cap, font.fill)
                }
            }
            _ => GlyphImage::default(),
        };
        self.images.insert(key, image);
        key
    }
}

/// The fill: the mask's coverage as alpha over white or the gradient (by the row's height over the baseline,
/// `cap` pixels at the top colour).
fn fill_image(mask: &[u8], w: usize, h: usize, left: i32, top: i32, cap: f32, fill: Fill) -> GlyphImage {
    let mut rgba = vec![0u8; w * h * 4];
    for (r, row) in rgba.chunks_exact_mut(w * 4).enumerate() {
        let colour = match fill {
            Fill::White => [1.0; 3],
            Fill::Gradient(a, b) => {
                // the pixel's centre over the baseline
                let above = top as f32 - r as f32 - 0.5;
                let t = (1.0 - above / cap.max(1.0)).clamp(0.0, 1.0);
                [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * t)
            }
        };
        let c = colour.map(|v| (v * 255.0).round() as u8);
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *px = [c[0], c[1], c[2], mask[r * w + x]];
        }
    }
    GlyphImage { left, top, width: w as u32, height: h as u32, rgba }
}

/// The shadow: the mask blurred (two box passes, `blur` of the size), black at `opacity`, moved by the offset.
fn shadow_image(mask: &[u8], w: usize, h: usize, left: i32, top: i32, px: f32, s: Shadow) -> GlyphImage {
    let r = (s.blur * px).round().max(0.0) as usize;
    let (bw, bh) = (w + 4 * r, h + 4 * r);
    let mut a: Vec<f32> = vec![0.0; bw * bh];
    for y in 0..h {
        for x in 0..w {
            a[(y + 2 * r) * bw + x + 2 * r] = mask[y * w + x] as f32;
        }
    }
    for _ in 0..2 {
        a = box_blur(&a, bw, bh, r);
    }
    let rgba = a.iter().flat_map(|&v| [0, 0, 0, (v * s.opacity).round().clamp(0.0, 255.0) as u8]).collect();
    let (dx, dy) = ((s.dx * px).round() as i32, (s.dy * px).round() as i32);
    GlyphImage {
        left: left - 2 * r as i32 + dx,
        top: top + 2 * r as i32 - dy,
        width: bw as u32,
        height: bh as u32,
        rgba,
    }
}

/// A box blur of radius `r`, across then down (edges clamped to 0 outside).
fn box_blur(a: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 {
        return a.to_vec();
    }
    let n = (2 * r + 1) as f32;
    let pass = |src: &[f32], len: usize, count: usize, at: &dyn Fn(usize, usize) -> usize| {
        let mut out = vec![0.0f32; src.len()];
        for line in 0..count {
            let mut sum = 0.0;
            for i in 0..=r.min(len - 1) {
                sum += src[at(line, i)];
            }
            for i in 0..len {
                out[at(line, i)] = sum / n;
                if i + r + 1 < len {
                    sum += src[at(line, i + r + 1)];
                }
                if i >= r {
                    sum -= src[at(line, i - r)];
                }
            }
        }
        out
    };
    let across = pass(a, w, h, &|y, x| y * w + x);
    pass(&across, h, w, &|x, y| y * w + x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_lists_lay_over_one_another() {
        let d = std::env::temp_dir().join(format!("qwc-text-layers-{}", std::process::id()));
        std::fs::create_dir_all(d.join("pack")).unwrap();
        std::fs::write(
            d.join("fonts.txt"),
            "Tiepolo = a.ttf b.ttf
Cardo = c.ttf
slant Tiepolo = 10
fallback = n.ttf
",
        )
        .unwrap();
        std::fs::write(
            d.join("pack").join("fonts.txt"),
            "Tiepolo = mine.ttf
shadow = 0 0 0 0
",
        )
        .unwrap();
        let l = FontList::read_layered(&[d.join("fonts.txt"), d.join("pack").join("fonts.txt")]).unwrap();
        assert_eq!(l.families["Tiepolo"], (d.join("pack").join("mine.ttf"), None));
        assert_eq!(l.families["Cardo"].0, d.join("c.ttf"));
        assert_eq!((l.slants["Tiepolo"], l.shadow.opacity, l.fallback.clone()), (10.0, 0.0, vec![d.join("n.ttf")]));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn styles_from_names() {
        let s = Style::of("frontend/Common/Fonts/Latin/TiepoloBlkITCTT30i_ds.ffn").unwrap();
        assert_eq!(s, Style { family: "TiepoloBlkITCTT".into(), size: 30, italic: true, shadow: true });
        assert_eq!(
            Style::of("SabonCEMT16_ds").unwrap(),
            Style { family: "SabonCEMT".into(), size: 16, italic: false, shadow: true }
        );
        assert_eq!(Style::of("DFGKakuPop-W512.ffn").unwrap().family, "DFGKakuPop-W5");
        assert_eq!(Style::of("AsiaMMGD-M12i_ds.ffn").unwrap().family, "AsiaMMGD-M");
        assert_eq!(
            Style::of("M24i_ds.ffn").unwrap(),
            Style { family: "M".into(), size: 24, italic: true, shadow: true }
        );
        assert_eq!(Style::of("root/System/IMEFont.ffn"), None);
    }

    #[test]
    fn parses_the_list() {
        let text = "# fonts\nTiepoloBlkITCTT = fonts/T.otf fonts/TI.otf\nM = fonts/sc.ttf\nscale M = 0.9\n\
                    slant M = 12\nshadow = 0.1 0.1 0.5 0\ngold = ffffff 000000 30\n";
        let l = FontList::parse(text, Path::new("r")).unwrap();
        let t = &l.families["TiepoloBlkITCTT"];
        assert_eq!((t.0.as_path(), t.1.as_deref()), (Path::new("r/fonts/T.otf"), Some(Path::new("r/fonts/TI.otf"))));
        assert_eq!(l.families["M"].1, None);
        assert_eq!(l.scales["M"], 0.9);
        assert_eq!((l.slants["M"], l.slants.get("TiepoloBlkITCTT")), (12.0, None));
        assert!(FontList::parse(
            "slant M = 60
",
            Path::new(".")
        )
        .is_err());
        assert_eq!(l.shadow, Shadow { dx: 0.1, dy: 0.1, opacity: 0.5, blur: 0.0 });
        assert_eq!((l.gold.0, l.gold.1, l.gold_sizes.as_slice()), ([1.0; 3], [0.0; 3], &[30u16][..]));
        assert!(FontList::parse("shadow = 1 2\n", Path::new(".")).unwrap_err().contains("line 1"));
        assert!(FontList::parse("gold = red blue\n", Path::new(".")).is_err());
    }

    #[test]
    fn blur_keeps_the_sum() {
        let mut a = vec![0.0; 9 * 9];
        a[4 * 9 + 4] = 81.0;
        let b = box_blur(&a, 9, 9, 1);
        assert!((b.iter().sum::<f32>() - 81.0).abs() < 1e-3);
        assert!((b[4 * 9 + 4] - 9.0).abs() < 1e-3 && (b[3 * 9 + 3] - 9.0).abs() < 1e-3 && b[2 * 9 + 2] == 0.0);
    }

    #[test]
    fn gradient_runs_from_the_capitals_to_the_baseline() {
        // a 1-wide mask 10 rows high, its top 10 px over the baseline; the capitals 10 px high
        let im = fill_image(&[255; 10], 1, 10, 0, 10, 10.0, Fill::Gradient([1.0; 3], [0.0; 3]));
        let reds: Vec<u8> = im.rgba.as_chunks::<4>().0.iter().map(|p| p[0]).collect();
        assert!(reds[0] > 240 && reds[9] < 15 && reds.windows(2).all(|w| w[0] >= w[1]), "{reds:?}");
    }

    /// With the fonts: a character Japanese's font lacks comes from the fallback.
    #[test]
    fn falls_back() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../remastered");
        let Ok(list) = FontList::read(&dir.join("fonts.txt")) else { return };
        let mut faces = Faces::default();
        let f = TextFont::new(&list, &mut faces, "DFGKakuPop-W516_ds.ffn", 33).expect("Japanese is listed").unwrap();
        // 0x4fa2 is not in Noto Sans JP; 0x3042 (hiragana a) is
        assert_eq!(f.glyph(0x3042).map(|g| g.0), Some(0));
        assert_eq!(f.glyph(0x4fa2).map(|g| g.0), Some(1));
        assert!(f.advance(0x4fa2) > 0);
        let placed = Renderer::new().run(&f, &[0x3042, 0x4fa2], [0.0, 0.0], (1280, 720));
        assert_eq!(placed.len(), 4, "two shadows, two fills");
    }

    /// With the fonts: a slant leans an italic style's glyphs to the right (their ink's top row starts further right
    /// than their bottom row's) and leaves the upright styles and every advance as they were.
    #[test]
    fn slant_leans_italics() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../remastered");
        let Ok(mut list) = FontList::read(&dir.join("fonts.txt")) else { return };
        list.families.insert("Lean".into(), (dir.join("fonts/Cardo-Regular.ttf"), None));
        list.slants.insert("Lean".into(), 14.0);
        let mut faces = Faces::default();
        let italic = TextFont::new(&list, &mut faces, "Lean30i.ffn", 65).unwrap().unwrap();
        let upright = TextFont::new(&list, &mut faces, "Lean30.ffn", 65).unwrap().unwrap();
        assert_eq!((italic.slant, upright.slant), (14.0, 0.0));
        assert_eq!(italic.advance(b'l' as u16), upright.advance(b'l' as u16));
        // the leftmost inked column of a glyph's top and bottom rows
        let lean = |f: &TextFont| {
            let mut r = Renderer::new();
            let p = r.run(f, &[b'l' as u16], [0.0, 0.0], (1280, 720))[0];
            let im = r.image(&p.key).unwrap();
            let first = |row: u32| (0..im.width).find(|&x| im.rgba[((row * im.width + x) * 4 + 3) as usize] > 128);
            let (top, bottom) = (first(1).unwrap() as i32, first(im.height - 2).unwrap() as i32);
            top - bottom
        };
        // 14 degrees over the glyph's ~30 px: about 7 px further right at the top than the upright's
        assert!(lean(&italic) - lean(&upright) >= 4, "{} {}", lean(&italic), lean(&upright));
    }

    /// With the fonts: the Latin font (Germania One for the original's Tiepolo) sized from the original's line
    /// height, its italic styles leaned, metrics across the window's width.
    #[test]
    fn latin_metrics() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../remastered");
        let Ok(list) = FontList::read(&dir.join("fonts.txt")) else { return };
        let mut faces = Faces::default();
        let f = TextFont::new(&list, &mut faces, "TiepoloBlkITCTT30i_ds.ffn", 65).expect("Latin is listed").unwrap();
        assert_eq!(list.slants.get("TiepoloBlkITCTT").map(|_| f.slant > 0.0), Some(true));
        assert_eq!((f.fill, f.shadow.is_some()), (Fill::Gradient(list.gold.0, list.gold.1), true));
        f.set_screen((1280, 720));
        let wide = f.advance(b'W' as u16);
        // on a 4:3 window the same glyph is more units across (each unit is fewer pixels)
        f.set_screen((960, 720));
        assert!(f.advance(b'W' as u16) > wide, "{} {wide}", f.advance(b'W' as u16));
        assert!(f.advance(b'W' as u16) > f.advance(b'i' as u16));
        let mut r = Renderer::new();
        let placed = r.run(&f, &"AV Wo".encode_utf16().collect::<Vec<_>>(), [100.0, 100.0], (1280, 720));
        // four glyphs (the space draws nothing), shadows first
        assert_eq!(placed.len(), 8);
        assert!(placed[..4].iter().all(|p| r.image(&p.key).unwrap().rgba.chunks(4).all(|px| px[..3] == [0, 0, 0])));
    }
}
