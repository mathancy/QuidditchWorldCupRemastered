//! The game console: the original registry of variables and commands ([`crate::registry`]) with their values, and
//! the `.cli` script runner that sets them.
//!
//! Every variable and command keeps two value slots, one per team. A team-scoped entry (`Passing.PassAI.*`, the team
//! commands such as `Passing.PassHelp.PassCone`) writes the slot of the team being loaded ([`Console::team`], the
//! game's `0x6685b4`); any other entry writes both, so reads never need to know an entry's scope.

use std::collections::HashMap;
use std::fmt;

use qwc_formats::text_defs::{self, Ext, Parsed};

use crate::registry::DEFS;

/// Parameter types, in the order of the registry's type codes (`0x4caee0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Word,
    Str,
    Enum,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Variable,
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    /// Written to the slot of the team whose `team_N.cli` is loading.
    Team,
    /// Accepted and ignored by the original (handler `0x53f020`).
    Ignored,
}

/// Every entry the original registers (QWC Studio's tuning editor lists them).
pub fn registry() -> &'static [Def] {
    DEFS
}

/// One registered entry.
#[derive(Debug)]
pub struct Def {
    pub path: &'static str,
    pub kind: Kind,
    pub scope: Scope,
    pub params: &'static [Ty],
    /// The image default of a variable, per team slot (0 where the storage is zero-filled).
    pub default: [f64; 2],
}

/// A parsed argument.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Num(f64),
    Text(String),
}

impl Value {
    pub fn num(&self) -> Option<f64> {
        match self {
            Value::Num(v) => Some(*v),
            Value::Text(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// No registered entry has this path.
    Unknown(String),
    /// An argument does not parse as its parameter type.
    BadArgument { path: String, arg: String, ty: Ty },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unknown(p) => write!(f, "unknown console entry {p}"),
            Error::BadArgument { path, arg, ty } => write!(f, "{path}: {arg:?} is not a {ty:?}"),
        }
    }
}

impl std::error::Error for Error {}

/// Split a line's arguments into words, keeping a quoted string as one word without its quotes.
fn words(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

fn parse_arg(ty: Ty, s: &str) -> Option<Value> {
    Some(match ty {
        // decimal or 0x hex (`0x4cfd90`)
        Ty::Int => Value::Num(match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            Some(h) => i64::from_str_radix(h, 16).ok()? as f64,
            None => s.parse::<i64>().ok()? as f64,
        }),
        Ty::Float => Value::Num(s.parse::<f64>().ok()?),
        Ty::Bool => Value::Num(match s.to_ascii_lowercase().as_str() {
            "true" | "1" => 1.0,
            "false" | "0" => 0.0,
            _ => return None,
        }),
        Ty::Word | Ty::Str | Ty::Enum => Value::Text(s.to_string()),
    })
}

/// The registry with its current values.
pub struct Console {
    exact: HashMap<String, usize>,
    /// Entries with a `?` segment, as lower-case segments.
    wild: Vec<(Vec<String>, usize)>,
    values: Vec<[Vec<Value>; 2]>,
    /// Every command call in order: the entry's index, the team slot loading, and its arguments.
    log: Vec<(usize, usize, Vec<Value>)>,
    /// The team slot team-scoped entries write (0 or 1).
    pub team: usize,
}

impl Default for Console {
    fn default() -> Console {
        Console::new()
    }
}

impl Console {
    /// Every entry at its image default.
    pub fn new() -> Console {
        let (mut exact, mut wild) = (HashMap::new(), Vec::new());
        for (i, d) in DEFS.iter().enumerate() {
            let key = d.path.to_ascii_lowercase();
            if key.split('.').any(|s| s == "?") {
                wild.push((key.split('.').map(str::to_string).collect(), i));
            } else {
                exact.insert(key, i);
            }
        }
        let values = DEFS
            .iter()
            .map(|d| match d.kind {
                Kind::Variable => d.default.map(|v| vec![Value::Num(v)]),
                Kind::Command => [Vec::new(), Vec::new()],
            })
            .collect();
        Console { exact, wild, values, log: Vec::new(), team: 0 }
    }

    /// The entry for a dotted path (names compare without regard to case).
    pub fn find(&self, path: &str) -> Option<&'static Def> {
        self.index(path).map(|i| &DEFS[i])
    }

    fn index(&self, path: &str) -> Option<usize> {
        let key = path.to_ascii_lowercase();
        if let Some(&i) = self.exact.get(&key) {
            return Some(i);
        }
        let segs: Vec<&str> = key.split('.').collect();
        self.wild
            .iter()
            .find(|(w, _)| w.len() == segs.len() && w.iter().zip(&segs).all(|(a, b)| a == "?" || a == b))
            .map(|&(_, i)| i)
    }

