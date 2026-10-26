//! The player's account in the pack store (`docs/PACK_STORE.md`, "Accounts"): signing in with Discord through the
//! browser, the session token kept for this user (encrypted for the Windows user with DPAPI; a file only the user
//! can read elsewhere), the account and the public profiles read and changed, and a picture made ready to upload.
//!
//! Signing in: the launcher listens on a one-time port of this computer, opens the browser on the store's
//! `/auth/discord` with the port and a PKCE challenge, and waits for the browser to come back to
//! `/signed-in?state=..&code=..`; the code and the verifier (which never left the launcher) buy a session.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::store::{CollectionSummary, Listed};

/// The player's own account, as the store gives it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Me {
    pub id: String,
    pub name: String,
    pub discord_username: Option<String>,
    pub show_discord: bool,
    /// Whether the picture is one they uploaded (else their Discord avatar).
    pub uploaded_avatar: bool,
    pub role: String,
    pub joined: String,
    /// When the name may next change; none if it may now.
    pub name_change_after: Option<String>,
}

/// A profile as anyone sees it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// Only when its player chose to show it.
    pub discord_username: Option<String>,
    pub joined: String,
    pub role: String,
    pub packs: Vec<Listed>,
    #[serde(default)]
    pub collections: Vec<CollectionSummary>,
}

/// A store call that failed: not signed in (the token is no longer good), or anything else, said for the player.
#[derive(Clone, Debug, PartialEq)]
pub enum Fail {
    SignedOut,
    Said(String),
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fail::SignedOut => f.write_str("You're signed out; log in again."),
            Fail::Said(s) => f.write_str(s),
        }
    }
}

impl From<Fail> for String {
    fn from(f: Fail) -> String {
        f.to_string()
    }
}

