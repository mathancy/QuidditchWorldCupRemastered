//! Formotion databases (`match/<mode>/formotionDB.txt` and `teams/<country>/formotionDB.txt`; loaded with the
//! match description near `0x4d0b4f`, keyword table `0x5f2720`-`0x5f281c`).
//!
//! A FORMOTION is a short scripted path for a chaser off the ball: up to a few waypoints at times `0..1` of
//! `timesteps` ticks, with positions and velocities in a unit frame that the game scales and places relative to
//! the play (`docs/GAMEPLAY.md`, "Chasers off the ball: Formotions"). A TRANSFERLIST names, for one of the 17
//! formation states (`eFT_Free` ... `eFT_BludgerDefend`, table `0x6067e8`), the formotions a team may pick. Team
//! files hold the team's own variants and lists; `match/quidd` is the global database.
//!
//! The text is a token stream: `<VERSION = 2>`, then `<FORMOTION> { key = value ... <WAYPOINT> { key = values ; ...
//! } ... }` and `<TRANSFERLIST = name > { formotions = A B ... NULL }`. Keys are matched without case (the table
//! holds them in capitals). `ppartner = NULL` and `defend = -1` in every retail file.

use crate::util::{bad, Result};

/// The 17 formation states in the order of the game's name table (`0x6067e8`) and of the transfer lists.
pub const STATES: [&str; 17] = [
    "Free",
    "FreeToDefend",
    "FreeToAttack",
    "AttackToAttack",
    "AttackToDefend",
    "DefendToDefend",
    "DefendToAttack",
    "AttackPass",
    "DefendPass",
    "AIDefend",
    "KeeperAttack",
    "KeeperDefend",
    "BeaterIdle",
    "BeaterAttack",
    "BeaterDefend",
    "BludgerIdle",
    "BludgerDefend",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Waypoint {
    /// Fraction of the formotion's duration, increasing within `(0, 1]`; 612 of the 619 retail formotions end at 1
    /// (the other 7, e.g. `DTA2_R1` of five teams, stop at about 0.6).
    pub time: f32,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub heading: [f32; 3],
    pub flags: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Formotion {
    pub name: String,
    /// 1 left, 2 right, 0 either; 4 seen on some keeper formotions (meaning not traced).
    pub flags: u32,
    /// Duration in ticks.
    pub timesteps: i32,
    pub ppartner: Option<String>,
    pub defend: i32,
    /// Pair index (-1 none), matched by the picker `0x42cee0` against the record's `+0x0c`.
    pub lpair: i32,
    pub waypoints: Vec<Waypoint>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransferList {
    /// The state name without its `eFT_` prefix.
    pub state: String,
    pub formotions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Database {
    pub version: i32,
    pub formotions: Vec<Formotion>,
    pub lists: Vec<TransferList>,
}

impl Database {
    pub fn formotion(&self, name: &str) -> Option<&Formotion> {
        self.formotions.iter().find(|f| f.name.eq_ignore_ascii_case(name))
    }

    /// The transfer list of state `index` (into [`STATES`]).
    pub fn list(&self, index: usize) -> Option<&TransferList> {
        self.lists.iter().find(|l| l.state.eq_ignore_ascii_case(STATES[index]))
    }
}

struct Tokens<'a> {
    toks: Vec<&'a str>,
    at: usize,
}

impl<'a> Tokens<'a> {
    fn next(&mut self) -> Result<&'a str> {
        let t = self.toks.get(self.at).copied();
        self.at += 1;
        t.map_or_else(|| bad("unexpected end of file"), Ok)
    }

    fn peek(&self) -> Option<&'a str> {
        self.toks.get(self.at).copied()
    }

    fn expect(&mut self, want: &str) -> Result<()> {
        let t = self.next()?;
        if t != want {
            return bad(format!("expected {want:?}, found {t:?}"));
        }
        Ok(())
    }

    fn float(&mut self) -> Result<f32> {
        let t = self.next()?;
        t.parse().map_or_else(|_| bad(format!("not a number: {t:?}")), Ok)
    }

    fn int(&mut self) -> Result<i32> {
        let t = self.next()?;
        t.parse().map_or_else(|_| bad(format!("not an integer: {t:?}")), Ok)
    }

    fn vec3(&mut self) -> Result<[f32; 3]> {
        Ok([self.float()?, self.float()?, self.float()?])
    }

    /// `key =`, lower-cased; `None` at the closing brace.
    fn key(&mut self) -> Result<Option<String>> {
        let k = self.next()?;
        if k == "}" {
            return Ok(None);
        }
        if k.starts_with('<') {
            self.at -= 1;
            return Ok(Some(k.to_ascii_uppercase()));
        }
        self.expect("=")?;
        Ok(Some(k.to_ascii_lowercase()))
    }
}

/// Splits the text so that `< > { } = ;` are tokens of their own; `<NAME` stays one token.
fn tokenize(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for word in text.split_ascii_whitespace() {
        let mut rest = word;
        while !rest.is_empty() {
            let cut = rest.find(['>', '{', '}', '=', ';']).unwrap_or(rest.len());
            let (a, b) = if cut == 0 { rest.split_at(1) } else { rest.split_at(cut) };
            out.push(a);
            rest = b;
        }
    }
    out
}

fn waypoint(t: &mut Tokens) -> Result<Waypoint> {
    t.expect("{")?;
    let mut w = Waypoint { time: 0.0, pos: [0.0; 3], vel: [0.0; 3], heading: [0.0; 3], flags: 0 };
    while let Some(k) = t.key()? {
        match k.as_str() {
            "time" => w.time = t.float()?,
            "pos" => w.pos = t.vec3()?,
            "vel" => w.vel = t.vec3()?,
            "heading" => w.heading = t.vec3()?,
            "flags" => w.flags = t.int()? as u32,
            _ => return bad(format!("unknown waypoint key {k:?}")),
        }
        t.expect(";")?;
    }
    Ok(w)
}

fn formotion(t: &mut Tokens) -> Result<Formotion> {
    t.expect("{")?;
    let mut f = Formotion {
        name: String::new(),
        flags: 0,
        timesteps: 0,
        ppartner: None,
        defend: -1,
        lpair: -1,
        waypoints: Vec::new(),
    };
    while let Some(k) = t.key()? {
        match k.as_str() {
            "name" => f.name = t.next()?.to_string(),
            "flags" => f.flags = t.int()? as u32,
            "timesteps" => f.timesteps = t.int()?,
            "ppartner" => {
                let p = t.next()?;
                f.ppartner = (p != "NULL").then(|| p.to_string());
            }
            "defend" => f.defend = t.int()?,
            "lpair" => f.lpair = t.int()?,
            "<WAYPOINT" => {
                t.next()?;
                t.expect(">")?;
                f.waypoints.push(waypoint(t)?);
            }
            _ => return bad(format!("unknown formotion key {k:?}")),
        }
    }
    Ok(f)
}

/// Parses a database and checks it: names unique, every waypoint list non-empty with times increasing in `(0, 1]`, every
/// transfer list ends with `NULL` and names only formotions of the same file.
pub fn parse(d: &[u8]) -> Result<Database> {
    let text = std::str::from_utf8(d).map_err(|e| crate::Error::Format(format!("not text: {e}")))?;
    let mut t = Tokens { toks: tokenize(text), at: 0 };
    let mut db = Database::default();
    while t.peek().is_some() {
        match t.next()?.to_ascii_uppercase().as_str() {
            "<VERSION" => {
                t.expect("=")?;
                db.version = t.int()?;
                t.expect(">")?;
            }
            "<FORMOTION" => {
                t.expect(">")?;
                db.formotions.push(formotion(&mut t)?);
            }
            "<TRANSFERLIST" => {
                t.expect("=")?;
                let state = t.next()?;
                let Some(state) = state.strip_prefix("eFT_") else { return bad(format!("bad state {state:?}")) };
                if !STATES.iter().any(|s| s.eq_ignore_ascii_case(state)) {
                    return bad(format!("unknown state {state:?}"));
                }
                t.expect(">")?;
                t.expect("{")?;
                if t.key()?.as_deref() != Some("formotions") {
                    return bad("transfer list without formotions");
                }
                let mut names = Vec::new();
                loop {
                    match t.next()? {
                        "NULL" => break,
                        n => names.push(n.to_string()),
                    }
                }
                t.expect("}")?;
                db.lists.push(TransferList { state: state.to_string(), formotions: names });
            }
            other => return bad(format!("unexpected {other:?}")),
        }
    }
    for (i, f) in db.formotions.iter().enumerate() {
        if db.formotions[..i].iter().any(|g| g.name.eq_ignore_ascii_case(&f.name)) {
            return bad(format!("formotion {} twice", f.name));
        }
        let times: Vec<f32> = f.waypoints.iter().map(|w| w.time).collect();
        let in_range = times.first().is_some_and(|&t| t > 0.0) && times.last().is_some_and(|&t| t <= 1.0);
        if !in_range || times.windows(2).any(|w| w[0] >= w[1]) {
            return bad(format!("formotion {}: waypoint times {times:?}", f.name));
        }
    }
    for l in &db.lists {
        if let Some(n) = l.formotions.iter().find(|n| db.formotion(n).is_none()) {
            return bad(format!("transfer list {} names unknown formotion {n}", l.state));
        }
    }
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "<VERSION = 2>\n\n<FORMOTION>\n{\nname\t     = AB_L1\nflags = 1\ntimesteps = 70\n\
        ppartner = NULL\ndefend = -1\nlpair = 2\n<WAYPOINT> {time = 0.25 ; pos = -0.5 0.0 0.25 ; vel = 1 0 -1 ; \
        heading = 0 0 1 ; flags = 1 ; }\n<WAYPOINT> {time = 1.000000 ; pos = 0 0 1 ; vel = 0 0 0 ; heading = 0 0 0 \
        ; flags = 0 ; }\n}\n<TRANSFERLIST = eFT_AttackToAttack >\n{\nformotions\t= AB_L1 NULL\n}\n\
        <TRANSFERLIST = eFT_Free >\n{\nformotions\t= NULL\n}\n";

    #[test]
    fn parses() {
        let db = parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(db.version, 2);
        let f = &db.formotions[0];
        assert_eq!((f.name.as_str(), f.flags, f.timesteps, f.lpair, f.defend), ("AB_L1", 1, 70, 2, -1));
        assert_eq!(f.ppartner, None);
        assert_eq!(f.waypoints.len(), 2);
        assert_eq!(f.waypoints[0].pos, [-0.5, 0.0, 0.25]);
        assert_eq!(f.waypoints[0].heading, [0.0, 0.0, 1.0]);
        assert_eq!(f.waypoints[0].flags, 1);
        assert_eq!(db.list(3).unwrap().formotions, ["AB_L1"]);
        assert_eq!(db.list(0).unwrap().formotions, Vec::<String>::new());
        assert!(db.list(1).is_none());
    }

    #[test]
    fn checks() {
        let unknown = SAMPLE.replace("= AB_L1 NULL", "= XY NULL");
        assert!(parse(unknown.as_bytes()).is_err());
        let back = SAMPLE.replace("time = 1.000000", "time = 0.2");
        assert!(parse(back.as_bytes()).is_err());
        let state = SAMPLE.replace("eFT_Free", "eFT_Nothing");
        assert!(parse(state.as_bytes()).is_err());
        assert!(parse(b"<VERSION = 2> <FORMOTION> {").is_err());
    }

    #[test]
    fn tokens() {
        assert_eq!(tokenize("<VERSION = 2>"), ["<VERSION", "=", "2", ">"]);
        assert_eq!(
            tokenize("{time = 0.1 ; flags = 0 ; }"),
            ["{", "time", "=", "0.1", ";", "flags", "=", "0", ";", "}"]
        );
        assert_eq!(tokenize("<TRANSFERLIST = eFT_Free >"), ["<TRANSFERLIST", "=", "eFT_Free", ">"]);
    }
}
