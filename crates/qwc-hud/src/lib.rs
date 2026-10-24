//! The remaster's HUD layouts ("scenes"): elements on a canvas, placed the way Unreal's UMG canvas panel places
//! them so that a layout holds on any screen, and moved, filled, tinted and shown by values the game provides (a
//! team's power, its score, its colour). The HUD editor (QWC Studio, `qwc-studio`) makes and previews scenes and the game
//! draws them (`qwc-game`'s `hud`), both through [`Scene::evaluate`], so the editor shows what the game will.
//!
//! The canvas is the screen in units: the scene's design size at the scale its [`ScaleRule`] picks for the screen.
//! An element sits in its parent's box (the canvas, for the scene's top elements):
//! - its anchors are points in the parent's box, 0 to 1 across and down. On an axis whose two anchors meet, the
//!   element's position is its offset from the anchor and its size is its own; on an axis whose anchors are apart
//!   it stretches with the parent, and the position and the size are its insets from the first and second anchor;
//! - its alignment is the point of it (0 to 1 of its size) that sits at its position, on the axes that don't
//!   stretch;
//! - its scale is drawn about its pivot (0 to 1 of its box) and carries to its children; a negative scale mirrors
//!   (an image's texture with it, but a number still reads left to right);
//! - it is drawn in its layer's turn (the overlay's material layers) and in the scene's order within a layer;
//! - its colour multiplies its texture, times a colour value when it is tinted; its opacity carries to its
//!   children, and it shows only while its `show_if` value is true (`name`, or `!name` for false), its parent too.
//!
//! A driver adds a value's keys to a property: pairs of (value, amount), linear between them and held beyond the
//! ends. With keys (0, 0) and (1, 100) on `X`, a power of 0.5 moves the element 50 units, whether its anchors meet
//! or stretch it (then both insets shift); a size grows it, from its far edge where it stretches; the scales'
//! amounts add to a factor of 1 that multiplies the element's scale. A driver on an anchor moves the element by a
//! share of its parent's size, so an element in a box that stretches with the screen (a bar's track) can ride
//! across it on any screen.
//!
//! A repeated element ([`Repeat`]) is drawn once per count of a value, each copy a step on from the last, a `#` in
//! its names standing for the copy's index: a row of buttons, one per pass a challenge needs.
//!
//! Scenes are kept as RON in `scenes/`; [`MATCH`] is the match's and [`CHALLENGE`] a challenge's, built in.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// The match's HUD (`scenes/match.ron`).
pub const MATCH: &str = include_str!("../scenes/match.ron");

/// A challenge's HUD (`scenes/challenge.ron`), drawn in place of the match's.
pub const CHALLENGE: &str = include_str!("../scenes/challenge.ron");

/// The stock scenes by name (`match`, `challenge`): the HUD drawn without a pack's (`qwc_packs`, its `hud/`).
pub const BUILT_IN: &[(&str, &str)] = &[("match", MATCH), ("challenge", CHALLENGE)];

/// The stock scene `name`'s RON, when there is one.
pub fn built_in(name: &str) -> Option<&'static str> {
    BUILT_IN.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

/// A box: x0, y0, x1, y1 (the screen's pixels, or units, down from the top left).
pub type Rect = [f32; 4];

/// A layout: the canvas, the values it reads and its elements.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scene {
    /// The canvas's size in units at a scale of 1 (the original's HUD is 640 x 480).
    pub design: [f32; 2],
    pub scale: ScaleRule,
    /// The values the game provides, each with its value until set (the editor's preview).
    pub params: Vec<Param>,
    pub elements: Vec<Element>,
}

impl Default for Scene {
    fn default() -> Self {
        Scene { design: [640.0, 480.0], scale: ScaleRule::default(), params: Vec::new(), elements: Vec::new() }
    }
}

/// How the canvas's units scale to a screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScaleRule {
    /// The screen's shorter side over the design's (UMG's default).
    #[default]
    ShortestSide,
    LongestSide,
    Width,
    Height,
    /// The largest scale at which the whole design fits on the screen.
    Fit,
    /// The smallest scale at which the design covers the screen.
    Fill,
    /// The design stretched over the screen, across and down apart (how the original drew its 640 x 480 HUD).
    Stretch,
}

