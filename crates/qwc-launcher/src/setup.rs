//! The launcher's first-run setup, without a window: the player's unwrapped exe and game files checked, and the game
//! files imported into the remaster's library (`docs/LAUNCHER.md`, "Setup"). Not the original's.
//!
//! The known editions (`editions/*.txt`) are SHA-256 hashes of the game's files and exes, and the files' names: no
//! game bytes. A copy whose files all match one is "verified"; any copy whose required files are there and whose exe's
//! tables read is accepted, with what differs listed.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

/// The top-level entries of the game's folder the remaster reads, copied by the import (any case). The rest
/// (SafeDisc's files, the installer, DirectX, the shim's logs, the retail exe) stay behind.
pub const IMPORTED: &[&str] = &[
    "Audio",
    "frontend",
    "language",
    "match",
    "movies",
    "root",
    "stadium",
    "teams",
    "Locale",
    "datastr.ccd",
    "language.ccd",
];

/// The entries without which the game cannot start.
pub const REQUIRED: &[&str] =
    &["Audio", "frontend", "language", "match", "movies", "root", "stadium", "teams", "datastr.ccd", "language.ccd"];

/// The known editions' lists.
const EDITIONS: &[&str] = &[include_str!("../editions/usa.txt")];

/// A known edition: its hashes.
#[derive(Clone, Debug, Default)]
pub struct Edition {
    pub id: String,
    pub name: String,
    /// The unwrapped exes known to work, and the retail (copy-protected) ones.
    pub unwrapped: Vec<String>,
    pub retail: Vec<String>,
    /// Lower-case path (`/`) -> SHA-256 (lower-case hex).
    pub files: BTreeMap<String, String>,
}

impl Edition {
    /// A list: `edition ID NAME...`, `exe unwrapped|retail SHA`, then `SHA PATH` lines; `#` starts a comment.
    pub fn parse(text: &str) -> Result<Edition, String> {
        let mut e = Edition::default();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let bad = || format!("line {}: {line}", n + 1);
            let (head, rest) = line.split_once(' ').ok_or_else(bad)?;
            match head {
                "edition" => {
                    let (id, name) = rest.split_once(' ').ok_or_else(bad)?;
                    (e.id, e.name) = (id.to_string(), name.trim().to_string());
                }
                "exe" => match rest.split_once(' ') {
                    Some(("unwrapped", h)) => e.unwrapped.push(h.trim().to_ascii_lowercase()),
                    Some(("retail", h)) => e.retail.push(h.trim().to_ascii_lowercase()),
                    _ => return Err(bad()),
                },
                sha if sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()) => {
                    e.files.insert(normalize(rest.trim()), sha.to_ascii_lowercase());
                }
                _ => return Err(bad()),
            }
        }
        if e.id.is_empty() {
            return Err("no `edition` line".into());
        }
        Ok(e)
    }

    /// The speech languages the edition has (`audio/<language>/dat.dat`).
    fn languages(&self) -> Vec<String> {
        self.files.keys().filter_map(|p| speech_language(p).filter(|_| p.ends_with("/dat.dat"))).collect()
    }
}

/// The known editions.
pub fn editions() -> Vec<Edition> {
    EDITIONS.iter().map(|t| Edition::parse(t).expect("the shipped lists read")).collect()
}

/// A path as the lists hold it: lower case, `/`.
pub fn normalize(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches('/').to_ascii_lowercase()
}

/// The speech language a path is in (`audio/french/...` -> `french`), for the folders that hold speech.
fn speech_language(path: &str) -> Option<String> {
    let rest = path.strip_prefix("audio/")?;
    let (dir, _) = rest.split_once('/')?;
    (!["music", "spchcmn", "stadium", "teams", "cats"].contains(&dir)).then(|| dir.to_string())
}

/// SHA-256 of `bytes` as lower-case hex.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// How far a long job is: bytes done of the total, what it is on, and whether it was asked to stop.
#[derive(Debug, Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub what: Mutex<String>,
    pub cancel: AtomicBool,
}

