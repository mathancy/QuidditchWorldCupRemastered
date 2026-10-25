//! News and updates: the remaster's releases on GitHub (the public repository's Releases, `docs/LAUNCHER.md`), the
//! newest one checked against this version, its installer for this platform downloaded and checked against the
//! release's `SHA256SUMS`.
//!
//! A release is tagged `vX.Y.Z` and carries `QWCRemastered-X.Y.Z-setup.exe` (Windows),
//! `QWCRemastered-X.Y.Z-x86_64.AppImage` (Linux) and `SHA256SUMS` (`sha256sum`'s format), as the release pipeline
//! makes them (milestone 6).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::setup::Progress;

/// The releases' address: `$QWC_RELEASES_URL`, else the public repository's.
pub fn releases_url() -> String {
    std::env::var("QWC_RELEASES_URL").unwrap_or_else(|_| {
        "https://api.github.com/repos/mathancy/QuidditchWorldCupRemastered/releases?per_page=10".into()
    })
}

/// This launcher's version.
pub fn current() -> semver::Version {
    semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate's own version")
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Asset {
    pub name: String,
    #[serde(rename = "browser_download_url")]
    pub url: String,
    #[serde(default)]
    pub size: u64,
}

/// A release as GitHub's API gives it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

impl Release {
    /// Its version, from the tag (`v0.2.0`); none for a tag that is not one.
    pub fn version(&self) -> Option<semver::Version> {
        semver::Version::parse(self.tag_name.trim_start_matches('v')).ok()
    }

    pub fn title(&self) -> String {
        self.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| self.tag_name.clone())
    }

    /// The day it came out (`2026-10-23`).
    pub fn date(&self) -> &str {
        self.published_at.as_deref().map_or("", |d| d.get(..10).unwrap_or(d))
    }

    /// The installer for this platform: the Windows setup or the Linux AppImage.
    pub fn installer(&self) -> Option<&Asset> {
        let wanted = if cfg!(windows) { "-setup.exe" } else { ".appimage" };
        self.assets.iter().find(|a| a.name.to_ascii_lowercase().ends_with(wanted))
    }
}

/// A piece of a release's notes, read from their Markdown ([`notes`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// A `#` heading.
    Heading(String),
    /// A paragraph.
    Text(String),
    /// A list item and how deep it sits (0 at the left).
    Item(usize, String),
    /// A quote, or a GitHub alert (`> [!WARNING]`), without its marker.
    Note(String),
}

/// A release's notes as blocks to draw: the lines GitHub's Markdown wraps joined into their paragraph, item or
/// quote, and the inline marks (`**`, `` ` ``, a link's address) taken out.
pub fn notes(body: &str) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    // whether the last block takes the next line's words
    let mut open = false;
    for raw in body.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            open = false;
            continue;
        }
        if let Some(h) = trimmed.strip_prefix('#') {
            out.push(Block::Heading(inline(h.trim_start_matches('#').trim())));
            open = false;
        } else if let Some(q) = trimmed.strip_prefix('>') {
            let q = q.trim();
            // an alert's marker line says nothing itself
            if q.starts_with("[!") && q.ends_with(']') {
                out.push(Block::Note(String::new()));
                open = true;
            } else if q.is_empty() {
                open = false;
            } else if let (true, Some(Block::Note(t))) = (open, out.last_mut()) {
                join(t, q);
            } else {
                out.push(Block::Note(inline(q)));
                open = true;
            }
        } else if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            let depth = (line.len() - trimmed.len()) / 2;
            out.push(Block::Item(depth, inline(item.trim())));
            open = true;
        } else if let (true, Some(Block::Text(t) | Block::Item(_, t) | Block::Note(t))) = (open, out.last_mut()) {
            join(t, trimmed);
        } else {
            out.push(Block::Text(inline(trimmed)));
            open = true;
        }
    }
    out.retain(|b| !matches!(b, Block::Note(t) if t.is_empty()));
    out
}

