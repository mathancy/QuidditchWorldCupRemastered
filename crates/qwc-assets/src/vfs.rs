//! A read-only virtual file system over the game data directory (the retail disc contents).
//!
//! Every `.ccd` archive is mounted as a directory named after the archive without its extension, so
//! `root/Balls.ccd` member `snitch.fpm` is `root/Balls/snitch.fpm`. This is the same layout
//! `tools/ccd_extract.py` writes to `extracted/`, which the docs and tools use for paths. Loose files keep their own
//! paths. Lookups ignore case and accept `/` or `\` separators, like the game's Windows file system.
//!
//! Only archive directories are read when the file system is opened; an archive is decompressed the first time one
//! of its members is read and then kept in memory.
//!
//! Overlays ([`Vfs::add_overlay`]) are directories laid over the data with the same layout: a file there replaces the
//! disc's file at the same path (or adds a new one), and a `.ccd` there is mounted like the disc's archives. This is
//! how remastered assets are dropped in without repacking archives.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::ops::{Deref, Range};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use qwc_formats::ccd::Directory;

#[derive(Debug)]
pub enum Error {
    Io(PathBuf, std::io::Error),
    Format(PathBuf, qwc_formats::Error),
    NotFound(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Error::Format(p, e) => write!(f, "{}: {e}", p.display()),
            Error::NotFound(p) => write!(f, "{p}: not found in the game data"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Bytes of one file: a shared buffer (a decompressed archive or a loose file) and the file's range in it.
#[derive(Clone)]
pub struct Bytes {
    buf: Arc<Vec<u8>>,
    range: Range<usize>,
}

impl Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.buf[self.range.clone()]
    }
}

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes({} bytes)", self.range.len())
    }
}

#[derive(Debug, Clone)]
enum Source {
    Loose(PathBuf),
    Member { archive: usize, range: Range<usize> },
}

#[derive(Debug)]
struct Archive {
    path: PathBuf,
    dir: Directory,
}

/// The mounted game data.
#[derive(Debug)]
pub struct Vfs {
    root: PathBuf,
    /// Normalised path -> (path as spelled on disc or in the archive, source).
    files: BTreeMap<String, (String, Source)>,
    archives: Vec<Archive>,
    cache: Mutex<HashMap<usize, Arc<Vec<u8>>>>,
}

fn same(a: &(String, Source), b: &(String, Source)) -> bool {
    match (&a.1, &b.1) {
        (Source::Loose(x), Source::Loose(y)) => x == y,
        (Source::Member { archive: x, range: r }, Source::Member { archive: y, range: q }) => x == y && r == q,
        _ => false,
    }
}

/// Lower-case, `/`-separated, no leading `./` or `/`.
pub fn normalize(path: &str) -> String {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    let p = p.trim_start_matches("./").trim_start_matches('/');
    let mut out = String::with_capacity(p.len());
    for part in p.split('/').filter(|s| !s.is_empty() && *s != ".") {
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part);
    }
    out
}

impl Vfs {
    /// Index the data directory `root` (for example `original/cd_files`).
    pub fn open(root: impl AsRef<Path>) -> Result<Vfs> {
        let root = root.as_ref().to_path_buf();
        let mut vfs = Vfs { root: root.clone(), files: BTreeMap::new(), archives: Vec::new(), cache: Mutex::default() };
        vfs.index(&root)?;
        Ok(vfs)
    }

    /// Lay the directory `dir` over the data: each of its files replaces the file at the same path (case and
    /// separators ignored) or adds a new one; each `.ccd` is mounted and its members replace the same way. Later
    /// overlays win. Returns the number of paths the overlay provides.
    pub fn add_overlay(&mut self, dir: impl AsRef<Path>) -> Result<usize> {
        let before = self.files.clone();
        let dir = dir.as_ref().to_path_buf();
        self.index(&dir)?;
        Ok(self.files.iter().filter(|(k, v)| before.get(*k).is_none_or(|b| !same(b, v))).count())
    }

    /// Whether `path` currently comes from somewhere other than `root` (an overlay).
    pub fn is_overridden(&self, path: &str) -> bool {
        match self.files.get(&normalize(path)) {
            Some((_, Source::Loose(p))) => !p.starts_with(&self.root),
            Some((_, Source::Member { archive, .. })) => !self.archives[*archive].path.starts_with(&self.root),
            None => false,
        }
    }

    fn index(&mut self, root: &Path) -> Result<()> {
        let root = root.to_path_buf();
        let mut stack = vec![root.clone()];
        let mut found = Vec::new();
        while let Some(dir) = stack.pop() {
            let entries = fs::read_dir(&dir).map_err(|e| Error::Io(dir.clone(), e))?;
            for entry in entries {
                let entry = entry.map_err(|e| Error::Io(dir.clone(), e))?;
                let path = entry.path();
                let ft = entry.file_type().map_err(|e| Error::Io(path.clone(), e))?;
                if ft.is_dir() {
                    stack.push(path);
                } else if ft.is_file() {
                    found.push(path);
                }
            }
        }
        found.sort();
        let vfs = self;
        for path in found {
            let rel = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
            if rel.to_ascii_lowercase().ends_with(".ccd") {
                vfs.mount(path, &rel[..rel.len() - 4])?;
            } else {
                vfs.files.insert(normalize(&rel), (rel, Source::Loose(path)));
            }
        }
        Ok(())
    }