impl ScaleRule {
    pub const ALL: [ScaleRule; 7] = [
        ScaleRule::ShortestSide,
        ScaleRule::LongestSide,
        ScaleRule::Width,
        ScaleRule::Height,
        ScaleRule::Fit,
        ScaleRule::Fill,
        ScaleRule::Stretch,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ScaleRule::ShortestSide => "Shortest side",
            ScaleRule::LongestSide => "Longest side",
            ScaleRule::Width => "Width",
            ScaleRule::Height => "Height",
            ScaleRule::Fit => "Fit",
            ScaleRule::Fill => "Fill",
            ScaleRule::Stretch => "Stretch",
        }
    }

    /// Pixels per unit, across and down, on a screen of `screen` pixels.
    pub fn scale(self, screen: [f32; 2], design: [f32; 2]) -> [f32; 2] {
        let [w, h] = screen;
        let [dw, dh] = design.map(|d| d.max(1.0));
        let s = match self {
            ScaleRule::ShortestSide => w.min(h) / dw.min(dh),
            ScaleRule::LongestSide => w.max(h) / dw.max(dh),
            ScaleRule::Width => w / dw,
            ScaleRule::Height => h / dh,
            ScaleRule::Fit => (w / dw).min(h / dh),
            ScaleRule::Fill => (w / dw).max(h / dh),
            ScaleRule::Stretch => return [w / dw, h / dh],
        };
        [s, s]
    }
}

/// A value the game provides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Param {
    pub name: String,
    /// Its value until the game sets it, and the editor's preview.
    pub value: Value,
    /// A number's range, for the editor's slider.
    pub range: [f32; 2],
    #[serde(skip_serializing_if = "is_default")]
    pub hint: Hint,
    /// What the game puts in it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl Default for Param {
    fn default() -> Self {
        Param {
            name: String::new(),
            value: Value::Scalar(0.0),
            range: [0.0, 1.0],
            hint: Hint::None,
            note: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Scalar(f32),
    Flag(bool),
    /// RGBA, 0 to 1.
    Colour([f32; 4]),
    Text(String),
}

impl Value {
    /// As a number: a flag 0 or 1, a text 1 unless empty, a colour its alpha.
    pub fn number(&self) -> f32 {
        match self {
            Value::Scalar(v) => *v,
            Value::Flag(b) => f32::from(u8::from(*b)),
            Value::Colour(c) => c[3],
            Value::Text(t) => f32::from(u8::from(!t.is_empty())),
        }
    }

    pub fn truthy(&self) -> bool {
        self.number() != 0.0
    }

    /// As text: a whole number without its point.
    pub fn text(&self) -> String {
        match self {
            Value::Text(t) => t.clone(),
            Value::Scalar(v) if v.fract() == 0.0 && v.abs() < 1e9 => format!("{}", *v as i64),
            Value::Scalar(v) => format!("{v}"),
            Value::Flag(b) => if *b { "1" } else { "0" }.to_string(),
            Value::Colour(_) => String::new(),
        }
    }
}

/// What a value is for, so the editor can offer choices.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Hint {
    #[default]
    None,
    /// A team's folder (`teams/<folder>`).
    Team,
    /// The colour of the team whose folder is in the named value ([`team_colour`]).
    TeamColour(String),
    /// A texture's name.
    Texture,
}

/// The values a scene is drawn with, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Values(pub HashMap<String, Value>);

impl Values {
    /// A scene's values until set.
    pub fn defaults(scene: &Scene) -> Values {
        Values(scene.params.iter().map(|p| (p.name.clone(), p.value.clone())).collect())
    }

    pub fn set(&mut self, name: &str, value: Value) {
        self.0.insert(name.to_string(), value);
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    /// A value as a number, 0 when there is none.
    pub fn number(&self, name: &str) -> f32 {
        self.get(name).map_or(0.0, Value::number)
    }

    /// A value as text, empty when there is none.
    pub fn text(&self, name: &str) -> String {
        self.get(name).map_or_else(String::new, Value::text)
    }

    pub fn colour(&self, name: &str) -> Option<[f32; 4]> {
        match self.get(name)? {
            Value::Colour(c) => Some(*c),
            _ => None,
        }
    }

    /// Whether a `show_if` holds: empty always, `name` while the value is true, `!name` while it is false.
    pub fn shows(&self, show_if: &str) -> bool {
        let s = show_if.trim();
        match s.strip_prefix('!') {
            _ if s.is_empty() => true,
            Some(name) => !self.get(name.trim()).is_some_and(Value::truthy),
            None => self.get(s).is_some_and(Value::truthy),
        }
    }

    /// `text` with each `{name}` replaced by that value's text.
    pub fn substitute(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            let Some(close) = rest[open..].find('}') else { break };
            out.push_str(&rest[..open]);
            out.push_str(&self.text(&rest[open + 1..open + close]));
            rest = &rest[open + close + 1..];
        }
        out.push_str(rest);
        out
    }
}