/// `more` (a wrapped line) added to `text`.
fn join(text: &mut String, more: &str) {
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(&inline(more));
}

/// A line without its Markdown marks: bold and code marks gone, `[words](address)` as its words.
fn inline(line: &str) -> String {
    let mut s = line.replace("**", "").replace('`', "");
    while let Some(open) = s.find('[') {
        let Some(close) = s[open..].find("](").map(|c| open + c) else { break };
        let Some(end) = s[close..].find(')').map(|e| close + e) else { break };
        s.replace_range(close..=end, "");
        s.remove(open);
    }
    s
}

/// The releases in GitHub's JSON, the newest first, drafts left out.
pub fn parse(json: &str) -> Result<Vec<Release>, String> {
    let mut r: Vec<Release> = serde_json::from_str(json).map_err(|e| format!("the releases do not read: {e}"))?;
    r.retain(|x| !x.draft);
    r.sort_by_key(|x| std::cmp::Reverse(x.version()));
    Ok(r)
}

/// The newest full release past `than`, if any.
pub fn newer<'a>(releases: &'a [Release], than: &semver::Version) -> Option<&'a Release> {
    releases
        .iter()
        .filter(|r| !r.prerelease)
        .filter(|r| r.version().is_some_and(|v| &v > than))
        .max_by_key(|r| r.version())
}

/// `SHA256SUMS` (`<hex>  <name>` lines) by file name.
pub fn checksums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (sha, name) = l.trim().split_once(char::is_whitespace)?;
            let name = name.trim().trim_start_matches('*');
            (sha.len() == 64).then(|| (name.to_string(), sha.to_ascii_lowercase()))
        })
        .collect()
}

