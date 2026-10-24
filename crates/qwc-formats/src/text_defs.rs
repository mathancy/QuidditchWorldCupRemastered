//! Text definition files (port of `tools/text_defs.py`): FX definitions, `.cli` tuning, `.gro` materials.
//!
//! ```text
//! .pot .lot .trl .got  < KIND name = "x" > { key = value }   values split on commas and whitespace like the game's
//!                      tokenizer (FUN_004011c0); '#' or '//' start a comment
//! .cli                 "dotted.key<whitespace>value" per line; a line with a single word is a command
//! .gro                 name { { stage directives } ... }, one directive per stage line
//! ```
//! [`vocab_problems`] checks blocks against the keyword tables in QWC.exe (0x5fb460-0x5fc7e0). Text outside any
//! block is kept as `leftover` (two shipped .pot files have some: an extra "}" and a trailing "S").

use std::sync::OnceLock;

use regex::Regex;

use crate::util::{bad, latin1, split_lines, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ext {
    Pot,
    Lot,
    Trl,
    Got,
    Cli,
    Gro,
}

impl Ext {
    /// From a lower-case extension without the dot.
    pub fn from_ext(ext: &str) -> Option<Ext> {
        Some(match ext {
            "pot" => Ext::Pot,
            "lot" => Ext::Lot,
            "trl" => Ext::Trl,
            "got" => Ext::Got,
            "cli" => Ext::Cli,
            "gro" => Ext::Gro,
            _ => return None,
        })
    }
}

/// A value: one word, or a list when there are zero or several (like the Python tool).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FxValue {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FxBlock {
    /// Upper-cased.
    pub kind: String,
    pub name: String,
    /// `(lower-cased key, value)` in file order.
    pub values: Vec<(String, FxValue)>,
    /// Lines without `key = ...`.
    pub unparsed: Vec<String>,
}

/// `(name, stages)`; a directive at material level is a stage of its own.
pub type Material = (String, Vec<Vec<String>>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Fx { blocks: Vec<FxBlock>, leftover: String },
    Cli { values: Vec<(String, Option<String>)>, unparsed: Vec<String> },
    Gro { materials: Vec<Material> },
}

/// Python `str.isspace` (adds `\x1c`-`\x1f` to Rust's whitespace).
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

fn strip_comments(text: &str) -> String {
    static C: OnceLock<Regex> = OnceLock::new();
    re(&C, r"(#|//)[^\n]*").replace_all(text, "").into_owned()
}