/// An element's anchors in its parent's box (0 to 1 across and down).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anchors {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl Anchors {
    pub const fn point(x: f32, y: f32) -> Anchors {
        Anchors { min: [x, y], max: [x, y] }
    }

    /// Whether the element stretches with its parent on `axis` (0 across, 1 down).
    pub fn stretches(&self, axis: usize) -> bool {
        self.min[axis] != self.max[axis]
    }
}

/// Something drawn, or a box holding others.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Element {
    pub name: String,
    pub kind: Kind,
    pub anchors: Anchors,
    /// Per axis: the offset from the anchor, or where the axis stretches the inset from the first anchor.
    pub position: [f32; 2],
    /// Per axis: the size, or where the axis stretches the inset from the second anchor.
    pub size: [f32; 2],
    #[serde(skip_serializing_if = "is_zero2")]
    pub alignment: [f32; 2],
    #[serde(skip_serializing_if = "is_centre")]
    pub pivot: [f32; 2],
    #[serde(skip_serializing_if = "is_one2")]
    pub scale: [f32; 2],
    pub layer: u32,
    #[serde(skip_serializing_if = "is_white")]
    pub colour: [f32; 4],
    /// A colour value multiplying `colour` (none when empty).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tint: String,
    #[serde(skip_serializing_if = "is_one")]
    pub opacity: f32,
    /// The value it shows by: `name` while true, `!name` while false, always when empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub show_if: String,
    /// Hidden in the editor (its eye); the game still draws it.
    #[serde(skip_serializing_if = "is_default")]
    pub hidden: bool,
    /// Not picked or dragged in the editor's canvas.
    #[serde(skip_serializing_if = "is_default")]
    pub locked: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drivers: Vec<Driver>,
    /// Drawn once per count of a value, each copy moved on by a step ([`Repeat`]).
    #[serde(skip_serializing_if = "is_default")]
    pub repeat: Repeat,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Element>,
}

/// An element drawn as a row of copies: as many as the `count` value (at most `max`), copy `i` moved by `i` times
/// `step` units and with each `#` in its and its children's value and texture names read as `i` (so copy 2's
/// texture `{button.#}` is the text of the value `button.2`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Repeat {
    /// The value counting the copies; empty for one, unrepeated.
    pub count: String,
    pub max: u32,
    pub step: [f32; 2],
}

impl Default for Repeat {
    fn default() -> Self {
        Repeat { count: String::new(), max: 12, step: [0.0, 0.0] }
    }
}