pub(crate) fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .user_agent(concat!("qwc-launcher/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Download `url` to `file` (written as `file.part`, renamed once whole), keeping it only if its SHA-256 is `sha256`
/// and, when given, it is exactly `size` bytes; a larger file is stopped as soon as it passes `size`. `progress`
/// counts the bytes, and its cancel stops the download. The releases' installers and the store's packs.
pub(crate) fn fetch_checked(
    url: &str,
    file: &Path,
    size: Option<u64>,
    sha256: &str,
    progress: &Progress,
) -> Result<(), String> {
    let name = file.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let part = file.with_file_name(format!("{name}.part"));
    let mut res = agent().get(url).call().map_err(|e| e.to_string())?;
    progress.total.store(size.or_else(|| res.body().content_length()).unwrap_or(0), Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);
    let mut body = res.body_mut().with_config().limit(4 << 30).reader();
    let mut out = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    let (mut h, mut buf, mut written) = (Sha256::new(), vec![0u8; 1 << 16], 0u64);
    let failed = |out: std::fs::File, why: String| {
        drop(out);
        let _ = std::fs::remove_file(&part);
        Err(why)
    };
    loop {
        let n = match body.read(&mut buf) {
            Ok(n) => n,
            Err(e) => return failed(out, e.to_string()),
        };
        if n == 0 {
            break;
        }
        written += n as u64;
        if size.is_some_and(|s| written > s) {
            return failed(out, format!("{name} is larger than the file reviewed: not installed"));
        }
        h.update(&buf[..n]);
        if let Err(e) = out.write_all(&buf[..n]) {
            return failed(out, e.to_string());
        }
        progress.done.fetch_add(n as u64, Ordering::Relaxed);
        if progress.cancel.load(Ordering::Relaxed) {
            return failed(out, "stopped".into());
        }
    }
    if let Some(s) = size.filter(|&s| s != written) {
        return failed(out, format!("{name} is {written} bytes, not the {s} expected: not installed"));
    }
    let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if !got.eq_ignore_ascii_case(sha256) {
        return failed(out, format!("{name} does not match its checksum: not installed"));
    }
    drop(out);
    std::fs::rename(&part, file).map_err(|e| format!("{}: {e}", file.display()))
}

/// The releases, fetched.
pub fn fetch(url: &str) -> Result<Vec<Release>, String> {
    let mut res = agent().get(url).header("Accept", "application/vnd.github+json").call().map_err(|e| match e {
        ureq::Error::StatusCode(404) => "No releases are published yet.".to_string(),
        e => format!("Could not reach the releases: {e}"),
    })?;
    let text = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    parse(&text)
}

/// Download `release`'s installer into `dir`, checked against its `SHA256SUMS`; answers the file.
pub fn download(release: &Release, dir: &Path, progress: &Progress) -> Result<PathBuf, String> {
    let asset = release.installer().ok_or("this release has no installer for this system")?;
    let sums = release.assets.iter().find(|a| a.name == "SHA256SUMS").ok_or("this release has no SHA256SUMS")?;
    let agent = agent();
    let text = agent
        .get(&sums.url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let want =
        checksums(&text).remove(&asset.name).ok_or_else(|| format!("SHA256SUMS does not list {}", asset.name))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let file = dir.join(&asset.name);
    fetch_checked(&asset.url, &file, Some(asset.size), &want, progress)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_join_their_wrapped_lines() {
        let body = "The first release: rebuilt
for today.

> [!WARNING]
> **Early.** Expect
> bugs.

## What you need

- Your own `QWC.exe`: the
  retail one is not read.
- The launcher:
  - sets up [your files](https://x/y);
";
        assert_eq!(
            notes(body),
            [
                Block::Text("The first release: rebuilt for today.".into()),
                Block::Note("Early. Expect bugs.".into()),
                Block::Heading("What you need".into()),
                Block::Item(0, "Your own QWC.exe: the retail one is not read.".into()),
                Block::Item(0, "The launcher:".into()),
                Block::Item(1, "sets up your files;".into()),
            ]
        );
    }

    const JSON: &str = r#"[
        {"tag_name": "v0.3.0-beta.1", "prerelease": true, "assets": []},
        {"tag_name": "v0.1.0", "name": "First", "published_at": "2026-11-01T10:00:00Z", "body": "Hello", "assets": []},
        {"tag_name": "v0.2.0", "name": "", "published_at": "2026-12-01T10:00:00Z", "body": "More",
         "assets": [{"name": "QWCRemastered-0.2.0-setup.exe", "browser_download_url": "https://x/setup.exe", "size": 9},
                    {"name": "QWCRemastered-0.2.0-x86_64.AppImage", "browser_download_url": "https://x/app", "size": 8},
                    {"name": "SHA256SUMS", "browser_download_url": "https://x/sums", "size": 1}]},
        {"tag_name": "v9.9.9", "draft": true, "assets": []}
    ]"#;

    #[test]
    fn releases_newest_first_and_the_newer_one() {
        let r = parse(JSON).unwrap();
        let tags: Vec<&str> = r.iter().map(|x| x.tag_name.as_str()).collect();
        assert_eq!(tags, ["v0.3.0-beta.1", "v0.2.0", "v0.1.0"], "drafts left out");
        let v = |s: &str| semver::Version::parse(s).unwrap();
        assert_eq!(newer(&r, &v("0.1.0")).map(|x| x.tag_name.as_str()), Some("v0.2.0"), "a beta is not offered");
        assert_eq!(newer(&r, &v("0.2.0")), None);
        assert_eq!(r[1].title(), "v0.2.0");
        assert_eq!((r[2].title().as_str(), r[2].date()), ("First", "2026-11-01"));
        let installer = r[1].installer().unwrap();
        assert!(installer.name.ends_with(if cfg!(windows) { "setup.exe" } else { "AppImage" }));
        assert!(parse("{").is_err());
    }

    #[test]
    fn checksum_lists() {
        let a = "a".repeat(64);
        let sums = checksums(&format!("{a}  QWCRemastered-0.2.0-setup.exe\n{a} *binary.AppImage\nnot a line\n"));
        assert_eq!(sums.get("QWCRemastered-0.2.0-setup.exe"), Some(&a));
        assert_eq!(sums.get("binary.AppImage"), Some(&a));
        assert_eq!(sums.len(), 2);
    }
}