/// The store's agent: its answers read whatever their status, so its `{"error": ...}` can be shown.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .user_agent(concat!("qwc-launcher/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// An answer's body when it is a success; its `error` (or its status) when it is not.
fn answer(res: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Vec<u8>, Fail> {
    let mut res = res.map_err(|e| Fail::Said(format!("Could not reach the store: {e}")))?;
    let status = res.status().as_u16();
    // No Content has nothing to read (and may still be marked gzip, which an empty body is not)
    if status == 204 {
        return Ok(Vec::new());
    }
    let body = res.body_mut().with_config().limit(8 << 20).read_to_vec().map_err(|e| Fail::Said(e.to_string()))?;
    match status {
        200..=299 => Ok(body),
        401 => Err(Fail::SignedOut),
        _ => {
            // every problem when the store lists them (a collection's), else its error
            let v = serde_json::from_slice::<serde_json::Value>(&body).ok();
            let problems = v
                .as_ref()
                .and_then(|v| v.get("problems")?.as_array().cloned())
                .map(|p| p.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("\n"));
            let said = problems.or_else(|| v?.get("error")?.as_str().map(str::to_string));
            Err(Fail::Said(said.unwrap_or_else(|| format!("The store answered {status}."))))
        }
    }
}

fn read<T: serde::de::DeserializeOwned>(body: Vec<u8>) -> Result<T, Fail> {
    serde_json::from_slice(&body).map_err(|e| Fail::Said(format!("The store's answer does not read: {e}")))
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// `bytes` as base64url without padding.
pub(crate) fn b64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut s = String::with_capacity(bytes.len() * 4 / 3 + 2);
    for c in bytes.chunks(3) {
        let n =
            (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..=c.len() {
            s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    s
}

/// A PKCE verifier (32 random bytes, 43 characters) and its S256 challenge.
fn pkce() -> Result<(String, String), String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("no random numbers: {e}"))?;
    let verifier = b64url(&bytes);
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    Ok((verifier, challenge))
}

/// The page the browser shows once it has come back to the launcher, in the launcher's look: the wood behind, a
/// leather card in the gold frame (`corner.png` at each corner, mirrored, joined by `gold_line.png` one pixel in, at
/// the corner's scale: 48 of its 128 pixels), Cinzel for the heading and Cardo for the text. The art and the fonts
/// come from the launcher's own files ([`serve_page_files`]); without them the colours alone stand in.
const SIGNED_IN_PAGE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Signed in - QWC Remastered</title>
<style>
@font-face { font-family: "Cinzel"; src: url("/fonts/Cinzel-Bold.ttf") format("truetype"); font-weight: 700; }
@font-face { font-family: "Cardo"; src: url("/fonts/Cardo-Regular.ttf") format("truetype"); }
:root { --c: 48px; --band: calc(var(--c) * 5 / 128); --in: calc(var(--c) / 128); }
* { box-sizing: border-box; }
html, body { height: 100%; }
body { margin: 0; display: flex; align-items: center; justify-content: center; padding: 24px 16px; color: #efe4cc;
  font: 19px/1.5 "Cardo", Georgia, serif;
  background: linear-gradient(rgba(0,0,0,.42), rgba(0,0,0,.42)), url("/art/backdrop.png") center / cover fixed, #1a130d; }
main { position: relative; width: min(560px, 100%); padding: clamp(40px, 9vw, 52px) clamp(28px, 8vw, 56px); text-align: center;
  background: linear-gradient(rgba(20,14,9,.35), rgba(20,14,9,.35)), url("/art/panel.png") center / cover, #1f1812;
  box-shadow: 0 18px 60px rgba(0,0,0,.7); }
.corner { position: absolute; width: var(--c); height: var(--c);
  background: url("/art/corner.png") 0 0 / 100% 100% no-repeat; }
.tl { top: 0; left: 0; }
.tr { top: 0; right: 0; transform: scaleX(-1); }
.bl { bottom: 0; left: 0; transform: scaleY(-1); }
.br { bottom: 0; right: 0; transform: scale(-1, -1); }
.edge { position: absolute; }
.top, .bottom { left: var(--c); right: var(--c); height: var(--band);
  background: url("/art/gold_line.png") 0 0 / auto 100% repeat-x; }
.top { top: var(--in); }
.bottom { bottom: var(--in); transform: scaleY(-1); }
.left, .right { top: var(--c); bottom: var(--c); width: var(--band);
  background: url("/art/gold_line_turned.png") 0 0 / 100% auto repeat-y; }
.left { left: var(--in); }
.right { right: var(--in); transform: scaleX(-1); }
h1 { font: 700 clamp(25px, 7vw, 34px)/1.2 "Cinzel", Georgia, serif; color: #d4a64a; letter-spacing: .03em; margin: 0 0 12px;
  text-shadow: 0 2px 6px rgba(0,0,0,.6); }
p { margin: 0; color: #efe4cc; }
.dim { color: #a89a82; font-size: 16px; margin-top: 10px; }
</style></head>
<body><main>
<div class="corner tl"></div><div class="corner tr"></div><div class="corner bl"></div><div class="corner br"></div>
<div class="edge top"></div><div class="edge bottom"></div><div class="edge left"></div><div class="edge right"></div>
<h1>You're signed in</h1>
<p>You can close this tab and go back to the launcher.</p>
<p class="dim">QWC Remastered pack store</p>
</main></body></html>
"#;

/// How long the launcher keeps answering the page's requests for its art and fonts.
const PAGE_FILES_FOR: Duration = Duration::from_secs(60);

/// A file the signed-in page asks for, from the launcher's shipped files under `root`: its bytes and type. Only these
/// names; `gold_line_turned.png` is `gold_line.png` turned upright, for the frame's sides.
fn page_file(root: &Path, path: &str) -> Option<(Vec<u8>, &'static str)> {
    let art = root.join("crates/qwc-launcher/art");
    let fonts = root.join("remastered/fonts");
    let file = match path {
        "/art/backdrop.png" | "/art/panel.png" | "/art/corner.png" | "/art/gold_line.png" => {
            art.join(&path["/art/".len()..])
        }
        "/art/gold_line_turned.png" => {
            let (rgba, w, h) = decode_image(&std::fs::read(art.join("gold_line.png")).ok()?).ok()?;
            // pixel (x, y) to (y, x): the band's rows become columns, the outermost on the left
            let mut turned = vec![0u8; rgba.len()];
            for y in 0..h {
                for x in 0..w {
                    let (from, to) = (((y * w + x) * 4) as usize, ((x * h + y) * 4) as usize);
                    turned[to..to + 4].copy_from_slice(&rgba[from..from + 4]);
                }
            }
            let mut out = Vec::new();
            let mut enc = png::Encoder::new(&mut out, h, w);
            enc.set_color(png::ColorType::Rgba);
            enc.write_header().ok()?.write_image_data(&turned).ok()?;
            return Some((out, "image/png"));
        }
        "/fonts/Cinzel-Bold.ttf" | "/fonts/Cardo-Regular.ttf" => fonts.join(&path["/fonts/".len()..]),
        _ => return None,
    };
    let kind = if path.ends_with(".png") { "image/png" } else { "font/ttf" };
    Some((std::fs::read(file).ok()?, kind))
}

/// Answer the signed-in page's requests for its art and fonts on `listener` for a while, from `root`, then close.
fn serve_page_files(listener: TcpListener, root: PathBuf) {
    std::thread::spawn(move || {
        let until = Instant::now() + PAGE_FILES_FOR;
        while Instant::now() < until {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut buf = [0u8; 4096];
                    let Ok(n) = stream.read(&mut buf) else { continue };
                    let head = String::from_utf8_lossy(&buf[..n]);
                    let path = head.lines().next().and_then(|l| l.split(' ').nth(1)).unwrap_or("").to_string();
                    let reply = match page_file(&root, &path) {
                        Some((bytes, kind)) => {
                            let head = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
                                 Cache-Control: max-age=3600\r\nConnection: close\r\n\r\n",
                                bytes.len()
                            );
                            [head.into_bytes(), bytes].concat()
                        }
                        None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                    };
                    let _ = stream.write_all(&reply);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
    });
}

/// The query's value for `key` in a request's path (`/signed-in?state=..&code=..`), percent-decoded.
fn query_value(path: &str, key: &str) -> Option<String> {
    let q = path.split_once('?')?.1;
    let raw = q.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?;
    let mut out = Vec::new();
    let b = raw.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// The browser's request on the loopback port: `Some((state, code))` for the sign-in's return, none for anything else
/// (a favicon); answered either way.
fn take_return(mut stream: TcpStream) -> Option<(String, String)> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).ok()?;
    let head = String::from_utf8_lossy(&buf[..n]);
    let path = head.lines().next()?.split(' ').nth(1)?.to_string();
    let back = path
        .starts_with("/signed-in?")
        .then(|| Some((query_value(&path, "state")?, query_value(&path, "code")?)))
        .flatten();
    let reply = if back.is_some() {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            SIGNED_IN_PAGE.len(),
            SIGNED_IN_PAGE
        )
    } else {
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
    };
    let _ = stream.write_all(reply.as_bytes());
    back
}

/// How long a sign-in waits for the browser.
const SIGN_IN_WAIT: Duration = Duration::from_secs(5 * 60);

#[derive(Deserialize)]
struct Session {
    token: String,
    account: Me,
}

/// Sign in to the store at `base`: `open` shows the player the store's sign-in address (in their browser), then the
/// launcher waits (up to five minutes, or until `cancel`) for the browser to come back, and trades the code for a
/// session. The page the browser then shows takes its art and fonts from the launcher's files under `root`, when
/// given. Answers the session token and the account.
pub fn sign_in(
    base: &str,
    cancel: &AtomicBool,
    root: Option<PathBuf>,
    open: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(String, Me), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("no port for the sign-in: {e}"))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let (verifier, challenge) = pkce()?;
    open(&format!("{}/auth/discord?port={port}&challenge={challenge}", base.trim_end_matches('/')))?;
    let start = Instant::now();
    let (state, code) = loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("stopped".into());
        }
        if start.elapsed() > SIGN_IN_WAIT {
            return Err("The sign-in took too long; try again.".into());
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                if let Some(back) = take_return(stream) {
                    break back;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e.to_string()),
        }
    };
    if let Some(root) = root {
        serve_page_files(listener, root);
    }
    let body = serde_json::json!({ "state": state, "code": code, "verifier": verifier }).to_string();
    let res = agent()
        .post(&format!("{}/auth/token", base.trim_end_matches('/')))
        .header("content-type", "application/json")
        .send(body.as_bytes());
    let s: Session = read(answer(res)?)?;
    Ok((s.token, s.account))
}

fn url_of(base: &str, path: &str) -> String {
    format!("{}{path}", base.trim_end_matches('/'))
}

/// The signed-in player's account.
pub fn me(base: &str, token: &str) -> Result<Me, Fail> {
    read(answer(agent().get(&url_of(base, "/me")).header("authorization", &bearer(token)).call())?)
}