impl Progress {
    /// Done, from 0 to 1.
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            0.0
        } else {
            self.done.load(Ordering::Relaxed) as f32 / total as f32
        }
    }

    fn on(&self, what: &str) {
        if let Ok(mut w) = self.what.lock() {
            what.clone_into(&mut w);
        }
    }

    fn cancelled(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) {
            Err("stopped".into())
        } else {
            Ok(())
        }
    }
}

/// The file's SHA-256, its bytes counted into `progress`.
fn hash_file(path: &Path, progress: &Progress) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        progress.done.fetch_add(n as u64, Ordering::Relaxed);
        progress.cancelled()?;
    }
    Ok(hex(&h.finalize()))
}

/// The entry of `dir` named `name` in any case.
pub fn find_entry(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(name)))
}

/// Every file under the imported entries of `dir`: (lower-case relative path, its path), sorted.
pub fn game_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for name in IMPORTED {
        let Some(top) = find_entry(dir, name) else { continue };
        let mut todo = vec![top];
        while let Some(p) = todo.pop() {
            if p.is_dir() {
                todo.extend(std::fs::read_dir(&p).into_iter().flatten().flatten().map(|e| e.path()));
            } else if let Ok(rel) = p.strip_prefix(dir) {
                out.push((normalize(&rel.to_string_lossy()), p));
            }
        }
    }
    out.sort();
    out
}

/// The SHA-256 of every imported file of `dir`, as an edition list's lines (`SHA PATH`).
pub fn hash_tree(dir: &Path, progress: &Progress) -> Result<Vec<(String, String)>, String> {
    let files = game_files(dir);
    progress.total.store(files.iter().map(|(_, p)| p.metadata().map_or(0, |m| m.len())).sum(), Ordering::Relaxed);
    let mut out = Vec::new();
    for (rel, p) in files {
        progress.on(&rel);
        out.push((rel, hash_file(&p, progress)?));
    }
    Ok(out)
}

/// What the exe the player gave is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExeVerdict {
    /// Unwrapped, its tables read, and a known one.
    Verified(String),
    /// Unwrapped and its tables read, but not one of the known editions'.
    Unwrapped,
    /// SafeDisc-wrapped (a retail exe): its code is encrypted.
    Protected,
    /// Not an exe of the game: why.
    NotTheGame(String),
}

impl ExeVerdict {
    pub fn usable(&self) -> bool {
        matches!(self, ExeVerdict::Verified(_) | ExeVerdict::Unwrapped)
    }

    /// What the launcher says about it.
    pub fn message(&self) -> String {
        match self {
            ExeVerdict::Verified(name) => format!("Verified: {name}."),
            ExeVerdict::Unwrapped => "Readable, but not an edition the remaster has been tested with.".into(),
            ExeVerdict::Protected => "This copy's QWC.exe is copy-protected; QWC Remastered needs an unprotected \
                                      QWC.exe from your own copy of the game."
                .into(),
            ExeVerdict::NotTheGame(why) => format!("This is not Quidditch World Cup's QWC.exe ({why})."),
        }
    }
}

/// An exe checked.
#[derive(Clone, Debug)]
pub struct ExeCheck {
    pub path: PathBuf,
    pub sha256: String,
    pub verdict: ExeVerdict,
    /// The load sets its table names.
    pub loadsets: usize,
}

/// Check the exe at `path`: SafeDisc's sections, the two tables the remaster reads from it, its hash.
pub fn check_exe(path: &Path) -> ExeCheck {
    let fail = |sha: String, why: String| ExeCheck {
        path: path.to_path_buf(),
        sha256: sha,
        verdict: ExeVerdict::NotTheGame(why),
        loadsets: 0,
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return fail(String::new(), e.to_string()),
    };
    let sha = hex(&Sha256::digest(&bytes));
    let known = editions();
    let check = |verdict, loadsets| ExeCheck { path: path.to_path_buf(), sha256: sha.clone(), verdict, loadsets };
    if known.iter().any(|e| e.retail.contains(&sha)) || has_safedisc(&bytes) {
        return check(ExeVerdict::Protected, 0);
    }
    let loadsets = match qwc_formats::flow::loadset_table(&bytes) {
        Ok(t) => t.len(),
        Err(e) => return fail(sha.clone(), format!("its load-set table does not read: {e}")),
    };
    if let Err(e) = qwc_formats::exe::card_numbers(&bytes) {
        return fail(sha.clone(), format!("its card table does not read: {e}"));
    }
    match known.iter().find(|e| e.unwrapped.contains(&sha)) {
        Some(e) => check(ExeVerdict::Verified(e.name.clone()), loadsets),
        None => check(ExeVerdict::Unwrapped, loadsets),
    }
}

