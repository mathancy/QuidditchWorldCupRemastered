//! The stadiums' looks: the remastered lighting's settings and colour grade for each stadium
//! (`qwc_render::remaster::Settings`), a RON file a stadium in a resource pack's `looks/` (`qwc_packs`;
//! `$QWC_LOOKS` names a folder instead), named by the stadium's folder (`england.ron`, `hog_win.ron`). A stadium
//! without one takes the defaults, as every stadium does without a pack, and a file names only what it changes. Not
//! the original's: the remaster's own (numbers, no game data). QWC Studio's Looks mode edits them; a running match
//! reads its stadium's again when it is saved ([`Watch`]).

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use qwc_render::remaster::Settings;

/// The look file of the stadium in folder `stadium`, which it is read from and saved to: in `$QWC_LOOKS` when that
/// is set, else the highest pack's that has one (`qwc_packs`, its `looks/`), else the highest pack's place for it.
/// None without a pack on.
pub fn path(stadium: &str) -> Option<PathBuf> {
    let file = format!("{}.ron", stadium.to_ascii_lowercase());
    if let Some(dir) = std::env::var_os("QWC_LOOKS") {
        return Some(PathBuf::from(dir).join(file));
    }
    let packs = qwc_packs::current();
    packs.look(stadium).or_else(|| packs.packs.first().map(|p| p.dir.join("looks").join(file)))
}

/// The look in `text`.
pub fn parse(text: &str) -> Result<Settings, String> {
    ron::from_str(text).map_err(|e| e.to_string())
}

/// The stadium's look: its file's, else the defaults (a file that does not read is reported).
pub fn load(stadium: &str) -> Settings {
    let Some(p) = path(stadium) else { return Settings::default() };
    match std::fs::read_to_string(&p) {
        Ok(text) => parse(&text).unwrap_or_else(|e| {
            eprintln!("{}: {e}; the default look is drawn", p.display());
            Settings::default()
        }),
        Err(_) => Settings::default(),
    }
}

/// The look as its file holds it: every setting that differs from the defaults.
pub fn to_text(look: &Settings) -> String {
    let full = ron::to_string(look).expect("the settings serialise");
    let defaults = ron::to_string(&Settings::default()).expect("the settings serialise");
    let fields = |s: &str| -> Vec<(String, String)> {
        let inner = s.trim().trim_start_matches('(').trim_end_matches(')');
        split_fields(inner).into_iter().filter_map(|f| f.split_once(':').map(|(k, v)| (k.into(), v.into()))).collect()
    };
    let base = fields(&defaults);
    let changed: Vec<String> = fields(&full)
        .into_iter()
        .filter(|(k, v)| !base.iter().any(|(bk, bv)| bk == k && same(bv, v)))
        .map(|(k, v)| format!("    {k}: {v},\n"))
        .collect();
    format!(
        "// The stadium's remastered look (qwc-game looks); what is not named keeps its default.\n(\n{})\n",
        changed.concat()
    )
}

/// Whether two fields' values are the same, their numbers to within float noise (a colour picker's round trip turns
/// 0.4 into 0.39999992).
fn same(a: &str, b: &str) -> bool {
    let nums = |s: &str| -> Vec<f64> {
        s.split(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | 'e' | 'E')))
            .filter_map(|t| t.parse().ok())
            .collect()
    };
    let (x, y) = (nums(a), nums(b));
    if x.is_empty() || x.len() != y.len() {
        return a == b;
    }
    x.iter().zip(&y).all(|(p, q)| (p - q).abs() <= 1e-5 * p.abs().max(1.0))
}

/// A RON struct's fields, split at the commas outside brackets.
fn split_fields(s: &str) -> Vec<&str> {
    let (mut out, mut depth, mut start) = (Vec::new(), 0, 0);
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim());
    out.retain(|f| !f.is_empty());
    out
}

/// Write the stadium's look file, making its folder.
pub fn save(stadium: &str, look: &Settings) -> Result<PathBuf, String> {
    let p = path(stadium).ok_or("no pack is on to save into: turn one on, or make one, in the Packs mode")?;
    let fail = |e: std::io::Error| format!("{}: {e}", p.display());
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(fail)?;
    }
    std::fs::write(&p, to_text(look)).map_err(fail)?;
    Ok(p)
}

/// A stadium's look file watched for saves (none without a pack on).
pub struct Watch {
    path: Option<PathBuf>,
    modified: Option<SystemTime>,
    checked: Instant,
}

impl Watch {
    pub fn new(stadium: &str) -> Watch {
        let path = path(stadium);
        let modified = path.as_ref().and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        Watch { path, modified, checked: Instant::now() }
    }

    /// The look again when the file has changed since the last call (looked at twice a second at most).
    pub fn poll(&mut self) -> Option<Settings> {
        let path = self.path.as_ref()?;
        if self.checked.elapsed() < Duration::from_millis(500) {
            return None;
        }
        self.checked = Instant::now();
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if modified == self.modified {
            return None;
        }
        self.modified = modified;
        let text = std::fs::read_to_string(path).unwrap_or_default();
        match parse(&text) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_names_only_what_changed() {
        let look = Settings { exposure: 1.25, tint: [1.0, 0.9, 0.8], ..Settings::default() };
        let text = to_text(&look);
        assert!(text.contains("exposure: 1.25") && text.contains("tint:"), "{text}");
        assert!(!text.contains("sun:"), "{text}");
        assert_eq!(parse(&text).unwrap(), look);
        assert_eq!(parse(&to_text(&Settings::default())).unwrap(), Settings::default());
        // float noise is no change
        let noisy = Settings { sky: [0.34, 0.399_999_92, 0.5], ..Settings::default() };
        assert!(!to_text(&noisy).contains("sky"), "{}", to_text(&noisy));
    }
}