impl Default for Element {
    fn default() -> Self {
        Element {
            name: "Element".to_string(),
            kind: Kind::Group,
            anchors: Anchors::point(0.0, 0.0),
            position: [0.0, 0.0],
            size: [100.0, 100.0],
            alignment: [0.0, 0.0],
            pivot: [0.5, 0.5],
            scale: [1.0, 1.0],
            layer: 0x34,
            colour: [1.0; 4],
            tint: String::new(),
            opacity: 1.0,
            show_if: String::new(),
            hidden: false,
            locked: false,
            drivers: Vec::new(),
            repeat: Repeat::default(),
            children: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Kind {
    /// A box for others, drawing nothing.
    #[default]
    Group,
    Image(Image),
    Number(Number),
}

/// A texture, or part of one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Image {
    /// The texture's name ([`texture_path`]); `{name}` puts in a value's text.
    pub texture: String,
    /// The part of the texture drawn: u0, v0, u1, v1.
    pub uv: [f32; 4],
    #[serde(skip_serializing_if = "is_default")]
    pub flip: [bool; 2],
    /// How much of it is shown (0 to 1), from the `fill_from` edge.
    pub fill: f32,
    #[serde(skip_serializing_if = "is_default")]
    pub fill_from: Edge,
    /// Whether the part shown shows that part of the texture (else the whole of it, squeezed).
    pub crop: bool,
}

impl Default for Image {
    fn default() -> Self {
        Image {
            texture: String::new(),
            uv: [0.0, 0.0, 1.0, 1.0],
            flip: [false; 2],
            fill: 1.0,
            fill_from: Edge::Left,
            crop: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edge {
    #[default]
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    pub const ALL: [Edge; 4] = [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom];
}

/// A value as a row of numeral textures.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Number {
    /// The value shown (rounded, at least 0).
    pub value: String,
    /// How many numerals: zeros fill the front, and the number is held at the largest that fits.
    pub digits: u32,
    /// The numerals' texture names, `{d}` the digit.
    pub numerals: String,
    /// A numeral's size, and the distance from one to the next.
    pub glyph: [f32; 2],
    pub step: f32,
    /// Where the row sits across the box: 0 at its left, 0.5 in its middle, 1 at its right (down, in its middle).
    pub justify: f32,
}

impl Default for Number {
    fn default() -> Self {
        Number {
            value: String::new(),
            digits: 3,
            numerals: "QWC_GUI_Numerals{d}_execversion".to_string(),
            glyph: [54.4, 54.4],
            step: 37.4,
            justify: 0.5,
        }
    }
}

/// A value moving a property: its keys, (value, amount), added to the property.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Driver {
    pub value: String,
    pub property: Property,
    pub keys: Vec<[f32; 2]>,
}

impl Default for Driver {
    fn default() -> Self {
        Driver { value: String::new(), property: Property::X, keys: vec![[0.0, 0.0], [1.0, 0.0]] }
    }
}

impl Driver {
    /// The amount at value `v`: linear between the keys, held beyond the ends.
    pub fn at(&self, v: f32) -> f32 {
        let sorted;
        let keys = if self.keys.is_sorted_by(|a, b| a[0] <= b[0]) {
            &self.keys
        } else {
            sorted = {
                let mut k = self.keys.clone();
                k.sort_by(|a, b| a[0].total_cmp(&b[0]));
                k
            };
            &sorted
        };
        let (Some(first), Some(last)) = (keys.first(), keys.last()) else { return 0.0 };
        if v <= first[0] {
            return first[1];
        }
        if v >= last[0] {
            return last[1];
        }
        for w in keys.windows(2) {
            let ([t0, a0], [t1, a1]) = (w[0], w[1]);
            if v <= t1 {
                let f = if t1 > t0 { (v - t0) / (t1 - t0) } else { 1.0 };
                return a0 + (a1 - a0) * f;
            }
        }
        last[1]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Property {
    #[default]
    X,
    Y,
    Width,
    Height,
    /// Both anchors across (so a point anchor moves).
    AnchorX,
    AnchorY,
    Scale,
    ScaleX,
    ScaleY,
    Opacity,
    Fill,
}

impl Property {
    pub const ALL: [Property; 11] = [
        Property::X,
        Property::Y,
        Property::Width,
        Property::Height,
        Property::AnchorX,
        Property::AnchorY,
        Property::Scale,
        Property::ScaleX,
        Property::ScaleY,
        Property::Opacity,
        Property::Fill,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Property::X => "Position X",
            Property::Y => "Position Y",
            Property::Width => "Size X",
            Property::Height => "Size Y",
            Property::AnchorX => "Anchor X",
            Property::AnchorY => "Anchor Y",
            Property::Scale => "Scale",
            Property::ScaleX => "Scale X",
            Property::ScaleY => "Scale Y",
            Property::Opacity => "Opacity",
            Property::Fill => "Fill",
        }
    }
}

/// An element as placed on a screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    /// Where it is in the scene: its index at each level.
    pub path: Vec<usize>,
    /// Its box on the screen (x0 < x1, y0 < y1).
    pub rect: Rect,
    /// Its parent's box (the canvas, for a top element) and its anchors' box in it, on the screen.
    pub parent: Rect,
    pub anchors: Rect,
    /// Screen pixels per unit of its position and size (its parent's scale; negative where that mirrors).
    pub units: [f32; 2],
    /// Its parent's size in units.
    pub parent_size: [f32; 2],
    /// Whether it shows: its and its parents' `show_if` (and, in the editor, their eyes).
    pub shown: bool,
}

/// A textured quad on the screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    /// The texture's name ([`texture_path`]).
    pub texture: String,
    /// x0 < x1, y0 < y1.
    pub rect: Rect,
    /// The texture's coordinates at (x0, y0) and at (x1, y1).
    pub uv: [f32; 4],
    /// RGBA multiplying the texture (1 leaves it as it is).
    pub colour: [f32; 4],
    pub layer: u32,
    /// Its element, in [`Frame::placed`].
    pub element: usize,
}

/// A scene on a screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    /// Pixels per unit, across and down.
    pub scale: [f32; 2],
    /// The canvas in units.
    pub canvas: [f32; 2],
    /// Every element, parents before their children.
    pub placed: Vec<Placed>,
    /// In drawing order: by layer, then the scene's order.
    pub draws: Vec<Draw>,
}

/// A frame's mapping to the screen: `p * k + o`.
#[derive(Clone, Copy, Debug)]
struct Xf {
    k: [f32; 2],
    o: [f32; 2],
}

impl Xf {
    fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        [p[0] * self.k[0] + self.o[0], p[1] * self.k[1] + self.o[1]]
    }