    fn mount(&mut self, path: PathBuf, mount: &str) -> Result<()> {
        let data = fs::read(&path).map_err(|e| Error::Io(path.clone(), e))?;
        let dir = Directory::parse(&data).map_err(|e| Error::Format(path.clone(), e))?;
        let archive = self.archives.len();
        for m in &dir.members {
            let spelled = format!("{mount}/{}", m.name.replace('\\', "/"));
            let source = Source::Member { archive, range: m.offset..m.offset + m.size };
            self.files.insert(normalize(&spelled), (spelled, source));
        }
        self.archives.push(Archive { path, dir });
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn archive_count(&self) -> usize {
        self.archives.len()
    }

    /// Number of files, archive members and loose files together.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn exists(&self, path: &str) -> bool {
        self.files.contains_key(&normalize(path))
    }

    /// Read a file's bytes.
    pub fn read(&self, path: &str) -> Result<Bytes> {
        let (_, source) = self.files.get(&normalize(path)).ok_or_else(|| Error::NotFound(path.to_string()))?;
        match source {
            Source::Loose(p) => {
                let buf = fs::read(p).map_err(|e| Error::Io(p.clone(), e))?;
                let range = 0..buf.len();
                Ok(Bytes { buf: Arc::new(buf), range })
            }
            Source::Member { archive, range } => Ok(Bytes { buf: self.blob(*archive)?, range: range.clone() }),
        }
    }

