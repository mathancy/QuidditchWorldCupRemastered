//! The pack store (`docs/PACK_STORE.md`): its packs, from the store's API (`$QWC_STORE_URL`, else the live store),
//! and a pack installed from its author's link only when the file is the one reviewed: its size and SHA-256 are the
//! listing's, and its `pack.ron` names the listed pack and version. The store keeps no files; it lists links.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::setup::Progress;

/// The store's address: `$QWC_STORE_URL` (a local store while testing), else the live one.
pub fn store_url() -> String {
    std::env::var("QWC_STORE_URL").unwrap_or_else(|_| "https://store.qwcr.workers.dev".into())
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Author {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Rating {
    pub average: Option<f32>,
    pub count: u32,
}

/// A pack as the store lists it: its newest approved version.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Listed {
    pub id: String,
    pub name: String,
    pub version: String,
    /// `Visual` or `Gameplay`.
    pub kind: String,
    #[serde(default)]
    pub description: String,
    pub author: Author,
    /// The remaster versions it works with (`>=0.1`), empty for any.
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub licence: String,
    /// This version's id in the store.
    pub listing: u64,
    /// The author's file, and the reviewed file's SHA-256 and size.
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub rating: Rating,
    #[serde(default)]
    pub installs: u64,
    /// When this version was approved.
    #[serde(default)]
    pub updated: String,
}

impl Listed {
    /// Whether this remaster can play it: its engine range includes this version (any, when it has none).
    pub fn fits(&self, version: &semver::Version) -> bool {
        self.engine.trim().is_empty() || semver::VersionReq::parse(&self.engine).is_ok_and(|r| r.matches(version))
    }
}

/// A page of the store's packs.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Page {
    pub packs: Vec<Listed>,
    pub more: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    New,
    Rating,
    Installs,
    Name,
}

impl Sort {
    pub const ALL: [Sort; 4] = [Sort::New, Sort::Rating, Sort::Installs, Sort::Name];

    fn word(self) -> &'static str {
        match self {
            Sort::New => "new",
            Sort::Rating => "rating",
            Sort::Installs => "installs",
            Sort::Name => "name",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Sort::New => "Newest",
            Sort::Rating => "Best rated",
            Sort::Installs => "Most installed",
            Sort::Name => "Name",
        }
    }
}

/// What to look for: words, a kind (`Visual`, `Gameplay`), the order, the page from 1.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub words: String,
    pub kind: Option<&'static str>,
    pub sort: Sort,
    pub page: u32,
}

/// `text` for an address's query, letters and digits kept, every other byte as `%XX`.
fn escape(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The address of `query`'s page on the store at `base`.
pub fn page_url(base: &str, query: &Query) -> String {
    let mut url = format!("{}/packs?sort={}&page={}", base.trim_end_matches('/'), query.sort.word(), query.page.max(1));
    if !query.words.trim().is_empty() {
        url += &format!("&q={}", escape(query.words.trim()));
    }
    if let Some(k) = query.kind {
        url += &format!("&kind={k}");
    }
    url
}

/// A page of the store's packs.
pub fn browse(base: &str, query: &Query) -> Result<Page, String> {
    let mut res = crate::updates::agent()
        .get(&page_url(base, query))
        .header("Accept", "application/json")
        .call()
        .map_err(|e| format!("Could not reach the store: {e}"))?;
    let text = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("The store's answer does not read: {e}"))
}

/// Whether the launcher downloads from `url`: HTTPS, or plain HTTP to this computer only (a store under test).
pub fn allowed(url: &str) -> bool {
    let local = ["http://127.0.0.1:", "http://127.0.0.1/", "http://localhost:", "http://localhost/"];
    url.starts_with("https://") || local.iter().any(|l| url.starts_with(l))
}

/// A pack version to install from the store: which, from where, and what the reviewed file was.
#[derive(Clone, Debug, PartialEq)]
pub struct Target<'a> {
    pub id: &'a str,
    pub version: &'a str,
    pub listing: u64,
    pub url: &'a str,
    pub sha256: &'a str,
    pub size: u64,
}