    /// `r` on the screen, put the right way round.
    fn rect(&self, r: Rect) -> Rect {
        let ([ax, ay], [bx, by]) = (self.apply([r[0], r[1]]), self.apply([r[2], r[3]]));
        [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)]
    }

    /// This frame scaled by `s` about `p` (in its units).
    fn about(&self, p: [f32; 2], s: [f32; 2]) -> Xf {
        Xf {
            k: [self.k[0] * s[0], self.k[1] * s[1]],
            o: [p[0] * (1.0 - s[0]) * self.k[0] + self.o[0], p[1] * (1.0 - s[1]) * self.k[1] + self.o[1]],
        }
    }
}

/// An element's properties with its drivers added.
struct Props {
    anchors: Anchors,
    position: [f32; 2],
    size: [f32; 2],
    scale: [f32; 2],
    opacity: f32,
    fill: f32,
}

impl Props {
    fn of(e: &Element, values: &Values) -> Props {
        let fill = match &e.kind {
            Kind::Image(i) => i.fill,
            _ => 1.0,
        };
        let mut p = Props {
            anchors: e.anchors,
            position: e.position,
            size: e.size,
            scale: [1.0, 1.0],
            opacity: e.opacity,
            fill,
        };
        // where an axis stretches, its position and size are insets: a move shifts both, a size moves the far one
        let stretches = [e.anchors.stretches(0), e.anchors.stretches(1)];
        for d in &e.drivers {
            let a = d.at(values.number(&d.value));
            let mut shift = |axis: usize| {
                p.position[axis] += a;
                if stretches[axis] {
                    p.size[axis] -= a;
                }
            };
            match d.property {
                Property::X => shift(0),
                Property::Y => shift(1),
                Property::Width | Property::Height => {
                    let axis = usize::from(d.property == Property::Height);
                    p.size[axis] += if stretches[axis] { -a } else { a };
                }
                Property::AnchorX => {
                    p.anchors.min[0] += a;
                    p.anchors.max[0] += a;
                }
                Property::AnchorY => {
                    p.anchors.min[1] += a;
                    p.anchors.max[1] += a;
                }
                Property::Scale => p.scale = [p.scale[0] + a, p.scale[1] + a],
                Property::ScaleX => p.scale[0] += a,
                Property::ScaleY => p.scale[1] += a,
                Property::Opacity => p.opacity += a,
                Property::Fill => p.fill += a,
            }
        }
        p.scale = [p.scale[0] * e.scale[0], p.scale[1] * e.scale[1]];
        p
    }
}

/// An element's box in its parent's (of `parent` units, from the parent's top left), and its anchors' box, for
/// these anchors, position and size.
fn layout(e: &Element, anchors: Anchors, position: [f32; 2], size: [f32; 2], parent: [f32; 2]) -> (Rect, Rect) {
    let (mut r, mut a) = ([0.0; 4], [0.0; 4]);
    for axis in 0..2 {
        let (a0, a1) = (parent[axis] * anchors.min[axis], parent[axis] * anchors.max[axis]);
        (a[axis], a[axis + 2]) = (a0, a1);
        if e.anchors.stretches(axis) {
            (r[axis], r[axis + 2]) = (a0 + position[axis], a1 - size[axis]);
        } else {
            let x0 = a0 + position[axis] - e.alignment[axis] * size[axis];
            (r[axis], r[axis + 2]) = (x0, x0 + size[axis]);
        }
    }
    (r, a)
}