/// Change the store name and/or whether the Discord username shows; the account as it is now.
pub fn edit(base: &str, token: &str, name: Option<&str>, show_discord: Option<bool>) -> Result<Me, Fail> {
    let mut change = serde_json::Map::new();
    if let Some(n) = name {
        change.insert("name".into(), n.into());
    }
    if let Some(s) = show_discord {
        change.insert("show_discord".into(), s.into());
    }
    let body = serde_json::Value::Object(change).to_string();
    let res = agent()
        .put(&url_of(base, "/me"))
        .header("authorization", &bearer(token))
        .header("content-type", "application/json")
        .send(body.as_bytes());
    read(answer(res)?)
}

/// End the session on the store.
pub fn log_out(base: &str, token: &str) -> Result<(), Fail> {
    let res = agent().post(&url_of(base, "/auth/logout")).header("authorization", &bearer(token)).send_empty();
    answer(res).map(|_| ())
}

/// Delete the account (its personal data; its packs unlisted).
pub fn delete(base: &str, token: &str) -> Result<(), Fail> {
    answer(agent().delete(&url_of(base, "/me")).header("authorization", &bearer(token)).call()).map(|_| ())
}

/// A profile, as anyone sees it.
pub fn profile(base: &str, id: &str) -> Result<Profile, Fail> {
    read(answer(agent().get(&url_of(base, &format!("/accounts/{id}"))).call())?)
}

/// A profile picture's bytes (PNG or JPEG), through the store.
pub fn picture(base: &str, id: &str) -> Result<Vec<u8>, Fail> {
    answer(agent().get(&url_of(base, &format!("/avatars/{id}"))).call())
}

/// Upload a picture made by [`prepare_picture`].
pub fn upload_picture(base: &str, token: &str, bytes: &[u8], mime: &str) -> Result<(), Fail> {
    let res = agent()
        .put(&url_of(base, "/me/avatar"))
        .header("authorization", &bearer(token))
        .header("content-type", mime)
        .send(bytes);
    answer(res).map(|_| ())
}

/// Back to the Discord avatar.
pub fn reset_picture(base: &str, token: &str) -> Result<(), Fail> {
    answer(agent().delete(&url_of(base, "/me/avatar")).header("authorization", &bearer(token)).call()).map(|_| ())
}

/// A collection's packs as the store takes them: each listing, on or off, the highest first.
fn items_json(items: &[(u64, bool)]) -> serde_json::Value {
    items.iter().map(|(listing, enabled)| serde_json::json!({ "listing": listing, "enabled": enabled })).collect()
}

/// Publish a collection: its name, description and packs; its id.
pub fn publish_collection(
    base: &str,
    token: &str,
    name: &str,
    description: &str,
    items: &[(u64, bool)],
) -> Result<String, Fail> {
    let body = serde_json::json!({ "name": name, "description": description, "items": items_json(items) }).to_string();
    let res = agent()
        .post(&url_of(base, "/collections"))
        .header("authorization", &bearer(token))
        .header("content-type", "application/json")
        .send(body.as_bytes());
    let made: serde_json::Value = read(answer(res)?)?;
    made.get("id")
        .and_then(|i| i.as_str())
        .map(str::to_string)
        .ok_or(Fail::Said("The store made no collection.".into()))
}

/// Change your collection `id`: its packs (a new revision), when given.
pub fn revise_collection(base: &str, token: &str, id: &str, items: &[(u64, bool)]) -> Result<(), Fail> {
    let body = serde_json::json!({ "items": items_json(items) }).to_string();
    let res = agent()
        .put(&url_of(base, &format!("/collections/{id}")))
        .header("authorization", &bearer(token))
        .header("content-type", "application/json")
        .send(body.as_bytes());
    answer(res).map(|_| ())
}

/// Take your collection `id` out of the store.
pub fn delete_collection(base: &str, token: &str, id: &str) -> Result<(), Fail> {
    answer(agent().delete(&url_of(base, &format!("/collections/{id}"))).header("authorization", &bearer(token)).call())
        .map(|_| ())
}

// ---------------------------------------------------------------------------------------------------------------
// Submitting, rating, reporting, reviewing

/// A `.qwcpack` read for submission: its SHA-256, its size and its manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct PackFile {
    pub sha256: String,
    pub size: u64,
    pub manifest: qwc_packs::Manifest,
}

/// Read the `.qwcpack` at `path` for submission.
pub fn read_pack_file(path: &Path) -> Result<PackFile, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let manifest = qwc_packs::archive_manifest(path)?;
    let sha256 = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    Ok(PackFile { sha256, size: bytes.len() as u64, manifest })
}

/// Submit pack file `f`, hosted at `url`, for review: the listing's id.
pub fn submit(
    base: &str,
    token: &str,
    f: &PackFile,
    url: &str,
    description: &str,
    screenshots: &[String],
) -> Result<u64, Fail> {
    let m = &f.manifest;
    let body = serde_json::json!({
        "url": url,
        "sha256": f.sha256,
        "size": f.size,
        "description": description,
        "screenshots": screenshots,
        "manifest": {
            "id": m.id, "name": m.name, "version": m.version,
            "kind": if m.kind == qwc_packs::Kind::Gameplay { "Gameplay" } else { "Visual" },
            "engine": m.engine, "requires": m.requires, "conflicts": m.conflicts, "description": m.description,
            "declaration": m.declaration.as_ref().map(|d| serde_json::json!({
                "own_work": d.own_work, "recipes": d.recipes, "licence": d.licence, "credits": d.credits
            })),
        },
    })
    .to_string();
    let res = agent()
        .post(&url_of(base, "/listings"))
        .header("authorization", &bearer(token))
        .header("content-type", "application/json")
        .send(body.as_bytes());
    let made: serde_json::Value = read(answer(res)?)?;
    made.get("listing").and_then(serde_json::Value::as_u64).ok_or(Fail::Said("The store took no submission.".into()))
}

/// One of your submissions and where it stands.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct MySubmission {
    pub listing: u64,
    pub id: String,
    pub name: String,
    pub version: String,
    /// `pending`, `approved`, `rejected` or `removed`.
    pub status: String,
    /// The reviewer's note (why it was rejected or removed).
    pub note: Option<String>,
    pub submitted: String,
}

pub fn my_submissions(base: &str, token: &str) -> Result<Vec<MySubmission>, Fail> {
    read(answer(agent().get(&url_of(base, "/me/listings")).header("authorization", &bearer(token)).call())?)
}

