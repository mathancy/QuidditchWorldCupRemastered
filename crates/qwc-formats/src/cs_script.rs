//! Cutscene scripts (`root/cs/<name>/<name>.txt`; port of `tools/cs_script.py`).
//!
//! Blocks look like `< KIND NAME = "x" > { KEY = value ... }` with `#` comments to end of line.
//! Keys are upper-cased; values are trimmed and stripped of surrounding quotes.

use std::sync::OnceLock;

use regex::Regex;

use crate::util::{latin1, split_lines};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: String,
    pub name: String,
    /// `(KEY, value)` in file order (a repeated key keeps its first position and last value, like a Python dict).
    pub entries: Vec<(String, String)>,
}

impl Block {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// Python `str.upper` for the Latin-1 range the scripts use.
fn upper(s: &str) -> String {
    s.chars().flat_map(char::to_uppercase).collect()
}

/// Parse a script's bytes (decoded as Latin-1).
pub fn parse(bytes: &[u8]) -> Vec<Block> {
    static COMMENT: OnceLock<Regex> = OnceLock::new();
    static BLOCK: OnceLock<Regex> = OnceLock::new();
    static LINE: OnceLock<Regex> = OnceLock::new();
    let comment = COMMENT.get_or_init(|| Regex::new(r"#[^\n]*").unwrap());
    let block = BLOCK.get_or_init(|| Regex::new(r#"(?s)<\s*(\w+)\s+NAME\s*=\s*"([^"]*)"\s*>\s*\{(.*?)\}"#).unwrap());
    let line = LINE.get_or_init(|| Regex::new(r"^\s*(\w+)\s*=\s*(.+?)\s*$").unwrap());
    let text = latin1(bytes);
    let text = comment.replace_all(&text, "");
    let mut out = Vec::new();
    for cap in block.captures_iter(&text) {
        let mut entries: Vec<(String, String)> = Vec::new();
        for l in split_lines(&cap[3]) {
            if let Some(m) = line.captures(l) {
                let key = upper(&m[1]);
                let val = m[2].trim().trim_matches('"').to_string();
                match entries.iter_mut().find(|(k, _)| *k == key) {
                    Some(e) => e.1 = val,
                    None => entries.push((key, val)),
                }
            }
        }
        out.push(Block { kind: upper(&cap[1]), name: cap[2].to_string(), entries });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_comments_and_quotes() {
        let src = b"# header\n< CUTSCENE NAME = \"cs1\" >\n{\n  camera_fov = 70 # wide\n  STREAMING_ANIMS_FILE = \"a.sfa\"\r\n}\n<role NAME=\"r\">{ANIM_NAME = x}";
        let b = parse(src);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].kind, "CUTSCENE");
        assert_eq!(b[0].get("CAMERA_FOV"), Some("70"));
        assert_eq!(b[0].get("STREAMING_ANIMS_FILE"), Some("a.sfa"));
        assert_eq!(b[1].kind, "ROLE");
        assert_eq!(b[1].get("ANIM_NAME"), Some("x"));
    }
}