/// Whether a PE has SafeDisc's sections (`stxt...`).
fn has_safedisc(pe: &[u8]) -> bool {
    let u16_at = |o: usize| pe.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |o: usize| pe.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let Some(nt) = u32_at(0x3c).map(|v| v as usize) else { return false };
    let (Some(count), Some(opt)) = (u16_at(nt + 6), u16_at(nt + 20)) else { return false };
    let first = nt + 24 + opt as usize;
    (0..count as usize).any(|i| pe.get(first + i * 40..first + i * 40 + 8).is_some_and(|n| n.starts_with(b"stxt")))
}

/// A game folder checked.
#[derive(Clone, Debug, Default)]
pub struct GameCheck {
    pub dir: PathBuf,
    /// Required entries not there: the game cannot be imported.
    pub missing_required: Vec<String>,
    /// The speech languages there (`Audio/<language>/dat.dat`), as the folders are named.
    pub languages: Vec<String>,
    /// The known edition it is closest to (none: no files match one), and whether every file matches it.
    pub edition: Option<(String, String)>,
    pub verified: bool,
    /// Against that edition: files that match, files that differ, files of it not there (its other languages
    /// apart), files it does not have.
    pub matching: usize,
    pub changed: Vec<String>,
    pub missing: Vec<String>,
    pub extra: Vec<String>,
    pub bytes: u64,
}

impl GameCheck {
    pub fn importable(&self) -> bool {
        self.missing_required.is_empty()
    }

    /// The library folder's name for it: the edition's id, else `custom`.
    pub fn folder(&self) -> String {
        self.edition.as_ref().map_or("custom".into(), |(id, _)| id.clone())
    }

    /// One line on what it is.
    pub fn summary(&self) -> String {
        if !self.importable() {
            return format!("Not the game's files: {} missing.", self.missing_required.join(", "));
        }
        let langs = if self.languages.is_empty() {
            "no speech".into()
        } else {
            format!("speech in {}", self.languages.join(", "))
        };
        match (&self.edition, self.verified) {
            (Some((_, name)), true) => format!("Verified: {name}. {} files, {langs}.", self.matching),
            (Some((_, name)), false) => format!(
                "{name}, with differences: {} files match, {} differ, {} missing, {} not in it; {langs}.",
                self.matching,
                self.changed.len(),
                self.missing.len(),
                self.extra.len()
            ),
            (None, _) => format!("An edition the remaster has not been tested with; {langs}."),
        }
    }
}