impl Listed {
    pub fn target(&self) -> Target<'_> {
        Target {
            id: &self.id,
            version: &self.version,
            listing: self.listing,
            url: &self.url,
            sha256: &self.sha256,
            size: self.size,
        }
    }
}

/// Download `listed` from its author's link into `downloads` and install it into `packs_dir`, only when the file is
/// the one reviewed (size, SHA-256) and its `pack.ron` is the listed pack at the listed version. The download is
/// deleted afterwards either way.
pub fn install(
    listed: &Listed,
    downloads: &Path,
    packs_dir: &Path,
    progress: &Progress,
) -> Result<qwc_packs::Pack, String> {
    install_target(&listed.target(), downloads, packs_dir, progress)
}

/// [`install`] for any pack version from the store (a listing, a collection's pack).
pub fn install_target(
    t: &Target<'_>,
    downloads: &Path,
    packs_dir: &Path,
    progress: &Progress,
) -> Result<qwc_packs::Pack, String> {
    if !allowed(t.url) {
        return Err(format!("{}: only https:// links are downloaded", t.url));
    }
    if t.sha256.len() != 64 || !t.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("the store's listing has no proper checksum".into());
    }
    std::fs::create_dir_all(downloads).map_err(|e| format!("{}: {e}", downloads.display()))?;
    // the file named by the listing, not by the author's link
    let file = downloads.join(format!("store-{}.qwcpack", t.listing));
    let fetched = crate::updates::fetch_checked(t.url, &file, Some(t.size), t.sha256, progress);
    let installed = fetched.and_then(|()| {
        let m = qwc_packs::archive_manifest(&file)?;
        if (m.id.as_str(), m.version.as_str()) != (t.id, t.version) {
            return Err(format!(
                "the file is {} {}, not the listed {} {}: not installed",
                m.id, m.version, t.id, t.version
            ));
        }
        qwc_packs::install(&file, packs_dir)
    });
    let _ = std::fs::remove_file(&file);
    installed
}

/// What a reviewer's check of a submitted file found: the file kept (to install and try), its manifest, what is in
/// it, and the problems QWC Studio's checks find in it.
#[derive(Clone, Debug)]
pub struct Checked {
    pub file: PathBuf,
    pub manifest: qwc_packs::Manifest,
    /// Its slots counted by kind ("46 texture, 3 movie").
    pub fills: String,
    pub problems: Vec<(qwc_packs::Severity, String)>,
}

/// Download submission `t` as a reviewer: only the file submitted (its size and SHA-256), whose `pack.ron` names the
/// submitted pack and version; unpacked to a scratch folder for the pack's own checks, then the scratch removed.
pub fn check_submission(t: &Target<'_>, downloads: &Path, progress: &Progress) -> Result<Checked, String> {
    if !allowed(t.url) {
        return Err(format!("{}: only https:// links are downloaded", t.url));
    }
    std::fs::create_dir_all(downloads).map_err(|e| format!("{}: {e}", downloads.display()))?;
    let file = downloads.join(format!("review-{}.qwcpack", t.listing));
    crate::updates::fetch_checked(t.url, &file, Some(t.size), t.sha256, progress)?;
    let manifest = qwc_packs::archive_manifest(&file)?;
    if (manifest.id.as_str(), manifest.version.as_str()) != (t.id, t.version) {
        return Err(format!(
            "the file is {} {}, not the submitted {} {}",
            manifest.id, manifest.version, t.id, t.version
        ));
    }
    let scratch = downloads.join(format!("review-{}", t.listing));
    let _ = std::fs::remove_dir_all(&scratch);
    let opened = qwc_packs::install(&file, &scratch);
    let checked = opened.map(|pack| {
        let mut by: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for slot in pack.slots() {
            *by.entry(slot.split('/').next().unwrap_or(&slot).to_string()).or_default() += 1;
        }
        let fills = if by.is_empty() {
            "nothing the remaster reads".to_string()
        } else {
            by.iter().map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join(", ")
        };
        Checked { file: file.clone(), manifest: manifest.clone(), fills, problems: pack.check() }
    });
    let _ = std::fs::remove_dir_all(&scratch);
    checked
}

