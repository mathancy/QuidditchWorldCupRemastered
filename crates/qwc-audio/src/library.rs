//! The game's sound data, read through the virtual file system and decoded on first use.
//!
//! - **Banks.** `Audio/cats/banks.fat` indexes `Audio/banks.dat`. Each file there is a memory bank (`BNKl`, decoded
//!   whole) or a streaming bank (a list of SCHl streams, decoded one sound at a time).
//! - **Effect categories.** Every `.map` (`Audio/cats/chaser.map`, `Audio/Stadium/<name>/crowd.map`, ...) maps the
//!   event numbers the game plays to weighted bank sounds.
//! - **Speech.** `Audio/<language>/fat/dat.fat` indexes `Audio/<language>/dat.dat`; one file holds the phrases of
//!   one speech topic as SCHl streams, followed by sentence data that is not decoded yet.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use qwc_assets::Vfs;
use qwc_formats::audio::{stream_run, walk_stream, Fat};
use qwc_formats::ea_audio;
use qwc_formats::sound_map::{bank_file_hash, SoundMap};

use crate::{Error, Result, Sound};

pub const BANKS_FAT: &str = "Audio/cats/banks.fat";
pub const BANKS_DAT: &str = "Audio/banks.dat";

/// The speech languages on the disc, as directory names under `Audio/`.
pub const LANGUAGES: [&str; 3] = ["English", "French", "Spanish"];

fn format(what: impl Into<String>) -> impl FnOnce(qwc_formats::Error) -> Error {
    let what = what.into();
    move |e| Error::Format(what, e)
}

/// One file of `banks.dat` once read.
enum Bank {
    /// Every slot of a memory bank (empty slots are `None`).
    Memory(Vec<Option<Sound>>),
    /// A streaming bank's bytes and the offset of each stream in them; sounds decode one by one.
    Streaming { data: qwc_assets::Bytes, streams: Vec<usize> },
}

/// Sound data access with caches. Shared by reference between threads; every cache has its own lock.
pub struct Library {
    banks: Fat,
    bank_cache: Mutex<HashMap<u32, Arc<Bank>>>,
    streamed: Mutex<HashMap<(u32, u32), Sound>>,
    categories: Mutex<HashMap<String, Arc<SoundMap>>>,
    speech_fats: Mutex<HashMap<String, Arc<Fat>>>,
}

impl Library {
    /// Read the bank table. Nothing else is read until it is asked for.
    pub fn open(vfs: &Vfs) -> Result<Library> {
        let banks = Fat::parse(&vfs.read(BANKS_FAT)?).map_err(format(BANKS_FAT))?;
        Ok(Library {
            banks,
            bank_cache: Mutex::default(),
            streamed: Mutex::default(),
            categories: Mutex::default(),
            speech_fats: Mutex::default(),
        })
    }