/// Returns `(blocks, leftover)` where leftover is the non-whitespace text outside any block.
pub fn parse_fx(text: &str) -> (Vec<FxBlock>, String) {
    static BLOCK: OnceLock<Regex> = OnceLock::new();
    static VALUE: OnceLock<Regex> = OnceLock::new();
    let block = re(&BLOCK, r#"(?si)<\s*(\w+)\s+name\s*=\s*"([^"]*)"\s*>\s*\{(.*?)\}"#);
    let value = re(&VALUE, r#""[^"\n]*"|[^\s,"]+"#);
    let text = strip_comments(text);
    let (mut blocks, mut leftover, mut last) = (Vec::new(), String::new(), 0);
    for cap in block.captures_iter(&text) {
        let m = cap.get(0).unwrap();
        leftover.push_str(&text[last..m.start()]);
        last = m.end();
        let (mut values, mut unparsed) = (Vec::new(), Vec::new());
        for line in split_lines(&cap[3]) {
            let line = line.trim_matches(py_space);
            if line.is_empty() {
                continue;
            }
            let Some((k, v)) = line.split_once('=').filter(|(k, _)| !k.trim_matches(py_space).is_empty()) else {
                unparsed.push(line.to_string());
                continue;
            };
            let mut vals: Vec<String> = value.find_iter(v).map(|m| m.as_str().trim_matches('"').to_string()).collect();
            let v = if vals.len() == 1 { FxValue::One(vals.pop().unwrap()) } else { FxValue::Many(vals) };
            values.push((k.trim_matches(py_space).to_lowercase(), v));
        }
        blocks.push(FxBlock { kind: cap[1].to_uppercase(), name: cap[2].to_string(), values, unparsed });
    }
    leftover.push_str(&text[last..]);
    (blocks, leftover.trim_matches(py_space).to_string())
}

/// `(key, value)` per line; a bare word (a command) has no value. Nothing is ever unparsed.
pub fn parse_cli(text: &str) -> (Vec<(String, Option<String>)>, Vec<String>) {
    let mut out = Vec::new();
    for line in split_lines(&strip_comments(text)) {
        let line = line.trim_matches(py_space);
        if line.is_empty() {
            continue;
        }
        out.push(match line.find(py_space) {
            None => (line.to_string(), None),
            Some(i) => (line[..i].to_string(), Some(line[i..].trim_start_matches(py_space).to_string())),
        });
    }
    (out, Vec::new())
}

/// `name { { directive... } ... }`.
pub fn parse_gro(text: &str) -> Result<Vec<Material>> {
    static TOK: OnceLock<Regex> = OnceLock::new();
    let text = strip_comments(text);
    let toks: Vec<&str> = re(&TOK, r"\{|\}|[^\s{}]+(?:[ \t]+[^\s{}]+)*").find_iter(&text).map(|m| m.as_str()).collect();
    let tok = |i: usize| toks.get(i).copied().ok_or_else(|| crate::Error::Format("unterminated .gro block".into()));
    let (mut mats, mut i) = (Vec::new(), 0);
    while i < toks.len() {
        let name = toks[i];
        if name == "{" || name == "}" || toks.get(i + 1) != Some(&"{") {
            return bad(format!("expected 'name {{' at token {i}: {:?}", &toks[i..toks.len().min(i + 3)]));
        }
        i += 2;
        let mut stages = Vec::new();
        while tok(i)? != "}" {
            if toks[i] == "{" {
                i += 1;
                let mut stage = Vec::new();
                while tok(i)? != "}" {
                    if toks[i] == "{" {
                        return bad(format!("nested block in stage of {name}"));
                    }
                    stage.push(toks[i].to_string());
                    i += 1;
                }
                stages.push(stage);
            } else {
                stages.push(vec![toks[i].to_string()]);
            }
            i += 1;
        }
        mats.push((name.to_string(), stages));
        i += 1;
    }
    Ok(mats)
}

/// Parse a file's bytes (Latin-1, universal newlines like Python's text mode).
pub fn parse(ext: Ext, bytes: &[u8]) -> Result<Parsed> {
    let text = latin1(bytes).replace("\r\n", "\n").replace('\r', "\n");
    Ok(match ext {
        Ext::Cli => {
            let (values, unparsed) = parse_cli(&text);
            Parsed::Cli { values, unparsed }
        }
        Ext::Gro => Parsed::Gro { materials: parse_gro(&text)? },
        _ => {
            let (blocks, leftover) = parse_fx(&text);
            Parsed::Fx { blocks, leftover }
        }
    })
}

const BEH_V3: &str = "ACC_BASE ACC_CLAMP_MIN ACC_CLAMP_MAX ACC_VARIANCE POS_BASE POS_CLAMP_MIN POS_CLAMP_MAX \
    POS_VARIANCE RING_RADIUS VEL_BASE VEL_CLAMP_MIN VEL_CLAMP_MAX VEL_VARIANCE";
const BEH_F: &str = "VEL_THRESH ALPHA_FADEIN ALPHA_FADEOUT SIZE_BASE SIZE_VARIANCE SIZE_VEL_BASE SIZE_VEL_VARIANCE \
    SIZE_ACC_BASE SIZE_ACC_VARIANCE SIZE_CLAMP_MIN SIZE_CLAMP_MAX EMISSION_RATE_BASE EMISSION_RATE_VARIANCE \
    EMISSION_RATE_CLAMP_MIN EMISSION_RATE_CLAMP_MAX EMISSION_RATE_VEL_SCALE LIFE_BASE LIFE_VARIANCE LIFE_CLAMP_MIN \
    LIFE_CLAMP_MAX ROT_BASE ROT_VARIANCE ROT_VEL_BASE ROT_VEL_VARIANCE ROT_VEL_CLAMP ROT_ACC_BASE ROT_ACC_VARIANCE \
    ROT_ACC_CLAMP SCALER_ADD SCALER_MUL SCALER_CLAMP_MIN SCALER_CLAMP_MAX";
const DESCRIPTOR: &[(&str, &str)] = &[
    ("BEHAVIOUR", "ref"),
    ("CELL_RENDER_FLAG", "b"),
    ("CELL_SIZE", "f"),
    ("DYNAMIC_PLACEMENT_FLAG", "b"),
    ("EMITTER_LIFE_BASE", "f"),
    ("EMITTER_LIFE_INFINITE", "b"),
    ("EMITTER_LIFE_VARIANCE", "f"),
    ("RGBA", "rgba"),
    ("SCALER", "ref"),
    ("LOCATOR", "s"),
    ("LOCATOR_PARENT", "s"),
    ("LOCATOR_NODE", "s"),
    ("MATERIAL", "ref"),
    ("PAUSE_TIME", "f"),
    ("PRIORITY", "i"),
    ("REPEAT_TIME", "f"),
    ("TRIGGER", "s"),
    ("TRIGGER_PARENT", "s"),
    ("TRIGGER_NODE", "s"),
    ("WAVEFORM_ALPHA", "ref"),
    ("WAVEFORM_RGB", "ref"),
    ("DO_IGNORE_ROT", "b"),
    ("DO_INTENSITY", "b"),
    ("DO_LIGHTING", "b"),
    ("DO_REPEAT", "b"),
];
const GRO: &str = "ALPHAFUNC ALPHAGEN ALPHAMODE ANIMMAP BLENDFUNC BINDMAP CLAMPMAP CONTEXT DEPTHWRITE FRAMEMASK LAYER \
    MAP MIPMAPBIAS PINGPONG RGBGEN SURFACE TEXTUREALPHA TEXTUREMODE TCMOD REGION BINDWIDTH BINDHEIGHT BINDPSM \
    VERTEX_MODULATE ZTEST ZFILL";

fn has(words: &str, w: &str) -> bool {
    words.split_whitespace().any(|x| x == w)
}

/// Value type of a particle key, from the .pot loaders (`TYPES` in the Python tool).
pub fn value_type(kind: &str, key: &str) -> Option<&'static str> {
    match kind {
        "BEHAVIOUR" if has(BEH_V3, key) => Some("v3"),
        "BEHAVIOUR" if has(BEH_F, key) => Some("f"),
        "BEHAVIOUR" if has("DO_ROTATE DO_REVERSE DO_RADIAL", key) => Some("b"),
        "DESCRIPTOR" => DESCRIPTOR.iter().find(|(k, _)| *k == key).map(|(_, t)| *t),
        _ => None,
    }
}

/// `None` if `kind` is not a block kind of `ext`, else whether its loader accepts `key` (`VOCAB`).
fn key_known(ext: Ext, kind: &str, key: &str) -> Option<bool> {
    let keys = match (ext, kind) {
        (Ext::Pot, "BEHAVIOUR" | "DESCRIPTOR") => return Some(value_type(kind, key).is_some()),
        (Ext::Pot, "MATERIAL") => {
            "ALPHA_TEST_ENABLE ALPHA_TEST_MODE ALPHA_TEST_VALUE ALPHA_TEST_FAIL_MODE BLEND_MODE \
             DEST_ALPHA_TEST_ENABLE FRAME_MASK LAYER TEXTURE Z_TEST_ENABLE Z_TEST_MODE Z_WRITE_ENABLE"
        }
        (Ext::Pot, "WAVEFORM") => "AMP BASE CLAMP_MAX CLAMP_MIN FREQ PHASE TYPE",
        (Ext::Got, "GEOM") => {
            "FLAG_BURST FLAG_CAMERA_FACE FLAG_CAMERA_TILT FLAG_ROT_IGNORE FLAG_TRANSITION COLOUR_START COLOUR_MID \
             COLOUR_END DEPTH_START DEPTH_MID DEPTH_END HEIGHT_START HEIGHT_MID HEIGHT_END MATERIAL WIDTH_START \
             WIDTH_MID WIDTH_END TIME_LIFE TIME_MID TIME_PAUSE LOCATOR_PARENT LOCATOR_NODE POS_BASE ROT_BASE ROT_VEL \
             SHAPE TESS U0 U1 V0 V1 TRIGGER_PARENT TRIGGER_NODE"
        }
        (Ext::Got | Ext::Trl, "MATERIAL") => "TEXTURE",
        (Ext::Lot, "LIGHT") => {
            "DO_BURST COLOUR_START COLOUR_MID COLOUR_END RADIUS_START RADIUS_MID RADIUS_END TIME_LIFE TIME_MID \
             LOCATOR_PARENT LOCATOR_NODE POS TRIGGER_PARENT TRIGGER_NODE"
        }
        (Ext::Trl, "TRAIL") => {
            "ALPHA_FADEIN ALPHA_FADEOUT FLAG_ROT_IGNORE LIFE LOCATOR_PARENT LOCATOR_NODE POS RATE ROT_BASE ROT_POS \
             ROT_VEL MATERIAL TILE TRIGGER_CMP TRIGGER_PARENT TRIGGER_NODE VEL_THRESH VEL_SCALE WIDTH_END WIDTH_START"
        }
        _ => return None,
    };
    Some(has(keys, key))
}

/// Python `repr` of a str.
fn py_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::from(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == q => out.extend(['\\', c]),
            c if c < ' ' || ('\x7f'..='\u{a0}').contains(&c) || c == '\u{ad}' => out += &format!("\\x{:02x}", c as u32),
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

/// Keys, kinds or directives the game's loaders do not know, and values of the wrong arity.
pub fn vocab_problems(ext: Ext, parsed: &Parsed) -> Vec<String> {
    let mut out = Vec::new();
    match parsed {
        Parsed::Gro { materials } => {
            for (name, stages) in materials {
                for d in stages.iter().flatten() {
                    if !has(GRO, &d.split(py_space).next().unwrap_or("").to_uppercase()) {
                        out.push(format!("{name}: unknown directive {}", py_repr(d)));
                    }
                }
            }
        }
        Parsed::Fx { blocks, .. } => {
            for b in blocks {
                if key_known(ext, &b.kind, "").is_none() {
                    out.push(format!("{}: unknown kind {}", b.name, b.kind));
                    continue;
                }
                for (k, v) in &b.values {
                    let key = k.to_uppercase();
                    if key_known(ext, &b.kind, &key) == Some(false) {
                        out.push(format!("{}: unknown key {key}", b.name));
                    }
                    let want = match value_type(&b.kind, &key) {
                        Some("v3") => 3,
                        Some("rgba") => 4,
                        _ => continue,
                    };
                    let has = match v {
                        FxValue::Many(l) if l.len() == want => continue,
                        FxValue::Many(l) => {
                            format!("[{}]", l.iter().map(|x| py_repr(x)).collect::<Vec<_>>().join(", "))
                        }
                        FxValue::One(s) => py_repr(s),
                    };
                    out.push(format!("{}: {key} wants {want} values, has {has}", b.name));
                }
            }
        }
        Parsed::Cli { .. } => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FX: &str = "# comment line\n< BEHAVIOUR name = \"b1\" >\n{\n  vel_base = .5 -1.5, -.2\n  \
                      acc_variance = 0, 0, 0,\n  do_rotate = 1\n}\n< DESCRIPTOR name = \"d1\" >\n{\n  \
                      behaviour = \"b1\"\n  rgba = 255, 128, 0, 255\n  colour = 1\n  pos_base = 1\n}\n\
                      < ATRAIL name = \"t\" >\n{\n  life = 1\n}\n";

    #[test]
    fn values_split_like_the_game() {
        let (blocks, left) = parse_fx(FX);
        assert_eq!(left, "");
        let many = |v: &[&str]| FxValue::Many(v.iter().map(|s| s.to_string()).collect());
        assert_eq!(blocks[0].values[0], ("vel_base".into(), many(&[".5", "-1.5", "-.2"])));
        assert_eq!(blocks[0].values[1].1, many(&["0", "0", "0"]));
        assert_eq!(blocks[1].values[0].1, FxValue::One("b1".into()));
    }

    #[test]
    fn vocab_reports_unknown_keys_and_kinds() {
        let (blocks, leftover) = parse_fx(FX);
        let probs = vocab_problems(Ext::Pot, &Parsed::Fx { blocks, leftover });
        assert_eq!(probs, ["d1: unknown key COLOUR", "d1: unknown key POS_BASE", "t: unknown kind ATRAIL"]);
        let (blocks, leftover) = parse_fx("<BEHAVIOUR name=\"b\">{ vel_base = 1 2\n pos_base = 'x' }");
        let probs = vocab_problems(Ext::Pot, &Parsed::Fx { blocks, leftover });
        assert_eq!(probs, ["b: VEL_BASE wants 3 values, has ['1', '2']", "b: POS_BASE wants 3 values, has \"'x'\""]);
    }

    #[test]
    fn gro_directives() {
        let p = parse(Ext::Gro, b"m1 { { map tex\r\n alphafunc gt0 } { bogus 1 } }").unwrap();
        let Parsed::Gro { materials } = &p else { panic!() };
        assert_eq!(materials[0].0, "m1");
        assert_eq!(materials[0].1[0], ["map tex", "alphafunc gt0"]);
        assert_eq!(vocab_problems(Ext::Gro, &p), ["m1: unknown directive 'bogus 1'"]);
        assert!(parse_gro("m1 { { map").is_err());
        assert!(parse_gro("{ m1 }").is_err());
    }

    #[test]
    fn cli_commands_and_leftover() {
        let (v, _) = parse_cli("A.b  1 2 # c\nPitch.Collision.Clear\n");
        assert_eq!(v, [("A.b".into(), Some("1 2".into())), ("Pitch.Collision.Clear".into(), None)]);
        let (b, left) = parse_fx("<LIGHT name=\"l\">{ pos = 1 }\n}\nS");
        assert_eq!((b.len(), left.as_str()), (1, "}\nS"));
    }
}