/// One approved version of a pack (`/packs/{id}`'s versions).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Version {
    pub listing: u64,
    pub version: String,
}

/// A pack's page in the store: its approved versions, newest first.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PackPage {
    pub id: String,
    pub versions: Vec<Version>,
}

/// A pack's page; none when the store does not list it.
pub fn pack(base: &str, id: &str) -> Result<Option<PackPage>, String> {
    let mut res = crate::updates::agent()
        .get(&format!("{}/packs/{id}", base.trim_end_matches('/')))
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| format!("Could not reach the store: {e}"))?;
    if res.status().as_u16() == 404 {
        return Ok(None);
    }
    let text = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map(Some).map_err(|e| format!("The store's answer does not read: {e}"))
}

/// A collection as the store lists it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CollectionSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub author: Author,
    pub revision: u32,
    pub packs: u32,
    /// Every pack in it still listed.
    pub complete: bool,
    pub updated: String,
}

/// One pack of a collection, the highest first.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CollectionItem {
    pub enabled: bool,
    pub listing: u64,
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: String,
    #[serde(default)]
    pub engine: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// Whether this pack version is still listed.
    pub listed: bool,
}

impl CollectionItem {
    pub fn target(&self) -> Target<'_> {
        Target {
            id: &self.id,
            version: &self.version,
            listing: self.listing,
            url: &self.url,
            sha256: &self.sha256,
            size: self.size,
        }
    }

    pub fn fits(&self, version: &semver::Version) -> bool {
        self.engine.trim().is_empty() || semver::VersionReq::parse(&self.engine).is_ok_and(|r| r.matches(version))
    }
}

/// A collection with its packs.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Collection {
    #[serde(flatten)]
    pub summary: CollectionSummary,
    pub items: Vec<CollectionItem>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CollectionPage {
    pub collections: Vec<CollectionSummary>,
    pub more: bool,
}

/// A page of the store's collections: `words` searched, newest first or by name.
pub fn browse_collections(base: &str, words: &str, by_name: bool, page: u32) -> Result<CollectionPage, String> {
    let mut url = format!(
        "{}/collections?sort={}&page={}",
        base.trim_end_matches('/'),
        if by_name { "name" } else { "new" },
        page.max(1)
    );
    if !words.trim().is_empty() {
        url += &format!("&q={}", escape(words.trim()));
    }
    let mut res = crate::updates::agent().get(&url).call().map_err(|e| format!("Could not reach the store: {e}"))?;
    let text = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("The store's answer does not read: {e}"))
}

/// A collection and its packs.
pub fn collection(base: &str, id: &str) -> Result<Collection, String> {
    let mut res = crate::updates::agent()
        .get(&format!("{}/collections/{id}", base.trim_end_matches('/')))
        .call()
        .map_err(|e| format!("Could not reach the store: {e}"))?;
    let text = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("The store's answer does not read: {e}"))
}

/// What installing a collection did: the packs installed (or updated), those already there, those left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Installed {
    pub installed: Vec<String>,
    /// The listings installed, for the store's install count and the player's rating.
    pub listings: Vec<u64>,
    pub kept: Vec<String>,
    pub skipped: Vec<String>,
}