    fn bank(&self, vfs: &Vfs, hash: u32) -> Result<Arc<Bank>> {
        if let Some(b) = self.bank_cache.lock().unwrap().get(&hash) {
            return Ok(b.clone());
        }
        let f = self
            .banks
            .files
            .iter()
            .find(|f| f.hash == hash)
            .ok_or_else(|| Error::Missing(format!("bank {hash:#010x} in {BANKS_FAT}")))?;
        let what = format!("{BANKS_DAT} bank at {:#x}", f.offset);
        let data = vfs.read_range(BANKS_DAT, f.offset as usize, f.size as usize)?;
        let bank = if data.starts_with(b"BNKl") {
            let sounds = ea_audio::decode_bnk(&data).map_err(format(&what))?;
            Bank::Memory(sounds.into_iter().map(|s| s.map(Arc::new)).collect())
        } else {
            // u32 n | n x u32 stream offset (layout checked by `qwc_formats::audio::bank_index`)
            let n = data.get(..4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
            let streams = (0..n)
                .map(|i| {
                    let b = data.get(4 + 4 * i..8 + 4 * i).ok_or_else(|| Error::Missing(format!("{what}: offsets")))?;
                    Ok(u32::from_le_bytes(b.try_into().unwrap()) as usize)
                })
                .collect::<Result<Vec<_>>>()?;
            Bank::Streaming { data, streams }
        };
        Ok(self.bank_cache.lock().unwrap().entry(hash).or_insert(Arc::new(bank)).clone())
    }

    /// Sound `index` (1-based, as `.map` files count) of the bank named `bank` (`Global\Foley` or `Foley`).
    /// `None` for an empty memory-bank slot.
    pub fn bank_sound(&self, vfs: &Vfs, bank: &str, index: u32) -> Result<Option<Sound>> {
        let hash = bank_file_hash(bank);
        let missing = || Error::Missing(format!("sound {index} of bank {bank}"));
        match &*self.bank(vfs, hash)? {
            Bank::Memory(sounds) => {
                Ok(sounds.get((index as usize).checked_sub(1).ok_or_else(missing)?).ok_or_else(missing)?.clone())
            }
            Bank::Streaming { data, streams } => {
                if let Some(s) = self.streamed.lock().unwrap().get(&(hash, index)) {
                    return Ok(Some(s.clone()));
                }
                let off = *streams.get((index as usize).checked_sub(1).ok_or_else(missing)?).ok_or_else(missing)?;
                let pcm = ea_audio::decode_schl(data.get(off..).ok_or_else(missing)?)
                    .map_err(format(format!("sound {index} of bank {bank}")))?;
                let s = Arc::new(pcm);
                Ok(Some(self.streamed.lock().unwrap().entry((hash, index)).or_insert(s).clone()))
            }
        }
    }

    /// Number of sounds (memory-bank slots or streams) in the bank named `bank`.
    pub fn bank_len(&self, vfs: &Vfs, bank: &str) -> Result<usize> {
        Ok(match &*self.bank(vfs, bank_file_hash(bank))? {
            Bank::Memory(s) => s.len(),
            Bank::Streaming { streams, .. } => streams.len(),
        })
    }

    /// The effect category at `path` (a `.map` file).
    pub fn category(&self, vfs: &Vfs, path: &str) -> Result<Arc<SoundMap>> {
        let key = qwc_assets::vfs::normalize(path);
        if let Some(m) = self.categories.lock().unwrap().get(&key) {
            return Ok(m.clone());
        }
        let map = Arc::new(SoundMap::parse(&vfs.read(path)?).map_err(format(path))?);
        Ok(self.categories.lock().unwrap().entry(key).or_insert(map).clone())
    }

    /// The sound that event `event_id` of the category at `path` plays. `roll` in [0, 1) picks one of the event's
    /// weighted samples (the weights are cumulative out of 65535).
    ///
    /// Inferred, not traced: the first variant that has samples is used. All but two of the 946 events on the disc
    /// have at most one variant; the other two (`chal_01` ambience and timer) link variants into sequences whose
    /// rules are in the unread playback code (`TbSoundManager.cpp`).
    pub fn event(&self, vfs: &Vfs, path: &str, event_id: u32, roll: f32) -> Result<Option<Sound>> {
        let map = self.category(vfs, path)?;
        let Some(event) = map.events().find(|e| e.event_id == event_id) else {
            return Err(Error::Missing(format!("event {event_id} in {path}")));
        };
        let Some(variant) = event.variants.iter().find(|v| !v.samples.is_empty()) else {
            return Ok(None);
        };
        let r = (roll.clamp(0.0, 1.0) * 65535.0) as u32;
        let sample = variant.samples.iter().find(|s| r < s.weight).unwrap_or(variant.samples.last().unwrap());
        let bank = map
            .banks
            .get((sample.bank as usize).wrapping_sub(1))
            .ok_or_else(|| Error::Missing(format!("bank {} of {path}", sample.bank)))?;
        self.bank_sound(vfs, &bank.name, sample.sound)
    }

    fn speech_fat(&self, vfs: &Vfs, lang: &str) -> Result<Arc<Fat>> {
        if let Some(f) = self.speech_fats.lock().unwrap().get(lang) {
            return Ok(f.clone());
        }
        let path = format!("Audio/{lang}/fat/dat.fat");
        let fat = Arc::new(Fat::parse(&vfs.read(&path)?).map_err(format(&path))?);
        Ok(self.speech_fats.lock().unwrap().entry(lang.to_string()).or_insert(fat).clone())
    }

    /// Speech file `name`'s bytes and where its trailer starts: streams on 0x100 boundaries, then `"HrIn"`, the
    /// sentence data (its `.hdr`'s bank, repeated), `"HrSz"` and the data's length (u32, big-endian).
    fn speech_file(&self, vfs: &Vfs, lang: &str, name: &str) -> Result<(qwc_assets::Bytes, usize, String)> {
        let fat = self.speech_fat(vfs, lang)?;
        let f = fat.lookup(name).ok_or_else(|| Error::Missing(format!("{name} in Audio/{lang}/fat/dat.fat")))?;
        let dat = format!("Audio/{lang}/dat.dat");
        let what = format!("{dat}: {name}");
        let d = vfs.read_range(&dat, f.offset as usize, f.size as usize)?;
        let end = d.len();
        let trailer = d
            .get(end.saturating_sub(4)..)
            .filter(|_| end >= 12 && &d[end - 8..end - 4] == b"HrSz")
            .map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize)
            .and_then(|n| end.checked_sub(12 + n))
            .filter(|&t| &d[t..t + 4] == b"HrIn")
            .ok_or_else(|| Error::Missing(format!("{what}: HrIn/HrSz trailer")))?;
        Ok((d, trailer, what))
    }

