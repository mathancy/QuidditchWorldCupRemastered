//! The game's save, as the launcher's save manager sees it: the memory-card file the original and the port share
//! (`<Documents>\EA Games Saves\Harry Potter - Quidditch WC\Harry Potter - Quidditch WC`, `qwc_feck::memcard`), its
//! slots' summaries read from the header (`docs/FECK_VM.md`, "The save buffer"), and copies of it kept in the
//! library's `backups/` folder.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The save's folder under the documents folder, and its file's name (`0x5ab274`, `0x5cf86c`).
const SAVE_FOLDER: &str = "EA Games Saves";
const GAME_NAME: &str = "Harry Potter - Quidditch WC";
/// The header's size, and the CRC's seed (`0x4eb240`).
const HEADER: usize = 0x14;
const CRC_SEED: u32 = 0x9ca1f2ca;
/// Automatic backups kept (made before each Play); the player's own are kept until deleted.
pub const AUTO_KEPT: usize = 5;

/// The save file: `<Documents>\EA Games Saves\<game>\<game>` (`$HOME/Documents` elsewhere, as the port does).
pub fn save_path() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(Path::new(&home).join("Documents").join(SAVE_FOLDER).join(GAME_NAME).join(GAME_NAME))
}

/// A slot's summary (its 8 bytes). The meanings are the DataStore entries the game copies into it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Slot {
    pub used: bool,
    /// `0xa20daf5d`, the Quidditch cards collected (`dAutosave_Summary_NN_QCards`).
    pub cards: u8,
    /// `0x9dc37226`, `0xe88f0701` (stored plus 1), `0x136ea64f`: the house and its cup (`_House`, `_HousePosition`,
    /// `_HouseCup`).
    pub house: u8,
    pub house_position: u8,
    pub house_cup: u8,
    /// `0x0acab0ef`, `0x80425ea5`: the World Cup (`_Exhibition`, `_ExhibitionPosition`).
    pub world_cup: u8,
    pub world_cup_position: u8,
}

/// The save file read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Save {
    pub bytes: usize,
    /// Whether the CRC holds (a save the game would load).
    pub intact: bool,
    pub current: u32,
    pub slots: Vec<Slot>,
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

impl Save {
    /// A save's bytes: the header's slot count and size, each slot's summary, the CRC checked.
    pub fn parse(b: &[u8]) -> Result<Save, String> {
        let (Some(crc), Some(count), Some(size), Some(current)) =
            (u32_at(b, 0), u32_at(b, 4), u32_at(b, 0xc), u32_at(b, 0x10))
        else {
            return Err("too short for a save".into());
        };
        if count == 0 || count > 16 {
            return Err(format!("{count} slots: not a save"));
        }
        let stride = size as usize + 8;
        if HEADER + stride * count as usize > b.len() {
            return Err("shorter than its slots".into());
        }
        let slots = (0..count as usize)
            .map(|i| {
                let s = &b[HEADER + i * stride..HEADER + i * stride + 8];
                Slot {
                    used: s[0] != 0,
                    cards: s[1],
                    house: s[2],
                    house_position: s[3],
                    house_cup: s[4],
                    world_cup: s[5],
                    world_cup_position: s[6],
                }
            })
            .collect();
        let intact = qwc_formats::flow::name_hash_seed(&b[4..], CRC_SEED) == crc;
        Ok(Save { bytes: b.len(), intact, current, slots })
    }
}

/// The Gameplay packs (id, version) the save's progress was made under: its side file
/// (`<save>.remaster.ron`, written by the game, `qwc-game`'s `record`); none without one.
pub fn save_rules(save: &Path) -> Vec<(String, String)> {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct SaveRules {
        rules: Vec<(String, String)>,
    }
    std::fs::read_to_string(rules_file(save))
        .ok()
        .and_then(|t| ron::from_str::<SaveRules>(&t).ok())
        .map(|r| r.rules)
        .unwrap_or_default()
}