impl Element {
    /// Its box in a parent of `parent` units (from the parent's top left), without its drivers.
    pub fn local_box(&self, parent: [f32; 2]) -> Rect {
        layout(self, self.anchors, self.position, self.size, parent).0
    }

    /// Its position and size set so that its box (without its drivers) is `r` in a parent of `parent` units.
    pub fn set_local_box(&mut self, r: Rect, parent: [f32; 2]) {
        for axis in 0..2 {
            let (a0, a1) = (parent[axis] * self.anchors.min[axis], parent[axis] * self.anchors.max[axis]);
            let (x0, x1) = (r[axis], r[axis + 2]);
            if self.anchors.stretches(axis) {
                (self.position[axis], self.size[axis]) = (x0 - a0, a1 - x1);
            } else {
                self.size[axis] = x1 - x0;
                self.position[axis] = x0 + self.alignment[axis] * self.size[axis] - a0;
            }
        }
    }

    /// Its anchors set to `to`, its box kept where it is in a parent of `parent` units.
    pub fn set_anchors_keeping_box(&mut self, to: Anchors, parent: [f32; 2]) {
        let r = self.local_box(parent);
        self.anchors = to;
        self.set_local_box(r, parent);
    }

    /// Its alignment set to `to`, its box kept where it is.
    pub fn set_alignment_keeping_box(&mut self, to: [f32; 2], parent: [f32; 2]) {
        let r = self.local_box(parent);
        self.alignment = to;
        self.set_local_box(r, parent);
    }

    /// How many copies it draws with `values` ([`Repeat`]): 1 unrepeated.
    pub fn copies(&self, values: &Values) -> usize {
        if self.repeat.count.is_empty() {
            return 1;
        }
        values.number(&self.repeat.count).round().clamp(0.0, self.repeat.max as f32) as usize
    }

    /// Copy `i` of a repeated element: moved by `i` steps, unrepeated, each `#` in the names read as `i`.
    pub fn copy(&self, i: usize) -> Element {
        let mut e = self.clone();
        e.repeat = Repeat::default();
        // where an axis stretches both insets move, as a driver on the position moves them
        for axis in 0..2 {
            let d = self.repeat.step[axis] * i as f32;
            e.position[axis] += d;
            if self.anchors.stretches(axis) {
                e.size[axis] -= d;
            }
        }
        e.index(&i.to_string());
        e
    }

    /// Each `#` in its and its children's value and texture names read as `i`.
    fn index(&mut self, i: &str) {
        let at = |s: &mut String| {
            if s.contains('#') {
                *s = s.replace('#', i);
            }
        };
        at(&mut self.tint);
        at(&mut self.show_if);
        for d in &mut self.drivers {
            at(&mut d.value);
        }
        match &mut self.kind {
            Kind::Group => {}
            Kind::Image(img) => at(&mut img.texture),
            Kind::Number(n) => at(&mut n.value),
        }
        for c in &mut self.children {
            c.index(i);
        }
    }
}

impl Scene {
    pub fn from_ron(text: &str) -> Result<Scene, String> {
        ron::from_str(text).map_err(|e| e.to_string())
    }

    pub fn to_ron(&self) -> String {
        let config = ron::ser::PrettyConfig::new()
            .indentor("    ")
            .escape_strings(false)
            .extensions(ron::extensions::Extensions::UNWRAP_VARIANT_NEWTYPES);
        let mut text = ron::ser::to_string_pretty(self, config).unwrap_or_default();
        text.push('\n');
        text
    }

    pub fn param(&self, name: &str) -> Option<&Param> {
        self.params.iter().find(|p| p.name == name)
    }

    /// The element at `path` (its index at each level).
    pub fn get(&self, path: &[usize]) -> Option<&Element> {
        let (first, rest) = path.split_first()?;
        rest.iter().try_fold(self.elements.get(*first)?, |e, &i| e.children.get(i))
    }