    /// Run one console line: `path` and the rest of the line. With no arguments the original prints the current
    /// value; here nothing changes.
    pub fn run(&mut self, path: &str, args: &str) -> Result<(), Error> {
        let i = self.index(path).ok_or_else(|| Error::Unknown(path.to_string()))?;
        let def = &DEFS[i];
        let words = words(args);
        if def.scope == Scope::Ignored {
            return Ok(());
        }
        if words.is_empty() {
            // a command without parameters (`Pitch.Collision.Clear`) runs; anything else only prints
            if def.kind == Kind::Command && def.params.is_empty() {
                self.log.push((i, self.team.min(1), Vec::new()));
            }
            return Ok(());
        }
        let mut parsed = Vec::new();
        for (k, w) in words.iter().enumerate() {
            // extra words go to the last parameter's type (a sentence after a string parameter, for example)
            let ty = *def.params.get(k).or(def.params.last()).unwrap_or(&Ty::Word);
            parsed.push(parse_arg(ty, w).ok_or_else(|| Error::BadArgument {
                path: def.path.to_string(),
                arg: w.clone(),
                ty,
            })?);
        }
        if def.kind == Kind::Command {
            self.log.push((i, self.team.min(1), parsed.clone()));
        }
        match def.scope {
            Scope::Team => self.values[i][self.team.min(1)] = parsed,
            _ => self.values[i] = [parsed.clone(), parsed],
        }
        Ok(())
    }

    /// Every command call so far, in order, with its path and arguments: for commands whose handlers keep more than
    /// their last call (`Pitch.Collision.*` adds to a list, [`crate::walls`]).
    pub fn calls(&self) -> impl Iterator<Item = (&'static str, &[Value])> {
        self.log.iter().map(|(i, _, v)| (DEFS[*i].path, v.as_slice()))
    }

    /// The command calls made while team slot `team`'s files loaded (for global commands whose handlers write the
    /// loading team's block, as `Formotions.TweakTime` does through `[0x6685b4]`).
    pub fn team_calls(&self, team: usize) -> impl Iterator<Item = (&'static str, &[Value])> {
        self.log.iter().filter(move |(_, t, _)| *t == team.min(1)).map(|(i, _, v)| (DEFS[*i].path, v.as_slice()))
    }

    /// Run a `.cli` script. Lines that fail are skipped and returned, as the original reports and skips them.
    pub fn run_script(&mut self, bytes: &[u8]) -> Vec<Error> {
        let Ok(Parsed::Cli { values, .. }) = text_defs::parse(Ext::Cli, bytes) else { return Vec::new() };
        values.iter().filter_map(|(k, v)| self.run(k, v.as_deref().unwrap_or("")).err()).collect()
    }

    /// The arguments an entry was last given for team slot `team` (a variable's value, a command's last call).
    pub fn args(&self, path: &str, team: usize) -> &[Value] {
        self.index(path).map_or(&[], |i| &self.values[i][team.min(1)])
    }

    /// A numeric variable or a command's first numeric argument for team slot `team`; 0 when unset.
    pub fn num(&self, path: &str, team: usize) -> f64 {
        self.args(path, team).first().and_then(Value::num).unwrap_or(0.0)
    }

    /// [`Console::num`] as `f32`, the type the original stores floats in.
    pub fn f32(&self, path: &str, team: usize) -> f32 {
        self.num(path, team) as f32
    }

    pub fn flag(&self, path: &str) -> bool {
        self.num(path, 0) != 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_scopes_and_scripts() {
        let mut c = Console::new();
        assert_eq!(c.num("Passing.PassAI.PassOutBase", 1), 1.0);
        assert_eq!(c.num("passing.passai.passoutbasegp", 0), 0.25);
        c.team = 1;
        let errors = c.run_script(b"Passing.PassAI.PassOutBase 2.5\nPitch.HeightMapHeight 12\nNo.Such.Key 1\n");
        assert_eq!(errors, [Error::Unknown("No.Such.Key".into())]);
        assert_eq!((c.num("Passing.PassAI.PassOutBase", 0), c.num("Passing.PassAI.PassOutBase", 1)), (1.0, 2.5));
        assert_eq!((c.num("Pitch.HeightMapHeight", 0), c.num("Pitch.HeightMapHeight", 1)), (12.0, 12.0));
        assert!(c.run("Pitch.HeightMapHeight", "high").is_err());
        assert!(c.find("GoalRender.flare.StartTime").is_some());
    }

    #[test]
    fn words_and_ints() {
        assert_eq!(words(r#" a "b c"  d "#), ["a", "b c", "d"]);
        assert_eq!(parse_arg(Ty::Int, "0x10"), Some(Value::Num(16.0)));
        assert_eq!(parse_arg(Ty::Bool, "TRUE"), Some(Value::Num(1.0)));
    }
}