/// One of your ratings: the pack's latest.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct MyRating {
    pub pack: String,
    pub listing: u64,
    pub stars: u8,
}

pub fn my_ratings(base: &str, token: &str) -> Result<Vec<MyRating>, Fail> {
    read(answer(agent().get(&url_of(base, "/me/ratings")).header("authorization", &bearer(token)).call())?)
}

fn send_json(base: &str, token: &str, method: &str, path: &str, body: serde_json::Value) -> Result<Vec<u8>, Fail> {
    let (url, body) = (url_of(base, path), body.to_string());
    let a = agent();
    let req = match method {
        "PUT" => a.put(&url),
        _ => a.post(&url),
    };
    answer(req.header("authorization", &bearer(token)).header("content-type", "application/json").send(body.as_bytes()))
}

/// Tell the store you installed listing `listing` from it (checked): what lets you rate it.
pub fn record_install(base: &str, token: &str, listing: u64) -> Result<(), Fail> {
    send_json(base, token, "POST", "/installs", serde_json::json!({ "listing": listing })).map(|_| ())
}

/// Rate listing `listing` 1 to 5 stars.
pub fn rate(base: &str, token: &str, listing: u64, stars: u8) -> Result<(), Fail> {
    send_json(base, token, "PUT", &format!("/ratings/{listing}"), serde_json::json!({ "stars": stars })).map(|_| ())
}

/// The reasons a report gives, as the store names them, and as the launcher says them.
pub const REASONS: [(&str, &str); 5] = [
    ("not-original", "Not the author's own work (the game's files, or someone else's)"),
    ("broken", "Broken or doesn't work"),
    ("harmful", "Harmful or offensive"),
    ("changed", "The file changed after review"),
    ("other", "Something else"),
];

/// Report `kind` (`listing`, `collection`, `account`) `target` to the reviewers.
pub fn report(base: &str, token: &str, kind: &str, target: &str, reason: &str, note: &str) -> Result<(), Fail> {
    // a listing's id is a number to the store
    let target: serde_json::Value = target.parse::<u64>().map_or_else(|_| target.into(), Into::into);
    let body = serde_json::json!({ "target_kind": kind, "target_id": target, "reason": reason, "note": note });
    send_json(base, token, "POST", "/reports", body).map(|_| ())
}

/// A pack version waiting for review, with everything a reviewer checks.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Pending {
    pub listing: u64,
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: String,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub description: String,
    pub licence: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub screenshots: Vec<String>,
    pub provenance: Provenance,
    pub submitted: String,
    pub author: crate::store::Author,
    /// How many versions of the pack are listed already (0: a new pack).
    pub listed_before: u32,
}

/// The author's statement, as submitted.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Provenance {
    #[serde(default)]
    pub own_work: bool,
    #[serde(default)]
    pub recipes: bool,
    #[serde(default)]
    pub credits: String,
}

impl Pending {
    pub fn target(&self) -> crate::store::Target<'_> {
        crate::store::Target {
            id: &self.id,
            version: &self.version,
            listing: self.listing,
            url: &self.url,
            sha256: &self.sha256,
            size: self.size,
        }
    }
}

pub fn review_queue(base: &str, token: &str) -> Result<Vec<Pending>, Fail> {
    read(answer(agent().get(&url_of(base, "/review/queue")).header("authorization", &bearer(token)).call())?)
}

/// Approve or reject a pending listing; a rejection's note is what its author reads.
pub fn decide(base: &str, token: &str, listing: u64, approve: bool, note: &str) -> Result<(), Fail> {
    let body = serde_json::json!({ "decision": if approve { "approve" } else { "reject" }, "note": note });
    send_json(base, token, "POST", &format!("/review/{listing}"), body).map(|_| ())
}

/// An open report.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct OpenReport {
    pub id: u64,
    pub target_kind: String,
    pub target_id: String,
    pub reason: String,
    #[serde(default)]
    pub note: String,
    pub at: String,
    pub reporter: Option<String>,
    /// What it is about: the pack and version, the collection's or the player's name.
    pub about: Option<String>,
    /// The listing's or collection's status, or the account's state.
    pub state: Option<String>,
}

pub fn open_reports(base: &str, token: &str) -> Result<Vec<OpenReport>, Fail> {
    read(answer(agent().get(&url_of(base, "/review/reports")).header("authorization", &bearer(token)).call())?)
}

/// Act on report `id`: `dismiss`, `remove` (a listing or collection), `reset-picture` or `close` (an account).
pub fn resolve_report(base: &str, token: &str, id: u64, action: &str) -> Result<(), Fail> {
    send_json(base, token, "POST", &format!("/review/reports/{id}"), serde_json::json!({ "action": action }))
        .map(|_| ())
}

/// An admin makes account `id` a reviewer (or a player again).
pub fn set_role(base: &str, token: &str, id: &str, reviewer: bool) -> Result<(), Fail> {
    let role = if reviewer { "reviewer" } else { "player" };
    send_json(base, token, "PUT", &format!("/accounts/{id}/role"), serde_json::json!({ "role": role })).map(|_| ())
}

// ---------------------------------------------------------------------------------------------------------------
// The session token, kept for this user

/// The file the token is kept in off Windows (and was, encrypted, on Windows before 0.1.3).
fn token_path(library: &Path) -> PathBuf {
    library.join("account.token")
}

/// The token's name in Windows Credential Manager: one per library.
#[cfg(windows)]
fn vault_name(library: &Path) -> String {
    format!("QWC Remastered pack store ({})", library.display())
}

/// Windows Credential Manager, where programs keep their sign-ins (Git, the GitHub CLI and VS Code keep theirs
/// there): the Windows user's own, on this computer. Kept there rather than in a file this program decrypts at start,
/// which security software's heuristics took for a password stealer (0.1.2, `Trojan:Win32/Bearfoos.A!ml`).
#[cfg(windows)]
mod vault {
    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    pub fn write(name: &str, secret: &[u8]) -> Result<(), String> {
        let mut wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let mut blob = secret.to_vec();
        let cred = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(wide.as_mut_ptr()),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        // SAFETY: the name and the blob live for the call, which copies them
        unsafe { CredWriteW(&cred, 0) }.map_err(|e| format!("Windows Credential Manager: {e}"))
    }

