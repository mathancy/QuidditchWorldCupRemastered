//! What the remaster records beside the original's data (`docs/LAUNCHER.md`, "Gameplay packs"). Not the original's.
//!
//! - **The save on disk** ([`save_card`]): the windowed front end's memory card is the file the original keeps
//!   (`qwc_feck::memcard::DirCard::documents`), so the port and the original share their progress; a headless run
//!   keeps its card in memory, so its pictures do not depend on a player's save.
//! - **The rules beside the save:** each time the save is written, `<save>.remaster.ron` lists every Gameplay pack
//!   (id and version) the saved progress has been made under. The save itself stays the original's format.
//! - **The match log:** each finished match, its teams, score, stadium and Gameplay packs, as a line of the library's
//!   `matches.ron`, for a later leaderboard or online play to ask for the same rules.

use std::path::{Path, PathBuf};

use qwc_feck::memcard::{CardError, CardIo, DirCard, GAME_NAME};
use serde::{Deserialize, Serialize};

/// A Gameplay pack: its id and version.
pub type Rule = (String, String);

/// The Gameplay packs on, the lowest first.
pub fn rules() -> Vec<Rule> {
    qwc_packs::current().gameplay().map(|p| (p.manifest.id.clone(), p.manifest.version.clone())).collect()
}

/// The save's side file: the Gameplay packs its progress was made under.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveRules {
    pub rules: Vec<Rule>,
}

/// The side file of the save at `save`.
pub fn rules_path(save: &Path) -> PathBuf {
    let mut name = save.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".remaster.ron");
    save.with_file_name(name)
}

/// The side file's rules (none when it is not there or does not read).
pub fn read_rules(save: &Path) -> SaveRules {
    std::fs::read_to_string(rules_path(save)).ok().and_then(|t| ron::from_str(&t).ok()).unwrap_or_default()
}

/// Add `now` to the side file of the save at `save` (each pack once; a newer version of a pack is listed as well).
fn note_rules(save: &Path, now: &[Rule]) -> Result<(), String> {
    let mut r = read_rules(save);
    let before = r.rules.len();
    for rule in now {
        if !r.rules.contains(rule) {
            r.rules.push(rule.clone());
        }
    }
    if r.rules.len() == before && rules_path(save).exists() {
        return Ok(());
    }
    if r.rules.is_empty() {
        return Ok(());
    }
    let p = rules_path(save);
    let text = ron::ser::to_string_pretty(&r, ron::ser::PrettyConfig::new()).map_err(|e| e.to_string())? + "\n";
    std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))
}

/// The original's save file as the card, noting the Gameplay packs on in its side file after each write.
pub struct SaveCard {
    card: DirCard,
    rules: Vec<Rule>,
}

impl CardIo for SaveCard {
    fn free_bytes(&mut self) -> Option<u64> {
        self.card.free_bytes()
    }
    fn exists(&mut self) -> bool {
        self.card.exists()
    }
    fn read(&mut self, len: usize) -> Result<Vec<u8>, CardError> {
        self.card.read(len)
    }
    fn write(&mut self, data: &[u8]) -> Result<(), CardError> {
        self.card.write(data)?;
        if let Err(e) = note_rules(&self.card.file, &self.rules) {
            eprintln!("save rules: {e}");
        }
        Ok(())
    }
    fn delete(&mut self) -> bool {
        let gone = self.card.delete();
        if gone {
            let _ = std::fs::remove_file(rules_path(&self.card.file));
        }
        gone
    }
}

/// The card a windowed front end saves to: the original's file (`QWC_SAVE_DIR` names another folder for it), or
/// none where there is no documents folder.
pub fn save_card() -> Option<SaveCard> {
    let card = match std::env::var_os("QWC_SAVE_DIR") {
        Some(d) => DirCard::new(PathBuf::from(d), GAME_NAME),
        None => DirCard::documents(GAME_NAME)?,
    };
    Some(SaveCard { card, rules: rules() })
}

/// The key bindings' file (`qwc_feck::keys::FILE_NAME`) in the card's folder, beside the save, as the original keeps
/// it: the front end's Controls menu saves there, and the launcher's Settings edit it.
pub fn keys_file() -> Option<PathBuf> {
    save_card().map(|c| c.card.folder.join(qwc_feck::keys::FILE_NAME))
}

/// A finished match as the log keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Match")]
pub struct MatchRecord {
    /// When it ended, `YYYY-MM-DD HH-MM-SS` UTC.
    pub when: String,
    pub stadium: String,
    pub teams: [String; 2],
    pub score: [i64; 2],
    /// `quidd` for a match, else the challenge's folder.
    pub kind: String,
    pub rules: Vec<Rule>,
}

/// `YYYY-MM-DD HH-MM-SS` (UTC) now.
fn stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // the civil date of a day count (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}-{:02}-{:02}", rem / 3600, rem / 60 % 60, rem % 60)
}

/// Append a finished match to `log` (a line of RON each).
pub fn log_match(log: &Path, stadium: &str, teams: [&str; 2], score: [i64; 2], kind: &str) -> Result<(), String> {
    let r = MatchRecord {
        when: stamp(),
        stadium: stadium.to_string(),
        teams: teams.map(str::to_string),
        score,
        kind: kind.to_string(),
        rules: rules(),
    };
    let line = ron::to_string(&r).map_err(|e| e.to_string())? + "\n";
    if let Some(d) = log.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .map_err(|e| format!("{}: {e}", log.display()))
}

/// The library's match log, if there is a library.
pub fn match_log() -> Option<PathBuf> {
    qwc_packs::library_dir().map(|d| d.join("matches.ron"))
}

/// Every match in `log`, the oldest first (lines that do not read are skipped).
pub fn read_matches(log: &Path) -> Vec<MatchRecord> {
    std::fs::read_to_string(log).map(|t| t.lines().filter_map(|l| ron::from_str(l).ok()).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qwc-game-record-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_save_keeps_its_rules_beside_it() {
        let dir = scratch("save");
        let mut card = SaveCard { card: DirCard::new(&dir, GAME_NAME), rules: vec![] };
        card.write(b"save").unwrap();
        assert_eq!(std::fs::read(dir.join(GAME_NAME)).unwrap(), b"save");
        assert!(!rules_path(&dir.join(GAME_NAME)).exists(), "no Gameplay packs: no side file");
        card.rules = vec![("a.rules".into(), "1.0.0".into())];
        card.write(b"save2").unwrap();
        card.rules = vec![("a.rules".into(), "1.0.0".into()), ("b.more".into(), "0.2.0".into())];
        card.write(b"save3").unwrap();
        card.write(b"save4").unwrap();
        assert_eq!(read_rules(&dir.join(GAME_NAME)).rules.len(), 2);
        assert!(card.delete());
        assert!(!rules_path(&dir.join(GAME_NAME)).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn matches_are_lines_of_the_log() {
        let dir = scratch("log");
        let log = dir.join("matches.ron");
        log_match(&log, "hog_sum", ["Gryff", "Slyth"], [170, 40], "quidd").unwrap();
        log_match(&log, "england", ["England", "France"], [10, 160], "quidd").unwrap();
        let all = read_matches(&log);
        assert_eq!(all.len(), 2);
        assert_eq!((all[0].teams[0].as_str(), all[1].score), ("Gryff", [10, 160]));
        assert_eq!(all[0].when.len(), "2026-10-23 01-02-03".len());
        let _ = std::fs::remove_dir_all(dir);
    }
}