/// Install every pack of `c` that is not installed at its version (`have`: installed versions by id), each only
/// when its file is the one reviewed; packs no longer listed are left out. Stops at the first that fails.
pub fn install_collection(
    c: &Collection,
    have: &std::collections::BTreeMap<String, String>,
    downloads: &Path,
    packs_dir: &Path,
    progress: &Progress,
) -> Result<Installed, String> {
    let mut done = Installed::default();
    for (n, item) in c.items.iter().enumerate() {
        if !item.listed {
            done.skipped.push(item.name.clone());
            continue;
        }
        if have.get(&item.id) == Some(&item.version) {
            done.kept.push(item.name.clone());
            continue;
        }
        if let Ok(mut w) = progress.what.lock() {
            *w = format!("{} ({} of {})", item.name, n + 1, c.items.len());
        }
        install_target(&item.target(), downloads, packs_dir, progress).map_err(|e| format!("{}: {e}", item.name))?;
        done.installed.push(item.name.clone());
        done.listings.push(item.listing);
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::path::PathBuf;

    use sha2::{Digest, Sha256};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qwc-launcher-store-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A small pack (`me.night` 1.0.0) built into a `.qwcpack`: its bytes.
    fn qwcpack(root: &Path) -> Vec<u8> {
        let mut m = qwc_packs::Manifest::new("me.night", "Night");
        m.version = "1.0.0".into();
        qwc_packs::Pack::create(root.join("src"), m).unwrap();
        let (file, _) = qwc_packs::build(&root.join("src"), &root.join("night.qwcpack")).unwrap();
        std::fs::read(file).unwrap()
    }

    /// `body` served once per connection over HTTP on this computer, for `n` connections: its address.
    fn serve(body: Vec<u8>, n: usize) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/night.qwcpack", listener.local_addr().unwrap().port());
        std::thread::spawn(move || {
            for stream in listener.incoming().take(n) {
                let mut s = stream.unwrap();
                let mut req = [0u8; 4096];
                let _ = s.read(&mut req);
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&body);
            }
        });
        url
    }

    fn listed(url: String, bytes: &[u8]) -> Listed {
        Listed {
            id: "me.night".into(),
            name: "Night".into(),
            version: "1.0.0".into(),
            kind: "Visual".into(),
            description: String::new(),
            author: Author { id: "a1".into(), name: "Me".into() },
            engine: String::new(),
            licence: "CC BY".into(),
            listing: 7,
            url,
            sha256: Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect(),
            size: bytes.len() as u64,
            rating: Rating { average: None, count: 0 },
            installs: 0,
            updated: String::new(),
        }
    }

    /// Against the store on this computer with its sample packs built and served (`store\dev.cmd`):
    /// `QWC_STORE_URL=http://127.0.0.1:8787 cargo test -p qwc-launcher against_a_running_store -- --ignored`.
    #[test]
    #[ignore = "needs a running store (QWC_STORE_URL)"]
    fn against_a_running_store() {
        let base = std::env::var("QWC_STORE_URL").expect("QWC_STORE_URL");
        let page = browse(&base, &Query { words: "floodlit".into(), ..Query::default() }).unwrap();
        let night = page.packs.iter().find(|p| p.id == "ana.floodlights").expect("the sample ana.floodlights");
        let root = scratch("live");
        let pack = install(night, &root.join("downloads"), &root.join("packs"), &Progress::default()).unwrap();
        assert_eq!((pack.manifest.id.as_str(), pack.manifest.version.as_str()), ("ana.floodlights", "1.1.0"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn installs_a_collection() {
        let root = scratch("collection");
        let bytes = qwcpack(&root);
        let url = serve(bytes.clone(), 2);
        let good = listed(url, &bytes);
        let item = |id: &str, listed: bool| CollectionItem {
            enabled: true,
            listing: good.listing,
            id: id.into(),
            name: format!("Pack {id}"),
            version: "1.0.0".into(),
            kind: "Visual".into(),
            engine: String::new(),
            url: good.url.clone(),
            sha256: good.sha256.clone(),
            size: good.size,
            listed,
        };
        let c = Collection {
            summary: CollectionSummary {
                id: "c1".into(),
                name: "Night set".into(),
                description: String::new(),
                author: Author { id: "a1".into(), name: "Me".into() },
                revision: 1,
                packs: 2,
                complete: false,
                updated: String::new(),
            },
            items: vec![item("me.night", true), item("me.gone", false)],
        };
        let (downloads, packs) = (root.join("downloads"), root.join("packs"));
        let mut have = std::collections::BTreeMap::new();
        let done = install_collection(&c, &have, &downloads, &packs, &Progress::default()).unwrap();
        assert_eq!(done.installed, ["Pack me.night"]);
        assert_eq!(done.skipped, ["Pack me.gone"]);
        assert!(packs.join("me.night").join("pack.ron").is_file());
        // already there at its version: kept, not downloaded again
        have.insert("me.night".to_string(), "1.0.0".to_string());
        let again = install_collection(&c, &have, &downloads, &packs, &Progress::default()).unwrap();
        assert_eq!((again.installed.len(), again.kept.len()), (0, 1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn installs_only_the_file_reviewed() {
        let root = scratch("install");
        let bytes = qwcpack(&root);
        let (downloads, packs) = (root.join("downloads"), root.join("packs"));
        let url = serve(bytes.clone(), 5);
        let good = listed(url.clone(), &bytes);
        let progress = Progress::default();

        // another checksum, size, pack or version: refused, nothing installed, nothing left in downloads
        let other_hash = Listed { sha256: "0".repeat(64), ..good.clone() };
        assert!(install(&other_hash, &downloads, &packs, &progress).unwrap_err().contains("checksum"));
        let smaller = Listed { size: good.size - 1, ..good.clone() };
        assert!(install(&smaller, &downloads, &packs, &progress).unwrap_err().contains("larger"));
        let other_pack = Listed { id: "me.day".into(), ..good.clone() };
        assert!(install(&other_pack, &downloads, &packs, &progress).unwrap_err().contains("not the listed"));
        let other_version = Listed { version: "2.0.0".into(), ..good.clone() };
        assert!(install(&other_version, &downloads, &packs, &progress).unwrap_err().contains("not the listed"));
        assert!(!packs.join("me.night").exists());
        assert_eq!(std::fs::read_dir(&downloads).unwrap().count(), 0);

        // the file reviewed: installed, the download gone
        let pack = install(&good, &downloads, &packs, &progress).unwrap();
        assert_eq!((pack.manifest.id.as_str(), pack.manifest.version.as_str()), ("me.night", "1.0.0"));
        assert!(packs.join("me.night").join("pack.ron").is_file());
        assert_eq!(std::fs::read_dir(&downloads).unwrap().count(), 0);

        // not https, and not this computer: never fetched
        let elsewhere = Listed { url: "http://example.com/night.qwcpack".into(), ..good };
        assert!(install(&elsewhere, &downloads, &packs, &progress).unwrap_err().contains("https"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_page_address() {
        let q = Query { words: "night & day".into(), kind: Some("Visual"), sort: Sort::Rating, page: 2 };
        assert_eq!(
            page_url("https://store.test/", &q),
            "https://store.test/packs?sort=rating&page=2&q=night%20%26%20day&kind=Visual"
        );
        assert_eq!(page_url("https://store.test", &Query::default()), "https://store.test/packs?sort=new&page=1");
    }

    #[test]
    fn only_https_or_this_computer() {
        assert!(allowed("https://github.com/a/b/releases/download/v1/p.qwcpack"));
        assert!(allowed("http://127.0.0.1:8788/p.qwcpack"));
        assert!(!allowed("http://example.com/p.qwcpack"));
        assert!(!allowed("http://127.0.0.1.example.com/p.qwcpack"));
        assert!(!allowed("file:///C:/p.qwcpack"));
    }

    #[test]
    fn a_page_reads() {
        let page: Page = serde_json::from_str(
            r#"{"packs": [{"id": "ana.night", "name": "Night matches", "version": "1.1.0", "kind": "Visual",
                "description": "Floodlights", "author": {"id": "a1", "name": "Ana"}, "engine": ">=0.1",
                "requires": [], "conflicts": [], "licence": "CC BY", "listing": 7,
                "url": "https://example.invalid/n.qwcpack", "sha256": "aa", "size": 1234, "screenshots": [],
                "rating": {"average": 4.5, "count": 2}, "installs": 3, "updated": "2026-10-20T00:00:00Z"}],
                "more": false}"#,
        )
        .unwrap();
        let p = &page.packs[0];
        assert_eq!((p.id.as_str(), p.listing, p.size), ("ana.night", 7, 1234));
        assert_eq!(p.rating.average, Some(4.5));
        assert!(p.fits(&semver::Version::new(0, 1, 1)));
        assert!(!Listed { engine: ">=0.2".into(), ..p.clone() }.fits(&semver::Version::new(0, 1, 1)));
    }
}