/// A match the game logged (the library's `matches.ron`, a line each).
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(rename = "Match")]
pub struct Played {
    pub when: String,
    pub stadium: String,
    pub teams: [String; 2],
    pub score: [i64; 2],
    pub kind: String,
    pub rules: Vec<(String, String)>,
}

/// The newest `n` matches of the log at `log`, the newest first.
pub fn recent_matches(log: &Path, n: usize) -> Vec<Played> {
    let mut all: Vec<Played> = std::fs::read_to_string(log)
        .map(|t| t.lines().filter_map(|l| ron::from_str(l).ok()).collect())
        .unwrap_or_default();
    all.reverse();
    all.truncate(n);
    all
}

/// A copy of the save in the backups folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backup {
    pub path: PathBuf,
    /// When it was made (the folder's name: `YYYY-MM-DD HH-MM-SS`, UTC) and whether it was made before a Play.
    pub when: String,
    pub auto: bool,
}

/// `YYYY-MM-DD HH-MM-SS` (UTC) of `t`.
pub fn stamp(t: SystemTime) -> String {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
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

/// The backups in `dir`, the newest first.
pub fn backups(dir: &Path) -> Vec<Backup> {
    let mut out: Vec<Backup> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join(GAME_NAME).is_file())
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let auto = name.ends_with(" before playing");
            Backup { path: e.path().join(GAME_NAME), when: name.trim_end_matches(" before playing").to_string(), auto }
        })
        .collect();
    out.sort_by(|a, b| b.when.cmp(&a.when));
    out
}

/// Copy the save at `save` into `dir/<stamp>[ before playing]/`; with `auto`, keep only the newest [`AUTO_KEPT`]
/// automatic ones. Answers the copy.
pub fn back_up(save: &Path, dir: &Path, auto: bool) -> Result<PathBuf, String> {
    let name = format!("{}{}", stamp(SystemTime::now()), if auto { " before playing" } else { "" });
    let to = dir.join(name).join(GAME_NAME);
    std::fs::create_dir_all(to.parent().expect("a folder")).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::copy(save, &to).map_err(|e| format!("{}: {e}", save.display()))?;
    // its side file goes with it (the Gameplay packs its progress was made under)
    if rules_file(save).is_file() {
        std::fs::copy(rules_file(save), rules_file(&to)).map_err(|e| format!("{}: {e}", save.display()))?;
    }
    if auto {
        for old in backups(dir).into_iter().filter(|b| b.auto).skip(AUTO_KEPT) {
            if let Some(d) = old.path.parent() {
                let _ = std::fs::remove_dir_all(d);
            }
        }
    }
    Ok(to)
}

/// Put `backup` back as the save at `save`, after backing the current one up (when there is one).
pub fn restore(backup: &Path, save: &Path, dir: &Path) -> Result<(), String> {
    Save::parse(&std::fs::read(backup).map_err(|e| format!("{}: {e}", backup.display()))?)?;
    if save.is_file() {
        back_up(save, dir, false)?;
    }
    if let Some(d) = save.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::copy(backup, save).map_err(|e| format!("{}: {e}", save.display()))?;
    // the side file as the backup had it: copied back, or gone when it had none
    if rules_file(backup).is_file() {
        std::fs::copy(rules_file(backup), rules_file(save)).map_err(|e| format!("{}: {e}", save.display()))?;
    } else {
        let _ = std::fs::remove_file(rules_file(save));
    }
    Ok(())
}