/// Check the game folder `dir`: the required entries, the speech languages, every file against the known editions.
pub fn check_game(dir: &Path, progress: &Progress) -> Result<GameCheck, String> {
    let mut c = GameCheck { dir: dir.to_path_buf(), ..GameCheck::default() };
    c.missing_required = REQUIRED.iter().filter(|n| find_entry(dir, n).is_none()).map(|n| n.to_string()).collect();
    if !c.importable() {
        return Ok(c);
    }
    let hashes: BTreeMap<String, String> = hash_tree(dir, progress)?.into_iter().collect();
    c.bytes = progress.total.load(Ordering::Relaxed);
    let mut langs: Vec<String> =
        hashes.keys().filter(|p| p.ends_with("/dat.dat")).filter_map(|p| speech_language(p)).collect();
    langs.dedup();
    if let Some(audio) = find_entry(dir, "Audio") {
        // the folders' own names, for showing
        c.languages = langs
            .iter()
            .map(|l| find_entry(&audio, l).and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned())))
            .map(|n| n.unwrap_or_default())
            .collect();
    }
    let best = editions()
        .into_iter()
        .map(|e| {
            // its files, but for the speech and locales of languages this copy does not have
            let absent: Vec<String> = e.languages().into_iter().filter(|l| !langs.contains(l)).collect();
            let optional = |p: &str| {
                speech_language(p).is_some_and(|l| absent.contains(&l))
                    || (p.starts_with("locale/") && !hashes.contains_key(p))
            };
            let wanted: Vec<(&String, &String)> = e.files.iter().filter(|(p, _)| !optional(p)).collect();
            let matching = wanted.iter().filter(|(p, h)| hashes.get(*p) == Some(h)).count();
            (matching, wanted.len(), e)
        })
        .max_by_key(|(m, _, _)| *m);
    if let Some((matching, wanted, e)) = best.filter(|(m, _, _)| *m > 0) {
        c.matching = matching;
        for (p, h) in &e.files {
            match hashes.get(p) {
                Some(mine) if mine != h => c.changed.push(p.clone()),
                // a language's speech or locale the copy does not have is not missing: an install has its own only
                None if speech_language(p).is_none_or(|l| langs.contains(&l)) && !p.starts_with("locale/") => {
                    c.missing.push(p.clone())
                }
                _ => {}
            }
        }
        c.extra = hashes.keys().filter(|p| !e.files.contains_key(*p)).cloned().collect();
        c.verified = matching == wanted && c.extra.is_empty();
        c.edition = Some((e.id, e.name));
    } else {
        c.extra = hashes.into_keys().collect();
    }
    Ok(c)
}

/// Where an import puts the game: `library/game/<edition>`.
pub fn game_dir(library: &Path, check: &GameCheck) -> PathBuf {
    library.join("game").join(check.folder())
}