    pub fn get_mut(&mut self, path: &[usize]) -> Option<&mut Element> {
        let (first, rest) = path.split_first()?;
        rest.iter().try_fold(self.elements.get_mut(*first)?, |e, &i| e.children.get_mut(i))
    }

    /// The list holding the element at `path`.
    pub fn siblings_mut(&mut self, path: &[usize]) -> Option<&mut Vec<Element>> {
        match path.split_last()? {
            (_, []) => Some(&mut self.elements),
            (_, parent) => Some(&mut self.get_mut(parent)?.children),
        }
    }

    /// The scene on a screen of `screen` pixels with `values`; `editor` leaves out the elements hidden there.
    pub fn evaluate(&self, screen: [f32; 2], values: &Values, editor: bool) -> Frame {
        let scale = self.scale.scale(screen, self.design);
        let canvas = [screen[0] / scale[0], screen[1] / scale[1]];
        let mut f = Frame { scale, canvas, placed: Vec::new(), draws: Vec::new() };
        let root = Xf { k: scale, o: [0.0, 0.0] };
        let mut path = Vec::new();
        for (i, e) in self.elements.iter().enumerate() {
            path.push(i);
            place_copies(e, [0.0, 0.0, canvas[0], canvas[1]], root, values, editor, &mut path, (true, 1.0), &mut f);
            path.pop();
        }
        f.draws.sort_by_key(|d| d.layer);
        f
    }
}

/// Place `e` in its parent's box `parent` (units of the parent's frame `xf`), then its children.
#[allow(clippy::too_many_arguments)]
fn place(
    e: &Element,
    parent: Rect,
    xf: Xf,
    values: &Values,
    editor: bool,
    path: &mut Vec<usize>,
    (parent_shown, parent_opacity): (bool, f32),
    f: &mut Frame,
) {
    let p = Props::of(e, values);
    let size = [parent[2] - parent[0], parent[3] - parent[1]];
    let (mut r, mut a) = layout(e, p.anchors, p.position, p.size, size);
    for v in [&mut r, &mut a] {
        *v = [v[0] + parent[0], v[1] + parent[1], v[2] + parent[0], v[3] + parent[1]];
    }
    let shown = parent_shown && values.shows(&e.show_if) && !(editor && e.hidden);
    let opacity = parent_opacity * p.opacity.clamp(0.0, 1.0);
    let pivot = [r[0] + (r[2] - r[0]) * e.pivot[0], r[1] + (r[3] - r[1]) * e.pivot[1]];
    let own = xf.about(pivot, p.scale);
    let element = f.placed.len();
    f.placed.push(Placed {
        path: path.clone(),
        rect: own.rect(r),
        parent: xf.rect(parent),
        anchors: xf.rect(a),
        units: xf.k,
        parent_size: size,
        shown,
    });
    if shown {
        let tint = values.colour(&e.tint).unwrap_or([1.0; 4]);
        let mut colour = [0, 1, 2, 3].map(|i| e.colour[i] * tint[i]);
        colour[3] *= opacity;
        match &e.kind {
            Kind::Group => {}
            Kind::Image(img) => {
                if let Some(d) = image(img, r, own, p.fill, values) {
                    f.draws.push(Draw { colour, layer: e.layer, element, ..d });
                }
            }
            Kind::Number(n) => {
                for d in number(n, r, own, values) {
                    f.draws.push(Draw { colour, layer: e.layer, element, ..d });
                }
            }
        }
    }
    for (i, c) in e.children.iter().enumerate() {
        path.push(i);
        place_copies(c, r, own, values, editor, path, (shown, opacity), f);
        path.pop();
    }
}

/// [`place`] `e`, or each of its copies when it repeats ([`Repeat`]; every copy is placed with `e`'s path).
#[allow(clippy::too_many_arguments)]
fn place_copies(
    e: &Element,
    parent: Rect,
    xf: Xf,
    values: &Values,
    editor: bool,
    path: &mut Vec<usize>,
    inherited: (bool, f32),
    f: &mut Frame,
) {
    if e.repeat.count.is_empty() {
        place(e, parent, xf, values, editor, path, inherited, f);
        return;
    }
    for i in 0..e.copies(values) {
        place(&e.copy(i), parent, xf, values, editor, path, inherited, f);
    }
}