/// The save's side file (`<save>.remaster.ron`).
fn rules_file(save: &Path) -> PathBuf {
    let mut name = save.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".remaster.ron");
    save.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A save of `slots` slots of `size` bytes, slot 1 used with 12 cards; its CRC set.
    fn save(slots: u32, size: u32) -> Vec<u8> {
        let mut b = vec![0u8; HEADER + (size as usize + 8) * slots as usize];
        b[4..8].copy_from_slice(&slots.to_le_bytes());
        b[0xc..0x10].copy_from_slice(&size.to_le_bytes());
        b[0x10..0x14].copy_from_slice(&1u32.to_le_bytes());
        let s1 = HEADER + size as usize + 8;
        b[s1..s1 + 8].copy_from_slice(&[1, 12, 2, 1, 3, 4, 2, 1]);
        let crc = qwc_formats::flow::name_hash_seed(&b[4..], CRC_SEED);
        b[0..4].copy_from_slice(&crc.to_le_bytes());
        b
    }

    #[test]
    fn reads_the_summaries_and_checks_the_crc() {
        let b = save(4, 100);
        let s = Save::parse(&b).unwrap();
        assert!(s.intact);
        assert_eq!((s.slots.len(), s.current), (4, 1));
        assert!(!s.slots[0].used);
        assert_eq!(
            s.slots[1],
            Slot {
                used: true,
                cards: 12,
                house: 2,
                house_position: 1,
                house_cup: 3,
                world_cup: 4,
                world_cup_position: 2
            }
        );
        let mut bad = b.clone();
        bad[40] ^= 1;
        assert!(!Save::parse(&bad).unwrap().intact);
        assert!(Save::parse(&b[..30]).is_err());
        assert!(Save::parse(&[0u8; 64]).is_err());
    }

    #[test]
    fn the_games_records() {
        let root = std::env::temp_dir().join(format!("qwc-launcher-records-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let save = root.join(GAME_NAME);
        assert!(save_rules(&save).is_empty());
        std::fs::write(root.join(format!("{GAME_NAME}.remaster.ron")), "(rules: [(\"a.rules\", \"1.0.0\")])").unwrap();
        assert_eq!(save_rules(&save), [("a.rules".to_string(), "1.0.0".to_string())]);
        let log = root.join("matches.ron");
        let line = |s: i64| {
            format!("Match(when:\"2026-10-23 0{s}-00-00\",stadium:\"hog_sum\",teams:(\"Gryff\",\"Slyth\"),score:({s},0),kind:\"quidd\",rules:[])\n")
        };
        std::fs::write(&log, line(1) + &line(2) + "not a match\n" + &line(3)).unwrap();
        let recent = recent_matches(&log, 2);
        assert_eq!(recent.iter().map(|m| m.score[0]).collect::<Vec<_>>(), [3, 2]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stamps() {
        assert_eq!(stamp(SystemTime::UNIX_EPOCH), "1970-01-01 00-00-00");
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_792_713_600 + 3723);
        assert_eq!(stamp(t), "2026-10-23 01-02-03");
    }

    #[test]
    fn backs_up_keeps_five_automatic_and_restores() {
        let root = std::env::temp_dir().join(format!("qwc-launcher-saves-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (saves, dir) = (root.join("Documents"), root.join("backups"));
        std::fs::create_dir_all(&saves).unwrap();
        let file = saves.join(GAME_NAME);
        std::fs::write(&file, save(4, 16)).unwrap();
        // names made by hand: the clock's seconds would collide
        for i in 0..7 {
            let d = dir.join(format!("2026-01-0{} 00-00-00 before playing", i + 1));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::copy(&file, d.join(GAME_NAME)).unwrap();
        }
        back_up(&file, &dir, true).unwrap();
        let all = backups(&dir);
        assert_eq!(all.iter().filter(|b| b.auto).count(), AUTO_KEPT);
        assert!(all[0].when.as_str() > "2026-01-07");
        // a restore keeps the save it replaces
        std::fs::write(&file, save(4, 32)).unwrap();
        let before = backups(&dir).len();
        restore(&all[1].path, &file, &dir).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), save(4, 16));
        assert_eq!(backups(&dir).len(), before + 1);
        let _ = std::fs::remove_dir_all(root);
    }
}