    pub fn read(name: &str) -> Option<Vec<u8>> {
        let mut found: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: the call fills `found`, which is read and then freed with CredFree
        unsafe {
            CredReadW(&HSTRING::from(name), CRED_TYPE_GENERIC, None, &mut found).ok()?;
            let c = &*found;
            let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec();
            CredFree(found as *const _);
            Some(bytes)
        }
    }

    pub fn delete(name: &str) {
        // SAFETY: a plain call with a name; none there is no error worth reporting
        let _ = unsafe { CredDeleteW(&HSTRING::from(name), CRED_TYPE_GENERIC, None) };
    }
}

/// Keep the session token: in Windows Credential Manager on Windows; elsewhere a file in the library only the user
/// can read.
pub fn store_token(library: &Path, token: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        // the file earlier versions kept, gone
        let _ = std::fs::remove_file(token_path(library));
        vault::write(&vault_name(library), token.as_bytes())
    }
    #[cfg(not(windows))]
    {
        let path = token_path(library);
        std::fs::create_dir_all(library).map_err(|e| format!("{}: {e}", library.display()))?;
        std::fs::write(&path, token.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(())
    }
}

/// The session token kept, if any. (On Windows, a token 0.1.2 kept in its file is not read: that player signs in
/// again, and the file goes.)
pub fn stored_token(library: &Path) -> Option<String> {
    #[cfg(windows)]
    let bytes = {
        let _ = std::fs::remove_file(token_path(library));
        vault::read(&vault_name(library))?
    };
    #[cfg(not(windows))]
    let bytes = std::fs::read(token_path(library)).ok()?;
    String::from_utf8(bytes).ok().filter(|t| !t.is_empty())
}

pub fn forget_token(library: &Path) {
    let _ = std::fs::remove_file(token_path(library));
    #[cfg(windows)]
    vault::delete(&vault_name(library));
}

// ---------------------------------------------------------------------------------------------------------------
// A picture made ready to upload

/// The side of an uploaded picture, and the most bytes the store takes.
pub const PICTURE_SIDE: u32 = 256;
pub const PICTURE_MAX: usize = 200_000;

/// An image file's pixels as RGBA, with its width and height: PNG or JPEG.
pub fn decode_image(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = dec.read_info().map_err(|e| e.to_string())?;
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
        let px = &buf[..info.buffer_size()];
        let rgba = match info.color_type {
            png::ColorType::Rgba => px.to_vec(),
            png::ColorType::Rgb => px.as_chunks::<3>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
            png::ColorType::GrayscaleAlpha => {
                px.as_chunks::<2>().0.iter().flat_map(|c| [c[0], c[0], c[0], c[1]]).collect()
            }
            png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::Indexed => return Err("an indexed PNG the decoder did not expand".into()),
        };
        return Ok((rgba, info.width, info.height));
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        let mut dec = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
        let px = dec.decode().map_err(|e| e.to_string())?;
        let info = dec.info().ok_or("a JPEG without its size")?;
        let rgba = match info.pixel_format {
            jpeg_decoder::PixelFormat::RGB24 => {
                px.as_chunks::<3>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect()
            }
            jpeg_decoder::PixelFormat::L8 => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            jpeg_decoder::PixelFormat::L16 => {
                px.as_chunks::<2>().0.iter().flat_map(|c| [c[0], c[0], c[0], 255]).collect()
            }
            jpeg_decoder::PixelFormat::CMYK32 => px
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|c| {
                    let k = 255 - u32::from(c[3]);
                    let ch = |v: u8| ((255 - u32::from(v)) * k / 255) as u8;
                    [ch(c[0]), ch(c[1]), ch(c[2]), 255]
                })
                .collect(),
        };
        return Ok((rgba, u32::from(info.width), u32::from(info.height)));
    }
    Err("Choose a PNG or JPEG picture.".into())
}

/// The square of `rgba` (`w` x `h`) as large as fits, its centre moved by `focus` (0 to 1 along the long side, 0.5
/// the middle), scaled down (or up) to `side` x `side` by averaging the pixels each one covers.
pub fn square(rgba: &[u8], w: u32, h: u32, focus: f32, side: u32) -> Vec<u8> {
    let s = w.min(h);
    let (x0, y0) = if w > h {
        (((w - s) as f32 * focus.clamp(0.0, 1.0)).round() as u32, 0)
    } else {
        (0, ((h - s) as f32 * focus.clamp(0.0, 1.0)).round() as u32)
    };
    let scale = s as f32 / side as f32;
    // the source pixels output pixel `o` covers along one side: at least one
    let span = |o: u32| {
        let a = ((o as f32 * scale) as u32).min(s - 1);
        (a, (((o + 1) as f32 * scale).ceil() as u32).clamp(a + 1, s))
    };
    let mut out = vec![0u8; (side * side * 4) as usize];
    for oy in 0..side {
        let (sy0, sy1) = span(oy);
        for ox in 0..side {
            let (sx0, sx1) = span(ox);
            let mut sum = [0u32; 4];
            let mut n = 0u32;
            for y in sy0..sy1 {
                for x in sx0..sx1 {
                    let i = (((y0 + y) * w + x0 + x) * 4) as usize;
                    for c in 0..4 {
                        sum[c] += u32::from(rgba[i + c]);
                    }
                    n += 1;
                }
            }
            let o = ((oy * side + ox) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (sum[c] / n.max(1)) as u8;
            }
        }
    }
    out
}

/// The image file at `path` read for a picture: its RGBA pixels, width and height, at least 64 pixels each way.
pub fn open_picture(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (rgba, w, h) = decode_image(&bytes)?;
    if w < 64 || h < 64 {
        return Err("The picture is too small: at least 64 pixels each way.".into());
    }
    Ok((rgba, w, h))
}

/// The image file at `path` made ready to upload: a centred (moved by `focus`) square, 256 x 256, as a JPEG (a PNG
/// when it has transparency), small enough for the store. Encoding from the pixels leaves everything else in the
/// file behind (a photo's place and camera). Answers the bytes, their type, and the square's pixels for a preview.
pub fn prepare_picture(path: &Path, focus: f32) -> Result<(Vec<u8>, &'static str, Vec<u8>), String> {
    let (rgba, w, h) = open_picture(path)?;
    let sq = square(&rgba, w, h, focus, PICTURE_SIDE);
    let (bytes, mime) = encode_picture(&sq)?;
    Ok((bytes, mime, sq))
}