    /// Read `len` bytes at `offset` in a file. A loose file reads only that range from disc, which suits the large
    /// audio containers (`banks.dat`, `dat.dat`) that are read a piece at a time.
    pub fn read_range(&self, path: &str, offset: usize, len: usize) -> Result<Bytes> {
        let (_, source) = self.files.get(&normalize(path)).ok_or_else(|| Error::NotFound(path.to_string()))?;
        let short = |p: &Path| {
            let e = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, format!("no {len} bytes at {offset:#x}"));
            Error::Io(p.to_path_buf(), e)
        };
        match source {
            Source::Loose(p) => {
                use std::io::{Read, Seek, SeekFrom};
                let mut f = fs::File::open(p).map_err(|e| Error::Io(p.clone(), e))?;
                f.seek(SeekFrom::Start(offset as u64)).map_err(|e| Error::Io(p.clone(), e))?;
                let mut buf = vec![0; len];
                f.read_exact(&mut buf).map_err(|_| short(p))?;
                Ok(Bytes { buf: Arc::new(buf), range: 0..len })
            }
            Source::Member { archive, range } => {
                if offset.checked_add(len).is_none_or(|end| end > range.len()) {
                    return Err(short(&self.archives[*archive].path));
                }
                let start = range.start + offset;
                Ok(Bytes { buf: self.blob(*archive)?, range: start..start + len })
            }
        }
    }

    fn blob(&self, archive: usize) -> Result<Arc<Vec<u8>>> {
        if let Some(b) = self.cache.lock().unwrap().get(&archive) {
            return Ok(b.clone());
        }
        let a = &self.archives[archive];
        let data = fs::read(&a.path).map_err(|e| Error::Io(a.path.clone(), e))?;
        let blob = Arc::new(a.dir.unpack(&data).map_err(|e| Error::Format(a.path.clone(), e))?);
        // Two threads may unpack the same archive at once; both results are identical, keep the first.
        Ok(self.cache.lock().unwrap().entry(archive).or_insert(blob).clone())
    }

    /// Drop every decompressed archive from memory.
    pub fn clear_cache(&self) {
        self.cache.lock().unwrap().clear();
    }

    /// All file paths (as spelled in the data), in sorted order of their normalised form.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.values().map(|(s, _)| s.as_str())
    }

    /// Paths whose normalised form ends with `.ext` (`ext` without the dot, any case).
    pub fn with_extension<'a>(&'a self, ext: &str) -> impl Iterator<Item = &'a str> + 'a {
        let suffix = format!(".{}", ext.to_ascii_lowercase());
        self.files.iter().filter(move |(k, _)| k.ends_with(&suffix)).map(|(_, (s, _))| s.as_str())
    }

    /// Paths of the files directly inside directory `dir`.
    pub fn list_dir<'a>(&'a self, dir: &str) -> impl Iterator<Item = &'a str> + 'a {
        let mut prefix = normalize(dir);
        if !prefix.is_empty() {
            prefix.push('/');
        }
        let n = prefix.len();
        self.files
            .range(prefix.clone()..)
            .take_while(move |(k, _)| k.starts_with(&prefix))
            .filter(move |(k, _)| !k[n..].contains('/'))
            .map(|(_, (s, _))| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_ccd(names: &[&str], contents: &[u8], sizes: &[usize]) -> Vec<u8> {
        // RefPack: 0xE0 + 4 literals ... encode everything as literal runs, then 0xFC + up to 3 literals.
        let mut packed = vec![0x10, 0xFB, 0, 0, contents.len() as u8];
        let mut rest = contents;
        while rest.len() >= 4 {
            let n = (rest.len() / 4 * 4).min(112);
            packed.push(0xE0 + ((n - 4) / 4) as u8);
            packed.extend_from_slice(&rest[..n]);
            rest = &rest[n..];
        }
        packed.push(0xFC + rest.len() as u8);
        packed.extend_from_slice(rest);
        let mut name_bytes = Vec::new();
        let names_off = 0x38 + names.len() * 16;
        let mut table = Vec::new();
        let mut off = 0;
        for (n, s) in names.iter().zip(sizes) {
            for v in [(names_off + name_bytes.len()) as u32, off as u32, *s as u32, *s as u32] {
                table.extend_from_slice(&v.to_le_bytes());
            }
            name_bytes.extend_from_slice(n.as_bytes());
            name_bytes.push(0);
            off += s;
        }
        let hdr = names_off + name_bytes.len();
        let mut d = b"FKNL".to_vec();
        for v in [2u32, hdr as u32, contents.len() as u32, packed.len() as u32, names.len() as u32] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.resize(0x38, 0);
        d.extend_from_slice(&table);
        d.extend_from_slice(&name_bytes);
        d.extend_from_slice(&packed);
        d
    }

    #[test]
    fn normalizes() {
        assert_eq!(normalize("Root\\Balls/./Snitch.FPM"), "root/balls/snitch.fpm");
        assert_eq!(normalize("/a//b/"), "a/b");
    }

    #[test]
    fn mounts_archives_and_loose_files() {
        let dir = std::env::temp_dir().join(format!("qwc-vfs-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("root/sub")).unwrap();
        fs::write(dir.join("root/Pack.ccd"), fake_ccd(&["a.txt", "Dir\\B.txt"], b"hello world", &[5, 6])).unwrap();
        fs::write(dir.join("root/sub/loose.bin"), b"xyz").unwrap();
        let vfs = Vfs::open(&dir).unwrap();
        assert_eq!(vfs.archive_count(), 1);
        assert_eq!(vfs.len(), 3);
        assert_eq!(&*vfs.read("ROOT/pack/A.TXT").unwrap(), b"hello");
        assert_eq!(&*vfs.read("root\\pack\\dir\\b.txt").unwrap(), b" world");
        assert_eq!(&*vfs.read("root/sub/loose.bin").unwrap(), b"xyz");
        assert!(matches!(vfs.read("root/pack/c.txt"), Err(Error::NotFound(_))));
        assert_eq!(vfs.list_dir("root/pack").collect::<Vec<_>>(), ["root/Pack/a.txt"]);
        assert_eq!(vfs.with_extension("TXT").count(), 2);
        assert_eq!(&*vfs.read_range("root/pack/dir/b.txt", 1, 3).unwrap(), b"wor");
        assert_eq!(&*vfs.read_range("root/sub/loose.bin", 1, 2).unwrap(), b"yz");
        assert!(vfs.read_range("root/pack/a.txt", 3, 3).is_err());
        assert!(vfs.read_range("root/sub/loose.bin", 2, 2).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn overlays_replace_and_add() {
        let base = std::env::temp_dir().join(format!("qwc-vfs-overlay-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (data, mods) = (base.join("data"), base.join("mods"));
        fs::create_dir_all(data.join("root")).unwrap();
        fs::create_dir_all(mods.join("ROOT/PACK")).unwrap();
        fs::write(data.join("root/Pack.ccd"), fake_ccd(&["a.txt", "b.txt"], b"hello world", &[5, 6])).unwrap();
        fs::write(data.join("root/loose.bin"), b"xyz").unwrap();
        // a loose replacement for an archive member, spelled in another case, and a new file
        fs::write(mods.join("ROOT/PACK/A.TXT"), b"HELLO!").unwrap();
        fs::write(mods.join("ROOT/new.png"), b"png").unwrap();
        let mut vfs = Vfs::open(&data).unwrap();
        assert!(!vfs.is_overridden("root/pack/a.txt"));
        assert_eq!(vfs.add_overlay(&mods).unwrap(), 2);
        assert_eq!(&*vfs.read("root/pack/a.txt").unwrap(), b"HELLO!");
        assert_eq!(&*vfs.read("root/pack/b.txt").unwrap(), b" world");
        assert_eq!(&*vfs.read("root/new.png").unwrap(), b"png");
        assert!(vfs.is_overridden("root/pack/a.txt") && !vfs.is_overridden("root/pack/b.txt"));
        assert_eq!(vfs.len(), 4);
        fs::remove_dir_all(&base).unwrap();
    }
}