    /// The sentence data speech file `name` ends with: the clip bank of the `.hdr` that belongs to it.
    pub fn speech_sentences(&self, vfs: &Vfs, lang: &str, name: &str) -> Result<Vec<u8>> {
        let (d, trailer, _) = self.speech_file(vfs, lang, name)?;
        Ok(d[trailer + 4..d.len() - 8].to_vec())
    }

    /// The phrases of speech file `name` (`AV\CCWaxl.dat`, the path its `.hdr` names after `Audio\<lang>\dat\`), in
    /// stored order. Not cached: speech is played once and dropped.
    pub fn speech(&self, vfs: &Vfs, lang: &str, name: &str) -> Result<Vec<Sound>> {
        let (d, trailer, what) = self.speech_file(vfs, lang, name)?;
        stream_run(&d, 0, trailer, 0x100)
            .map_err(format(&what))?
            .iter()
            .map(|s| Ok(Arc::new(ea_audio::decode_schl(&d[s.offset..]).map_err(format(&what))?)))
            .collect()
    }

    /// The one phrase of speech file `name` that starts at `offset` and ends by `end` (0x100-byte units from the
    /// file's start, as a clip bank's `.hdr` gives them: the next clip's offset, or the bank's size). Not cached.
    pub fn speech_clip(&self, vfs: &Vfs, lang: &str, name: &str, offset: u16, end: u16) -> Result<Sound> {
        let fat = self.speech_fat(vfs, lang)?;
        let f = fat.lookup(name).ok_or_else(|| Error::Missing(format!("{name} in Audio/{lang}/fat/dat.fat")))?;
        let (start, end) = (offset as usize * 0x100, (end as usize * 0x100).min(f.size as usize));
        let what = format!("Audio/{lang}/dat.dat: {name} at {start:#x}");
        let len = end.checked_sub(start).filter(|&n| n > 0).ok_or_else(|| Error::Missing(what.clone()))?;
        let d = vfs.read_range(&format!("Audio/{lang}/dat.dat"), f.offset as usize + start, len)?;
        stream_at(&d, 0, &what)
    }
}

/// Decode the SCHl stream at `offset` in `data`, checking that its blocks stay inside `data`.
pub(crate) fn stream_at(data: &[u8], offset: usize, what: &str) -> Result<Sound> {
    walk_stream(data, offset, None).map_err(format(what))?;
    Ok(Arc::new(ea_audio::decode_schl(&data[offset..]).map_err(format(what))?))
}