/// A 256 x 256 square ([`square`]) encoded for the store: a PNG when it has transparency and fits, else a JPEG as
/// good as fits (transparency over a dark background). The bytes and their type.
pub fn encode_picture(sq: &[u8]) -> Result<(Vec<u8>, &'static str), String> {
    let see_through = sq.as_chunks::<4>().0.iter().any(|p| p[3] < 255);
    if see_through {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, PICTURE_SIDE, PICTURE_SIDE);
            enc.set_color(png::ColorType::Rgba);
            enc.set_compression(png::Compression::Best);
            let mut writer = enc.write_header().map_err(|e| e.to_string())?;
            writer.write_image_data(sq).map_err(|e| e.to_string())?;
        }
        if out.len() <= PICTURE_MAX {
            return Ok((out, "image/png"));
        }
    }
    // a JPEG, as good as fits; transparency (a PNG too large) on a dark background
    let rgb: Vec<u8> = sq
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = u32::from(p[3]);
            let over = |c: u8| ((u32::from(c) * a + 0x1f * (255 - a)) / 255) as u8;
            [over(p[0]), over(p[1]), over(p[2])]
        })
        .collect();
    for quality in [85u8, 75, 65, 55, 45] {
        let mut out = Vec::new();
        jpeg_encoder::Encoder::new(&mut out, quality)
            .encode(&rgb, PICTURE_SIDE as u16, PICTURE_SIDE as u16, jpeg_encoder::ColorType::Rgb)
            .map_err(|e| e.to_string())?;
        if out.len() <= PICTURE_MAX {
            return Ok((out, "image/jpeg"));
        }
    }
    Err("The picture doesn't compress small enough.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url() {
        assert_eq!(b64url(b""), "");
        assert_eq!(b64url(b"f"), "Zg");
        assert_eq!(b64url(b"fo"), "Zm8");
        assert_eq!(b64url(b"foo"), "Zm9v");
        assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
        let (v, c) = pkce().unwrap();
        assert_eq!((v.len(), c.len()), (43, 43));
    }

    #[test]
    fn the_signed_in_pages_files() {
        // the repository is laid out as the shipped files: crates/qwc-launcher/art, remastered/fonts
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (path, kind) in [
            ("/art/backdrop.png", "image/png"),
            ("/art/panel.png", "image/png"),
            ("/art/corner.png", "image/png"),
            ("/art/gold_line.png", "image/png"),
            ("/fonts/Cinzel-Bold.ttf", "font/ttf"),
            ("/fonts/Cardo-Regular.ttf", "font/ttf"),
        ] {
            let (bytes, k) = page_file(&root, path).unwrap_or_else(|| panic!("{path}"));
            assert!(!bytes.is_empty() && k == kind, "{path}");
        }
        // the band turned upright: 5 x 1024
        let (turned, _) = page_file(&root, "/art/gold_line_turned.png").unwrap();
        let (_, w, h) = decode_image(&turned).unwrap();
        assert_eq!((w, h), (5, 1024));
        // nothing else, and nothing outside
        for path in ["/art/hero.png", "/art/../../Cargo.toml", "/fonts/../LICENSE", "/", "/art/corner.png?x"] {
            assert!(page_file(&root, path).is_none(), "{path}");
        }
        // the page asks for exactly these
        for path in [
            "/art/backdrop.png",
            "/art/panel.png",
            "/art/corner.png",
            "/art/gold_line.png",
            "/art/gold_line_turned.png",
            "/fonts/Cinzel-Bold.ttf",
            "/fonts/Cardo-Regular.ttf",
        ] {
            assert!(SIGNED_IN_PAGE.contains(path), "{path}");
        }
    }

    #[test]
    fn the_return_query() {
        let p = "/signed-in?state=ab-c_D&code=x%2By%3D&other=1";
        assert_eq!(query_value(p, "state").as_deref(), Some("ab-c_D"));
        assert_eq!(query_value(p, "code").as_deref(), Some("x+y="));
        assert_eq!(query_value(p, "nothing"), None);
    }

    #[test]
    fn a_square_from_the_middle() {
        // 4 x 2: left half black, right half white; the square from the middle is half and half
        let px: Vec<u8> = (0..8).flat_map(|i| if i % 4 < 2 { [0, 0, 0, 255] } else { [255, 255, 255, 255] }).collect();
        let sq = square(&px, 4, 2, 0.5, 2);
        assert_eq!(&sq[..4], &[0, 0, 0, 255]);
        assert_eq!(&sq[4..8], &[255, 255, 255, 255]);
        // moved to the left: all black
        assert!(square(&px, 4, 2, 0.0, 2).chunks(4).all(|p| p[0] == 0));
        // up and down scales keep a single colour
        let one = [10u8, 20, 30, 255].repeat(300 * 300);
        assert!(square(&one, 300, 300, 0.5, 256).chunks(4).all(|p| p == [10, 20, 30, 255]));
        assert!(square(&one[..100 * 100 * 4], 100, 100, 0.5, 256).chunks(4).all(|p| p == [10, 20, 30, 255]));
    }

    #[test]
    fn a_picture_ready_to_upload() {
        let dir = std::env::temp_dir().join(format!("qwc-launcher-account-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // a noisy 640 x 480 photo-like PNG, no transparency: a JPEG under the limit
        let (w, h) = (640u32, 480u32);
        let mut seed = 1u32;
        let px: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                [(i % w) as u8, (i / w) as u8, (seed >> 16) as u8, 255]
            })
            .collect();
        let file = dir.join("photo.png");
        {
            let f = std::fs::File::create(&file).unwrap();
            let mut enc = png::Encoder::new(f, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.write_header().unwrap().write_image_data(&px).unwrap();
        }
        let (bytes, mime, preview) = prepare_picture(&file, 0.5).unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(bytes.len() <= PICTURE_MAX);
        assert_eq!(preview.len(), (PICTURE_SIDE * PICTURE_SIDE * 4) as usize);
        let (_, dw, dh) = decode_image(&bytes).unwrap();
        assert_eq!((dw, dh), (PICTURE_SIDE, PICTURE_SIDE));
        // see-through: a PNG
        let clear: Vec<u8> = px.chunks(4).flat_map(|p| [p[0], p[1], 0, 128]).collect();
        let file = dir.join("clear.png");
        {
            let f = std::fs::File::create(&file).unwrap();
            let mut enc = png::Encoder::new(f, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.write_header().unwrap().write_image_data(&clear).unwrap();
        }
        let (bytes, mime, _) = prepare_picture(&file, 0.5).unwrap();
        assert_eq!(mime, "image/png");
        assert!(bytes.len() <= PICTURE_MAX);
        std::fs::write(dir.join("x.gif"), b"GIF89a").unwrap();
        assert!(prepare_picture(&dir.join("x.gif"), 0.5).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A browser, for the end-to-end test: GET `url`, not following redirects; the status, `Location` and body.
    fn browse(url: &str, form: Option<&str>) -> (u16, Option<String>, String) {
        let agent: ureq::Agent =
            ureq::Agent::config_builder().max_redirects(0).http_status_as_error(false).build().into();
        let mut res = match form {
            Some(f) => agent.post(url).header("content-type", "application/x-www-form-urlencoded").send(f.as_bytes()),
            None => agent.get(url).call(),
        }
        .unwrap();
        let location = res.headers().get("location").and_then(|l| l.to_str().ok()).map(str::to_string);
        (res.status().as_u16(), location, res.body_mut().read_to_string().unwrap_or_default())
    }

    /// Signed in to the store at `base` through the stand-in Discord, as the player's browser does it: its first
    /// made-up player, registered as `name` the first time.
    fn stand_in_sign_in(base: &str, name: &str) -> (String, Me) {
        let browser = |start: &str| -> Result<(), String> {
            let start = start.to_string();
            let name = name.to_string();
            std::thread::spawn(move || {
                // the store sends the browser to Discord, whose page offers its made-up players
                let (status, to_discord, _) = browse(&start, None);
                assert_eq!(status, 302);
                let (_, _, page) = browse(&to_discord.unwrap(), None);
                let href = page.split("href=\"").nth(1).unwrap().split('"').next().unwrap().replace("&amp;", "&");
                // back at the store: the registration form (or, signed in before, straight back)
                let (status, location, body) = browse(&href, None);
                let back = if status == 200 {
                    assert!(body.contains("Your name in the store"));
                    let state = body.split("name=\"state\" value=\"").nth(1).unwrap().split('"').next().unwrap();
                    let form = format!("state={state}&name={}&show_discord=1&agree=1", name.replace(' ', "+"));
                    let (status, location, _) =
                        browse(&format!("{}/register", start.split("/auth").next().unwrap()), Some(&form));
                    assert_eq!(status, 302);
                    location.unwrap()
                } else {
                    location.unwrap()
                };
                // the launcher's own port
                assert!(back.starts_with("http://127.0.0.1:"));
                let (status, _, body) = browse(&back, None);
                assert_eq!(status, 200);
                assert!(body.contains("signed in"));
            });
            Ok(())
        };
        sign_in(base, &AtomicBool::new(false), None, browser).unwrap()
    }

    /// Against the store on this computer with the stand-in Discord (`store\dev.cmd --fake-discord`): the whole
    /// sign-in (a first one, so the registration form), then the account's calls.
    /// `QWC_STORE_URL=http://127.0.0.1:8787 cargo test -p qwc-launcher stand_in -- --ignored`
    #[test]
    #[ignore = "needs store\\dev.cmd --fake-discord running"]
    fn signing_in_with_the_stand_in_discord() {
        let base = std::env::var("QWC_STORE_URL").expect("QWC_STORE_URL");
        let (token, me) = stand_in_sign_in(&base, &format!("Tester {}", std::process::id() % 100_000));
        assert_eq!(me.discord_username.as_deref(), Some("seeker_sam"));
        assert_eq!(super::me(&base, &token).unwrap().id, me.id);
        // the profile shows the Discord username when chosen (at registration, or here on a later run), and the
        // picture comes through the store
        edit(&base, &token, None, Some(true)).unwrap();
        let p = profile(&base, &me.id).unwrap();
        assert_eq!(p.discord_username.as_deref(), Some("seeker_sam"));
        assert!(decode_image(&picture(&base, &me.id).unwrap()).is_ok());
        // hide it again
        assert!(!edit(&base, &token, None, Some(false)).unwrap().show_discord);
        assert_eq!(profile(&base, &me.id).unwrap().discord_username, None);
        // a picture uploaded, then back to Discord's
        let sq = [200u8, 40, 40, 255].repeat((PICTURE_SIDE * PICTURE_SIDE) as usize);
        let (bytes, mime) = encode_picture(&sq).unwrap();
        upload_picture(&base, &token, &bytes, mime)
            .unwrap_or_else(|e| panic!("upload ({mime}, {} bytes): {e}", bytes.len()));
        assert!(super::me(&base, &token).unwrap().uploaded_avatar);
        let (rgba, _, _) = decode_image(&picture(&base, &me.id).unwrap()).unwrap();
        assert!(rgba[0] > 150 && rgba[1] < 90);
        reset_picture(&base, &token).unwrap();
        assert!(!super::me(&base, &token).unwrap().uploaded_avatar);
        // logged out: the token is no good
        log_out(&base, &token).unwrap();
        assert_eq!(super::me(&base, &token), Err(Fail::SignedOut));
    }

    /// Submitting, reviewing, rating and reporting against the store on this computer (`store\dev.cmd
    /// --fake-discord`: the first player to sign in is the admin; its waiting samples are built and served).
    /// `QWC_STORE_URL=http://127.0.0.1:8787 cargo test -p qwc-launcher reviewing_against -- --ignored`
    #[test]
    #[ignore = "needs store\\dev.cmd --fake-discord running"]
    fn reviewing_against_the_local_store() {
        use crate::store;
        let base = std::env::var("QWC_STORE_URL").expect("QWC_STORE_URL");
        let (token, me) = stand_in_sign_in(&base, &format!("Reviewer {}", std::process::id() % 100_000));
        assert_eq!(me.role, "admin", "the local store makes the first player to sign in the admin");

        // a pack of our own, with its statement, built and served where dev.mjs serves files
        let served = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../store/.dev-files/files");
        let version = format!("1.0.{}", std::process::id() % 100_000);
        let src = std::env::temp_dir().join(format!("qwc-submit-src-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&src);
        let mut m = qwc_packs::Manifest::new("sam.lanterns", "Lanterns");
        m.version = version.clone();
        m.declaration = Some(qwc_packs::Declaration {
            own_work: true,
            recipes: false,
            licence: "CC0 1.0".into(),
            credits: "Sam".into(),
        });
        qwc_packs::Pack::create(&src, m).unwrap();
        let file = served.join(format!("sam.lanterns-{version}.qwcpack"));
        qwc_packs::build(&src, &file).unwrap();
        let _ = std::fs::remove_dir_all(&src);
        let pack = read_pack_file(&file).unwrap();
        let url = format!("http://127.0.0.1:8790/sam.lanterns-{version}.qwcpack");
        // the dialog's check: the link gives this very file
        let check = std::env::temp_dir().join(format!("qwc-submit-check-{}.qwcpack", std::process::id()));
        crate::updates::fetch_checked(
            &url,
            &check,
            Some(pack.size),
            Some(&pack.sha256),
            &crate::setup::Progress::default(),
        )
        .unwrap();
        let _ = std::fs::remove_file(&check);
        let mine = submit(&base, &token, &pack, &url, "Paper lanterns over the stands.", &[]).unwrap();
        assert!(my_submissions(&base, &token).unwrap().iter().any(|s| s.listing == mine && s.status == "pending"));
        // a link the store can't use, and the statement missing
        let mut bare = pack.clone();
        bare.manifest.declaration = None;
        assert!(submit(&base, &token, &bare, &url, "", &[]).unwrap_err().to_string().contains("statement"));
        assert!(submit(&base, &token, &pack, "https://example.com/x.qwcpack", "", &[])
            .unwrap_err()
            .to_string()
            .contains("GitHub"));

        // the queue: our own can't be decided by us; another author's checked and approved
        let queue = review_queue(&base, &token).unwrap();
        assert!(decide(&base, &token, mine, true, "").unwrap_err().to_string().contains("own"));
        let sky = queue.iter().find(|p| p.id == "bo.starry-sky").expect("the waiting sample");
        let downloads = std::env::temp_dir().join(format!("qwc-review-{}", std::process::id()));
        let checked = store::check_submission(&sky.target(), &downloads, &crate::setup::Progress::default()).unwrap();
        assert_eq!(checked.manifest.id, "bo.starry-sky");
        let _ = std::fs::remove_dir_all(&downloads);
        decide(&base, &token, sky.listing, true, "").unwrap();
        assert!(store::pack(&base, "bo.starry-sky").unwrap().is_some());

        // rated once installed
        assert!(rate(&base, &token, sky.listing, 4).unwrap_err().to_string().contains("Install"));
        record_install(&base, &token, sky.listing).unwrap();
        rate(&base, &token, sky.listing, 4).unwrap();
        assert!(my_ratings(&base, &token).unwrap().iter().any(|r| r.pack == "bo.starry-sky" && r.stars == 4));

        // reported, then dismissed
        report(&base, &token, "listing", &sky.listing.to_string(), "broken", "The stars flicker.").unwrap();
        let open = open_reports(&base, &token).unwrap();
        let r = open.iter().find(|r| r.target_id == sky.listing.to_string()).expect("the report");
        resolve_report(&base, &token, r.id, "dismiss").unwrap();
        assert!(!open_reports(&base, &token).unwrap().iter().any(|x| x.id == r.id));

        // a reviewer made
        set_role(&base, &token, "dev-bo", true).unwrap();
        assert_eq!(profile(&base, "dev-bo").unwrap().role, "reviewer");
        let _ = std::fs::remove_file(&file);
    }

    /// Collections against the store on this computer (`store\dev.cmd --fake-discord`, its sample packs): one made
    /// of two sample packs, found, installed for real, revised and removed.
    /// `QWC_STORE_URL=http://127.0.0.1:8787 cargo test -p qwc-launcher collections_against -- --ignored`
    #[test]
    #[ignore = "needs store\\dev.cmd --fake-discord running"]
    fn collections_against_the_local_store() {
        use crate::store;
        let base = std::env::var("QWC_STORE_URL").expect("QWC_STORE_URL");
        let (token, me) = stand_in_sign_in(&base, &format!("Collector {}", std::process::id() % 100_000));
        let listing = |id: &str| store::pack(&base, id).unwrap().unwrap().versions[0].listing;
        let (hud, lights) = (listing("qwcr.clean-hud"), listing("ana.floodlights"));
        // a refusal lists its problems: the same pack twice
        let twice = publish_collection(&base, &token, "Twice over", "", &[(hud, true), (hud, true)]).unwrap_err();
        assert!(twice.to_string().contains("more than once"), "{twice}");
        let name = format!("Night set {}", std::process::id());
        let id = publish_collection(&base, &token, &name, "Dark and quiet", &[(hud, true), (lights, false)]).unwrap();
        // found in the store and on the profile
        let found = store::browse_collections(&base, &name, false, 1).unwrap();
        assert_eq!(found.collections.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), [id.as_str()]);
        assert!(profile(&base, &me.id).unwrap().collections.iter().any(|c| c.id == id));
        let c = store::collection(&base, &id).unwrap();
        assert_eq!(
            c.items.iter().map(|i| (i.id.as_str(), i.enabled)).collect::<Vec<_>>(),
            [("qwcr.clean-hud", true), ("ana.floodlights", false)]
        );
        // installed for real, each pack checked
        let root = std::env::temp_dir().join(format!("qwc-launcher-collection-{}", std::process::id()));
        let done = store::install_collection(
            &c,
            &Default::default(),
            &root.join("downloads"),
            &root.join("packs"),
            &crate::setup::Progress::default(),
        )
        .unwrap();
        assert_eq!(done.installed.len(), 2);
        assert!(root.join("packs").join("ana.floodlights").join("pack.ron").is_file());
        let _ = std::fs::remove_dir_all(&root);
        // revised: a new revision, the order turned round
        revise_collection(&base, &token, &id, &[(lights, true), (hud, true)]).unwrap();
        let c = store::collection(&base, &id).unwrap();
        assert_eq!((c.summary.revision, c.items[0].id.as_str()), (2, "ana.floodlights"));
        // removed
        delete_collection(&base, &token, &id).unwrap();
        assert!(store::collection(&base, &id).is_err());
    }

    #[test]
    fn the_token_kept_and_read_back() {
        let dir = std::env::temp_dir().join(format!("qwc-launcher-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(stored_token(&dir), None);
        store_token(&dir, "secret-token").unwrap();
        assert_eq!(stored_token(&dir).as_deref(), Some("secret-token"));
        // on Windows in Credential Manager, not in a file
        #[cfg(windows)]
        assert!(!token_path(&dir).exists());
        forget_token(&dir);
        assert_eq!(stored_token(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
