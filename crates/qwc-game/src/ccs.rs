//! The crowd cutscene tables (`root/CCSCLI/CSCrowd.cli`, `CutSceneMgr`'s `CCS.*` commands registered at
//! `0x48a0e0`): which cutscenes a match plays and who is in them.
//!
//! **[T]** (`RE_NOTES.md`, "Match cutscenes"):
//! - `CCS.AddCCSItem CODE NAME TYPE`: a cutscene code (`CS01`, `IN02`, `SN01`, `OT01`, ...) and its folder of
//!   `root/cs`; `CCS.AddCCSCharacterItem CODE MODEL SIZE`: a character and its model (`root/csmodels`);
//!   `CCS.AddCCSRefereeItem NATION CODE`: the referee's character for a nationality;
//! - `CCS.AddCCSCMatchItem` (`0x489e70` → `0x48a550`): nineteen fields (`0x48b920`): the match type, the two teams
//!   and the location (the keys), the referee's nationality, two commentators, two supporters, the crowd cutscenes
//!   for a goal by team 0 and by team 1 (`CCSConcede`, `CCSScore`), two intros, four catches and two outros, a
//!   blank field none. A record is dropped when an earlier one has its keys, or when a cutscene field is not an item
//!   (`0x48bb20`);
//! - the match's record (`0x48ae00` with `0x48ace0` in its wildcard mode, from `0x432d30` when the match starts and
//!   it is not a challenge): the first whose keys match, `*` matching anything. The match type is `H`, `P`, `E`,
//!   `W` or `Q` for DataStore `MatchType` 1 to 5 (anything else is 1, `0x48b570`), the teams' codes are `0x48b210`'s,
//!   the location `0x48b480`'s from `StadiumLoadSet`;
//! - a pick among a kind's fields (`0x48b5d0`, `0x48bd70`, `0x48bdf0`): `n` the fields set, the field
//!   `trunc(rand() * n / 32767)` when `n > 1` (the first otherwise), counted from the kind's first field;
//! - the outro (`0x48b6a0`): the second for a winner from France, England, Hufflepuff, the Nordic team, Ravenclaw or
//!   Spain (team ids 1, 2, 5, 7, 8 and 11), the first for the others.
//!
//! **[I]**: the team folders' codes (the game's team ids are not traced to the folders); the Hogwarts cup winner's
//! characters (`HG1`, `HG2`) are Gryffindor's (the winner's are not read); `rand() = 32767` picks the last field
//! (the original's index runs one past it).

use qwc_assets::Vfs;
use qwc_sim::rng::Rand;

/// Where the tables are.
pub const PATH: &str = "root/CCSCLI/CSCrowd.cli";

/// The kinds of cutscene a record names, with its fields' range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A crowd cutscene after team 0's goal (`CCSConcede`).
    GoalTeam0,
    /// A crowd cutscene after team 1's goal (`CCSScore`).
    GoalTeam1,
    Intro,
    Catch,
    Outro,
}

impl Kind {
    fn fields(self) -> std::ops::Range<usize> {
        match self {
            Kind::GoalTeam0 => 9..10,
            Kind::GoalTeam1 => 10..11,
            Kind::Intro => 11..13,
            Kind::Catch => 13..17,
            Kind::Outro => 17..19,
        }
    }
}

/// One `AddCCSCMatchItem`: the nineteen fields, blank ones empty.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub fields: [String; 19],
}

impl Record {
    /// The codes of a kind, blank ones included.
    fn slots(&self, kind: Kind) -> &[String] {
        &self.fields[kind.fields()]
    }

    /// How many of the kind's fields are set (`0x48bdf0`).
    pub fn count(&self, kind: Kind) -> usize {
        self.slots(kind).iter().filter(|c| !c.is_empty()).count()
    }

    /// The code `0x48b5d0` picks for the kind, drawing from `rng` when there is a choice.
    pub fn pick(&self, kind: Kind, rng: &mut Rand) -> Option<&str> {
        let n = self.count(kind);
        let i = if n > 1 { (rng.rand() as f64 * n as f64 * (1.0 / 32767.0)) as usize } else { 0 };
        let code = &self.slots(kind)[i.min(n.saturating_sub(1))];
        (!code.is_empty()).then_some(code.as_str())
    }

    /// The outro's code for a winner of team code `winner` (`0x48b6a0`).
    pub fn outro(&self, winner: &str) -> Option<&str> {
        let want = usize::from(matches!(winner, "FRA" | "ENG" | "HUF" | "NDK" | "RAV" | "SPA"));
        let n = self.count(Kind::Outro);
        let i = if n < want { n.saturating_sub(1) } else { want };
        let code = &self.slots(Kind::Outro)[i];
        (!code.is_empty()).then_some(code.as_str())
    }
}

/// The tables.
#[derive(Debug, Default, Clone)]
pub struct Tables {
    /// Cutscene code, folder of `root/cs`, type.
    items: Vec<(String, String, String)>,
    /// Character code, model (a folder of `root/csmodels`), skeleton size.
    characters: Vec<(String, String, String)>,
    /// Nationality, the referee's character code.
    referees: Vec<(String, String)>,
    pub records: Vec<Record>,
}