/// An image's quad: box `r` in the frame `xf`, `fill` of it shown.
fn image(img: &Image, r: Rect, xf: Xf, fill: f32, values: &Values) -> Option<Draw> {
    let texture = values.substitute(&img.texture);
    let fill = fill.clamp(0.0, 1.0);
    if texture.is_empty() || fill <= 0.0 {
        return None;
    }
    let [mut x0, mut y0, mut x1, mut y1] = r;
    let [mut u0, mut v0, mut u1, mut v1] = img.uv;
    if img.flip[0] {
        (u0, u1) = (u1, u0);
    }
    if img.flip[1] {
        (v0, v1) = (v1, v0);
    }
    let keep = |crop: bool, a: f32, b: f32| if crop { a + (b - a) * fill } else { b };
    match img.fill_from {
        Edge::Left => (x1, u1) = (x0 + (x1 - x0) * fill, keep(img.crop, u0, u1)),
        Edge::Right => (x0, u0) = (x1 - (x1 - x0) * fill, keep(img.crop, u1, u0)),
        Edge::Top => (y1, v1) = (y0 + (y1 - y0) * fill, keep(img.crop, v0, v1)),
        Edge::Bottom => (y0, v0) = (y1 - (y1 - y0) * fill, keep(img.crop, v1, v0)),
    }
    let ([ax, ay], [bx, by]) = (xf.apply([x0, y0]), xf.apply([x1, y1]));
    let (mut rect, mut uv) = ([ax, ay, bx, by], [u0, v0, u1, v1]);
    for axis in 0..2 {
        if rect[axis] > rect[axis + 2] {
            rect.swap(axis, axis + 2);
            uv.swap(axis, axis + 2);
        }
    }
    (rect[2] > rect[0] && rect[3] > rect[1]).then_some(Draw {
        texture,
        rect,
        uv,
        colour: [1.0; 4],
        layer: 0,
        element: 0,
    })
}

/// A number's numerals: in box `r` of the frame `xf`, reading left to right on the screen.
fn number(n: &Number, r: Rect, xf: Xf, values: &Values) -> Vec<Draw> {
    let digits = n.digits.clamp(1, 9);
    let largest = 10u64.pow(digits) - 1;
    let v = values.number(&n.value).round().max(0.0) as u64;
    let text = format!("{:0w$}", v.min(largest), w = digits as usize);
    let width = n.step * (text.len() - 1) as f32 + n.glyph[0];
    let x0 = r[0] + (r[2] - r[0] - width) * n.justify;
    let y0 = r[1] + (r[3] - r[1] - n.glyph[1]) / 2.0;
    let row = xf.rect([x0, y0, x0 + width, y0 + n.glyph[1]]);
    let k = xf.k[0].abs();
    text.chars()
        .enumerate()
        .map(|(i, d)| {
            let x = row[0] + i as f32 * n.step * k;
            Draw {
                texture: n.numerals.replace("{d}", &d.to_string()),
                rect: [x, row[1], x + n.glyph[0] * k, row[3]],
                uv: [0.0, 0.0, 1.0, 1.0],
                colour: [1.0; 4],
                layer: 0,
                element: 0,
            }
        })
        .collect()
}

/// A texture's file in the game data: a name with a `/` is a path without its `.fsh`, else a HUD texture
/// (`root/HUD/<name>.fsh`).
pub fn texture_path(name: &str) -> String {
    if name.contains('/') {
        format!("{name}.fsh")
    } else {
        format!("root/HUD/{name}.fsh")
    }
}

/// A team's colour from its bar (`teams/<folder>/QWC_GUI_SnitchBar`, RGBA `width` pixels across): the pixel
/// between its rails (**[I]**, as the original's screen shows it).
pub fn team_colour(rgba: &[u8], width: u32) -> Option<[f32; 3]> {
    let at = ((8 * width + 95) * 4) as usize;
    rgba.get(at..at + 3).map(|c| [c[0], c[1], c[2]].map(|v| f32::from(v) / 255.0))
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

fn is_zero2(v: &[f32; 2]) -> bool {
    *v == [0.0, 0.0]
}

fn is_centre(v: &[f32; 2]) -> bool {
    *v == [0.5, 0.5]
}

fn is_one2(v: &[f32; 2]) -> bool {
    *v == [1.0, 1.0]
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

fn is_white(v: &[f32; 4]) -> bool {
    *v == [1.0; 4]
}

#[cfg(test)]
mod tests;