/// Copy the checked game files and the unwrapped exe into the library (`game/<edition>/`, the exe as its
/// `QWC.exe`), replacing an earlier import, and point the profile at them. Answers the profile's path.
pub fn import(game: &GameCheck, exe: &ExeCheck, library: &Path, progress: &Progress) -> Result<PathBuf, String> {
    if !game.importable() {
        return Err(game.summary());
    }
    if !exe.verdict.usable() {
        return Err(exe.verdict.message());
    }
    let dest = game_dir(library, game);
    let parent = dest.parent().expect("under the library");
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    // copied beside, then swapped in, so a stopped or failed import leaves the earlier one as it was
    let fresh = parent.join(format!(".{}.importing", game.folder()));
    let _ = std::fs::remove_dir_all(&fresh);
    let copy = || -> Result<(), String> {
        let files = game_files(&game.dir);
        progress.done.store(0, Ordering::Relaxed);
        progress.total.store(files.iter().map(|(_, p)| p.metadata().map_or(0, |m| m.len())).sum(), Ordering::Relaxed);
        for (_, p) in files {
            let rel = p.strip_prefix(&game.dir).expect("under the game folder");
            progress.on(&rel.to_string_lossy());
            let to = fresh.join(rel);
            if let Some(d) = to.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            let n = std::fs::copy(&p, &to).map_err(|e| format!("{}: {e}", p.display()))?;
            progress.done.fetch_add(n, Ordering::Relaxed);
            progress.cancelled()?;
        }
        std::fs::copy(&exe.path, fresh.join("QWC.exe")).map_err(|e| format!("{}: {e}", exe.path.display()))?;
        Ok(())
    };
    if let Err(e) = copy() {
        let _ = std::fs::remove_dir_all(&fresh);
        return Err(e);
    }
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    std::fs::rename(&fresh, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let path = library.join("profile.ron");
    let mut profile = if path.is_file() { qwc_packs::Profile::read(&path)? } else { qwc_packs::Profile::default() };
    profile.data = Some(dest.clone());
    profile.exe = Some(dest.join("QWC.exe"));
    profile.write(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qwc-launcher-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_shipped_editions_read() {
        let e = editions();
        assert!(!e.is_empty());
        for ed in &e {
            assert!(!ed.unwrapped.is_empty() && !ed.files.is_empty(), "{}", ed.id);
        }
    }

    #[test]
    fn edition_lists() {
        let text = format!(
            "# a list\nedition usa USA disc\nexe unwrapped {a}\nexe retail {b}\n{a} Audio/English/dat.dat\n{b} \
             audio/french/dat.dat\n{a} Audio/Music/x.mus\n{b} frontend\\Common.ccd\n",
            a = "a".repeat(64),
            b = "B".repeat(64)
        );
        let e = Edition::parse(&text).unwrap();
        assert_eq!((e.id.as_str(), e.name.as_str()), ("usa", "USA disc"));
        assert_eq!(e.retail, ["b".repeat(64)]);
        assert_eq!(e.files.get("frontend/common.ccd"), Some(&"b".repeat(64)));
        assert_eq!(e.languages(), ["english", "french"]);
        assert!(Edition::parse("exe unwrapped x\n").is_err());
        assert!(Edition::parse("edition a A\nnot a line\n").is_err());
    }

    #[test]
    fn a_folder_without_the_game_is_refused() {
        let d = scratch("notgame");
        std::fs::write(d.join("datastr.ccd"), "x").unwrap();
        let c = check_game(&d, &Progress::default()).unwrap();
        assert!(!c.importable());
        assert!(
            c.missing_required.contains(&"frontend".to_string()) && !c.missing_required.contains(&"datastr.ccd".into())
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn safedisc_sections() {
        // a PE header with two sections, the second SafeDisc's
        let mut pe = vec![0u8; 0x200];
        pe[0x3c] = 0x80;
        pe[0x80 + 6] = 2;
        pe[0x80 + 20] = 0xe0;
        let first = 0x80 + 24 + 0xe0;
        pe[first..first + 5].copy_from_slice(b".text");
        pe[first + 40..first + 47].copy_from_slice(b"stxt371");
        assert!(has_safedisc(&pe));
        pe[first + 40..first + 47].copy_from_slice(b".rsrc\0\0");
        assert!(!has_safedisc(&pe));
        assert!(!has_safedisc(b"MZ"));
    }

    #[test]
    fn a_file_that_is_not_an_exe() {
        let d = scratch("notexe");
        std::fs::write(d.join("QWC.exe"), vec![0u8; 1000]).unwrap();
        let c = check_exe(&d.join("QWC.exe"));
        assert!(matches!(c.verdict, ExeVerdict::NotTheGame(_)), "{:?}", c.verdict);
        assert!(!c.verdict.usable());
        let _ = std::fs::remove_dir_all(d);
    }

    /// With the game data (`QWC_DATA_DIR`, the repo's root): the disc and the unwrapped exe are the verified USA
    /// edition, the retail exe is refused, and an import lands in the library with the profile pointing at it.
    #[test]
    fn real_data() {
        let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else { return };
        let disc = root.join("original/cd_files");
        let exe = check_exe(&root.join("analysis/QWC.exe"));
        assert!(matches!(exe.verdict, ExeVerdict::Verified(_)), "{:?}", exe.verdict);
        assert!(exe.loadsets > 0);
        assert_eq!(check_exe(&disc.join("QWC.exe")).verdict, ExeVerdict::Protected);
        let game = check_game(&disc, &Progress::default()).unwrap();
        assert!(game.verified, "{}", game.summary());
        assert_eq!(game.languages.len(), 3);
        let library = scratch("library");
        let profile = import(&game, &exe, &library, &Progress::default()).unwrap();
        let p = qwc_packs::Profile::read(&profile).unwrap();
        assert_eq!(p.data, Some(library.join("game/usa")));
        assert!(library.join("game/usa/QWC.exe").is_file() && library.join("game/usa/datastr.ccd").is_file());
        assert!(!library.join("game/usa/SECDRV.SYS").exists() && !library.join("game/usa/setup.exe").exists());
        let _ = std::fs::remove_dir_all(library);
    }
}