/// A command's arguments: quoted strings or bare words, trimmed.
fn args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        if let Some(q) = rest.strip_prefix('"') {
            let end = q.find('"').unwrap_or(q.len());
            out.push(q[..end].trim().to_string());
            rest = q.get(end + 1..).unwrap_or("").trim_start();
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            out.push(rest[..end].to_string());
            rest = rest[end..].trim_start();
        }
    }
    out
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl Tables {
    /// The tables a `CSCrowd.cli` builds.
    pub fn parse(text: &str) -> Tables {
        let mut t = Tables::default();
        let (lines, _) = qwc_formats::text_defs::parse_cli(text);
        for (cmd, rest) in lines {
            let a = args(rest.as_deref().unwrap_or(""));
            let arg = |i: usize| a.get(i).cloned().unwrap_or_default();
            match cmd.to_ascii_lowercase().as_str() {
                "ccs.clearalltables" => t = Tables::default(),
                "ccs.addccsitem" => t.items.push((arg(0), arg(1), arg(2))),
                "ccs.addccscharacteritem" => t.characters.push((arg(0), arg(1), arg(2))),
                "ccs.addccsrefereeitem" => t.referees.push((arg(0), arg(1))),
                "ccs.addccscmatchitem" => {
                    let r = Record { fields: std::array::from_fn(arg) };
                    let dup = t.records.iter().any(|o| (0..4).all(|k| same(&o.fields[k], &r.fields[k])));
                    let known = |c: &String| c.is_empty() || t.items.iter().any(|(code, _, _)| same(code, c));
                    if !dup && r.fields[9..].iter().all(known) {
                        t.records.push(r);
                    }
                }
                _ => {}
            }
        }
        t
    }

    /// The disc's tables, or none (said why).
    pub fn load(vfs: &Vfs) -> Option<Tables> {
        let bytes = vfs.read(PATH).map_err(|e| eprintln!("{PATH}: {e}")).ok()?;
        Some(Tables::parse(&String::from_utf8_lossy(&bytes)))
    }

    /// The record for a match (`0x48ace0`, wildcard mode): its type letter, its teams' codes and its location's.
    pub fn find(&self, mode: &str, teams: [&str; 2], location: &str) -> Option<usize> {
        let keys = [mode, teams[0], teams[1], location];
        self.records.iter().position(|r| (0..4).all(|k| r.fields[k] == "*" || same(&r.fields[k], keys[k])))
    }

    /// The folder of `root/cs` a cutscene code names.
    pub fn cutscene(&self, code: &str) -> Option<&str> {
        self.items.iter().find(|(c, _, _)| same(c, code)).map(|(_, n, _)| n.as_str())
    }

    /// The model a character code names (none for `NA`); the cup winner's `HG1` and `HG2` are Gryffindor's.
    pub fn model(&self, code: &str) -> Option<&str> {
        let code = match code.to_ascii_uppercase().as_str() {
            "HG1" => "CG1",
            "HG2" => "CG2",
            _ => code,
        };
        let m = self.characters.iter().find(|(c, _, _)| same(c, code)).map(|(_, m, _)| m.as_str())?;
        (!same(m, "NA")).then_some(m)
    }

    /// Who a record puts in its cutscenes: the referee (by its nationality's character), the commentators and the
    /// supporters.
    pub fn cast(&self, r: &Record) -> crate::cutscene::Cast {
        let model = |code: &str| self.model(code).map(str::to_string);
        let referee = self.referees.iter().find(|(n, _)| same(n, &r.fields[4])).and_then(|(_, c)| model(c));
        crate::cutscene::Cast {
            referee,
            commentators: [model(&r.fields[5]), model(&r.fields[6])],
            supporters: [model(&r.fields[7]), model(&r.fields[8])],
            own: None,
            other_own: None,
        }
    }
}

/// A team folder's code (`0x48b210`'s codes; **[I]** the folders they stand for).
pub fn team_code(folder: &str) -> &'static str {
    match folder.to_ascii_lowercase().as_str() {
        "aus" => "AUS",
        "france" => "FRA",
        "england" => "ENG",
        "germany" => "GER",
        "gryff" => "GRY",
        "huffle" => "HUF",
        "japan" => "JPN",
        "nordic" => "NDK",
        "raven" => "RAV",
        "slyth" => "SLY",
        "usa" => "USA",
        "spain" => "SPA",
        "bulgaria" => "BUL",
        "edgarclo" => "EDG",
        _ => "",
    }
}

/// The location's code for a `StadiumLoadSet` value (`0x48b480`).
pub fn location_code(stadium_load_set: u32) -> &'static str {
    match stadium_load_set {
        2 | 5 | 9 => "HGS",
        3 | 7 => "HGW",
        0xb => "ENG",
        0xd => "USA",
        0xf => "JPN",
        0x11 => "GER",
        0x13 => "FRA",
        0x15 => "AUS",
        0x17 => "NDK",
        0x19 => "SPA",
        0x1b => "BUL",
        _ => "XXX",
    }
}

