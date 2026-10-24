//! Match descriptions (`match/<name>/matchdesc.txt`, the `matchdesc` package of a `MatchDescLoadSet`): what a game is
//! played with. Port of `tools/matchdesc_parse.py`, which documents the layout.
//!
//! Records are a tag in angle brackets and a block in braces of `key = value` lines: `MATCH_DESCRIPTION` (the kind of
//! game and, for a challenge, which one), then each `PLAYER` and `BALL` in order.

use crate::{Error, Result};

/// One record: its tag and its fields in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub tag: String,
    pub fields: Vec<(String, String)>,
}

impl Record {
    /// The value of field `key` (case not counted), if it has one.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }
}

/// Every record of a file, in order.
pub fn parse(data: &[u8]) -> Result<Vec<Record>> {
    let text: String = data.iter().map(|&b| b as char).collect();
    let mut records: Vec<Record> = Vec::new();
    let (mut open, mut inside) = (false, false);
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let bad = || Error::Format(format!("matchdesc line {}: {raw:?}", n + 1));
        if !inside && line.starts_with('<') && line.ends_with('>') {
            records.push(Record { tag: line[1..line.len() - 1].trim().to_string(), fields: Vec::new() });
            open = true;
        } else if line == "{" && open && !inside {
            inside = true;
        } else if line == "}" && inside {
            inside = false;
            open = false;
        } else if inside {
            let (k, v) = line.split_once('=').ok_or_else(bad)?;
            records.last_mut().ok_or_else(bad)?.fields.push((k.trim().to_string(), v.trim().to_string()));
        } else {
            return Err(bad());
        }
    }
    if inside {
        return Err(Error::Format("matchdesc: unterminated block".into()));
    }
    Ok(records)
}

/// A player of the description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    /// `eKeeper`, `eChaser`, `eBeater` or `eSeeker`.
    pub kind: String,
    pub team: u8,
    pub subtype_index: Option<u32>,
}

/// The description read: the kind of game (`Quidditch`, `Challenge`, `SubGame`), the challenge (`MatchSubType`,
/// 1 to 6), the players and the balls (`eQuaffle`, `eBludger`, `eSnitch`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchDesc {
    pub match_type: String,
    pub sub_type: Option<u32>,
    pub players: Vec<Player>,
    pub balls: Vec<String>,
}

impl MatchDesc {
    pub fn from_records(records: &[Record]) -> MatchDesc {
        let head = records.iter().find(|r| r.tag.eq_ignore_ascii_case("MATCH_DESCRIPTION"));
        let num = |r: &Record, k: &str| r.get(k).and_then(|v| v.parse().ok());
        MatchDesc {
            match_type: head.and_then(|h| h.get("MatchType")).unwrap_or("Quidditch").to_string(),
            sub_type: head.and_then(|h| num(h, "MatchSubType")),
            players: records
                .iter()
                .filter(|r| r.tag.eq_ignore_ascii_case("PLAYER"))
                .map(|r| Player {
                    kind: r.get("type").unwrap_or_default().to_string(),
                    team: r.get("team").and_then(|v| v.parse().ok()).unwrap_or(0),
                    subtype_index: num(r, "subtypeindex"),
                })
                .collect(),
            balls: records
                .iter()
                .filter(|r| r.tag.eq_ignore_ascii_case("BALL"))
                .map(|r| r.get("type").unwrap_or_default().to_string())
                .collect(),
        }
    }

    /// Parse and read a file.
    pub fn parse(data: &[u8]) -> Result<MatchDesc> {
        Ok(MatchDesc::from_records(&parse(data)?))
    }

    /// Whether this is a challenge (`MatchType = Challenge`).
    pub fn is_challenge(&self) -> bool {
        self.match_type.eq_ignore_ascii_case("Challenge")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_challenge() {
        let d = MatchDesc::parse(
            b"<MATCH_DESCRIPTION>\r\n{\r\nMatchType = Challenge\r\nMatchSubType = 2\r\n}\r\n\r\n<PLAYER> \r\n{ \r\n\
              type = eChaser \r\nteam = 0 \r\nsubtypeindex = 2\r\n}\r\n<BALL>\r\n{\r\ntype = eQuaffle\r\n}\r\n",
        )
        .unwrap();
        assert!(d.is_challenge());
        assert_eq!(d.sub_type, Some(2));
        assert_eq!(d.players, [Player { kind: "eChaser".into(), team: 0, subtype_index: Some(2) }]);
        assert_eq!(d.balls, ["eQuaffle"]);
        assert!(parse(b"<A>\n{\nx = 1\n").is_err());
        assert!(parse(b"stray\n").is_err());
    }
}