/// The location's code for a stadium folder (**[I]**: `0x48b480`'s codes for the folders they stand for).
pub fn location_of_folder(folder: &str) -> &'static str {
    match folder.to_ascii_lowercase().as_str() {
        "hog_sum" => "HGS",
        "hog_win" => "HGW",
        "england" => "ENG",
        "usa" => "USA",
        "japan" => "JPN",
        "germany" => "GER",
        "france" => "FRA",
        "aus" => "AUS",
        "nordic" => "NDK",
        "spain" => "SPA",
        "bulgaria" => "BUL",
        _ => "XXX",
    }
}

/// The match type's letter for DataStore `MatchType` (`0x432d30`, `0x48b570`).
pub fn mode_letter(match_type: u32) -> &'static str {
    match match_type {
        2 => "P",
        3 => "E",
        4 => "W",
        5 => "Q",
        _ => "H",
    }
}

/// A challenge's intro (`0x45ac60`, by the match description's mode `[0x1018f98]`).
pub fn challenge_intro(kind: u32) -> Option<&'static str> {
    match kind {
        1 => Some("chpass"),
        2 | 5 => Some("chedgar"),
        3 => Some("chseek"),
        4 => Some("chbeat"),
        6 => Some("chcomb"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLES: &str = r#"
CCS.ClearAllTables
CCS.AddCCSItem "AA01"    "intro_one" "X"
CCS.AddCCSItem "AA02"    "intro_two" "X"
CCS.AddCCSItem "BB01"    "Outro_One" "X"
CCS.AddCCSItem "BB02"    "outro_two" "X"
CCS.AddCCSCharacterItem "CMA"       "comm_a" "MED"
CCS.AddCCSCharacterItem "RFA"       "Ref_A" "MED"
CCS.AddCCSCharacterItem "NA"        "NA" 	"NA"
CCS.AddCCSRefereeItem "ENG"       "RFA"
#CCS.AddCCSCMatchItem "E" "*" "*" "*" "ENG" "CMA" "CMA" "NA" "NA" " " " " "AA01" " " " " " " " " " " "BB01" " "
CCS.AddCCSCMatchItem   "E"  "ENG" "*"  "*"  "ENG" "CMA" "NA" "NA" "NA" " " " " "AA01" "AA02" " " " " " " " " "BB01" "BB02"
CCS.AddCCSCMatchItem   "E"  "ENG" "*"  "*"  "ENG" "CMA" "NA" "NA" "NA" " " " " "AA02" " " " " " " " " " " "BB01" " "
CCS.AddCCSCMatchItem   "E"  "*"   "*"  "*"  "ENG" "CMA" "NA" "NA" "NA" "ZZ99" " " "AA02" " " " " " " " " " " "BB01" " "
CCS.AddCCSCMatchItem   "*"  "*"   "*"  "*"  "ENG" "NA" "NA" "NA" "NA" " " " " "AA02" " " " " " " " " " " "BB01" " "
"#;

    #[test]
    fn records_match_with_wildcards_in_order() {
        let t = Tables::parse(TABLES);
        // the duplicate keys and the unknown cutscene code are dropped, the comment never ran
        assert_eq!(t.records.len(), 2);
        assert_eq!(t.find("E", ["ENG", "FRA"], "XXX"), Some(0));
        assert_eq!(t.find("E", ["FRA", "ENG"], "XXX"), Some(1));
        assert_eq!(t.find("W", ["FRA", "ENG"], "XXX"), Some(1));
        let r = &t.records[0];
        assert_eq!(r.count(Kind::Intro), 2);
        assert_eq!(r.count(Kind::Catch), 0);
        assert_eq!(t.cutscene(r.outro("ENG").unwrap()), Some("outro_two"));
        assert_eq!(t.cutscene(r.outro("GER").unwrap()), Some("Outro_One"));
        // one outro: the second is asked for, and is blank
        assert_eq!(t.records[1].outro("FRA"), None);
        assert_eq!(t.records[1].outro("GER"), Some("BB01"));
        let cast = t.cast(r);
        assert_eq!(cast.referee.as_deref(), Some("Ref_A"));
        assert_eq!(cast.commentators, [Some("comm_a".into()), None]);
    }

    #[test]
    fn a_pick_draws_only_among_several() {
        let t = Tables::parse(TABLES);
        let mut rng = Rand::new(1);
        let before = rng.clone();
        assert_eq!(t.records[1].pick(Kind::Intro, &mut rng), Some("AA02"));
        assert_eq!(rng, before);
        let picks: Vec<_> = (0..20).filter_map(|_| t.records[0].pick(Kind::Intro, &mut rng)).collect();
        assert!(picks.contains(&"AA01") && picks.contains(&"AA02"));
    }
}
