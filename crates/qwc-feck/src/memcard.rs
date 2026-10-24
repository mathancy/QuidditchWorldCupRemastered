//! The memory-card manager (`FECK\MemCard\FECK_MemoryCardManager.cpp`, code `0x4eaea0`-`0x4ed1a0`, the manager
//! object at `0x60e908`), as traced in `docs/FECK_VM.md` ("Memory card"). The PC build keeps the console's
//! card protocol and emulates the card with one file, `<Documents>\EA Games Saves\<game name>\<game name>`.
//!
//! The save is one buffer (`0x60e980 + 0x68`) holding a header and [`SLOTS`] slots. Each slot is an 8-byte
//! summary followed by the DataStore entries named in [`SAVE_KEYS`], packed as `{key, type, count, stride, data}`
//! records. The scripts drive a state machine through `Memcard_ChangeState`; it advances one step per frame, does
//! the file work through a [`CardIo`], asks the player through the registered message callback and answers through
//! the state callback.

use qwc_formats::flow::{name_hash, name_hash_seed};

use crate::frontend::{Entry, FrontEnd, TYPE_SIZE};
use crate::layer;
use crate::memory::Memory;
use crate::vm::Vm;

/// The DataStore keys a save holds, in order (the table `0x5ef6d8` the front-end manager gets from `0x4fe010`;
/// `0x7714aa8d` is listed twice, so it is saved twice).
pub const SAVE_KEYS: [u32; 38] = [
    0x01b23891, 0x1527016b, 0xacfe6233, 0x3db8fbf2, 0xa20daf5d, 0xa4bebcdd, 0xa6a33a51, 0xe2999616, 0x7714aa8d,
    0x136ea64f, 0xc1f55188, 0x642c41e9, 0xc3584e67, 0x538a60bf, 0xacb6e3dd, 0xe687334f, 0xa703801f, 0x55f1a816,
    0x7714aa8d, 0xf82cee20, 0x9dc37226, 0xe88f0701, 0x863648cb, 0x0acab0ef, 0x80425ea5, 0x4d804136, 0xc1a1827b,
    0xb152b5c4, 0xfe53e9a2, 0xfbed26f4, 0x6a0c9d65, 0xb1a0b4d4, 0xd9207782, 0xc46d5abd, 0x238b7bfc, 0x4f05a268,
    0x7d96e04d, 0xe099013b,
];
/// Save slots (the constructor's last argument, `0x4fe03d`).
pub const SLOTS: u32 = 4;
/// The DataStore key holding the current slot.
pub const CURRENT_SLOT: u32 = 0xb2a0cfb4;
/// Seed of the save's CRC (`0x4eb240`, `0x4eb620`).
pub const CRC_SEED: u32 = 0x9ca1f2ca;
/// Header: CRC of the rest, slot count, schema checksum, slot data size, current slot.
pub const HEADER: usize = 0x14;
/// The PC folder under the documents folder (`0x5ab274`).
pub const SAVE_FOLDER: &str = "EA Games Saves";
/// The English game name, also the save file's name (`0x5cf86c`; `0x52dd7a` picks one of 14 by language).
pub const GAME_NAME: &str = "Harry Potter - Quidditch WC";
/// The position names a summary's position bytes index (`0x668bb4`), stored as their name hashes.
pub const POSITIONS: [&str; 8] = ["Zeroth", "First", "Second", "Third", "Forth", "Fifth", "Sixth", "Seventh"];

/// `QuidditchCardsIntroShown`, which the card intro sets (`Challenge_QuidditchCardIntro*.vso`'s construct) and the
/// results' choice reads to send the first card to the intro (`Challenge_Results.vso` `ChoiceChoose`).
pub const CARDS_INTRO_SHOWN: u32 = 0xacb6e3dd;
/// `HogwartsPreviousScreenID`, which the results' choice sets to `ChallengeQuidditchCardIntro` ([`CARD_INTRO`]) as
/// it sends the first card there.
pub const PREVIOUS_SCREEN: u32 = 0xe32ac34c;
pub const CARD_INTRO: u32 = 0x1ec3c028;

/// The DataStore keys the slot summary copies (`0x4eb240`), by summary byte.
mod summary {
    pub const QCARDS: u32 = 0xa20daf5d; // byte 1
    pub const HOUSE: u32 = 0x9dc37226; // byte 2
    pub const HOUSE_POSITION: u32 = 0xe88f0701; // byte 3, stored + 1 and wrapping after 4
    pub const HOUSE_CUP: u32 = 0x136ea64f; // byte 4
    pub const EXHIBITION: u32 = 0x0acab0ef; // byte 5
    pub const EXHIBITION_POSITION: u32 = 0x80425ea5; // byte 6
}

/// What the manager needs from the card (the PC build's Win32 calls).
pub trait CardIo {
    /// Free bytes for the save folder (`GetDiskFreeSpaceExA`); `None` when unknown, which the game treats as full.
    fn free_bytes(&mut self) -> Option<u64>;
    /// Whether the save file opens for reading.
    fn exists(&mut self) -> bool;
    /// Read up to `len` bytes of the save file.
    fn read(&mut self, len: usize) -> Result<Vec<u8>, CardError>;
    /// Replace the save file.
    fn write(&mut self, data: &[u8]) -> Result<(), CardError>;
    /// Delete the save file; `false` when that failed.
    fn delete(&mut self) -> bool;
}

/// How a file operation failed: the open (`CreateFileA`) or the transfer (`ReadFile`/`WriteFile`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardError {
    Open,
    Transfer,
}

/// A card in memory: no file until one is written, and as much space as asked unless `free` says otherwise.
#[derive(Debug, Clone, Default)]
pub struct MemoryCard {
    pub file: Option<Vec<u8>>,
    pub free: Option<u64>,
    /// Make the next writes fail this way (for tests).
    pub fail_write: Option<CardError>,
}

impl CardIo for MemoryCard {
    fn free_bytes(&mut self) -> Option<u64> {
        Some(self.free.unwrap_or(u64::MAX))
    }
    fn exists(&mut self) -> bool {
        self.file.is_some()
    }
    fn read(&mut self, len: usize) -> Result<Vec<u8>, CardError> {
        let f = self.file.as_ref().ok_or(CardError::Open)?;
        Ok(f[..f.len().min(len)].to_vec())
    }
    fn write(&mut self, data: &[u8]) -> Result<(), CardError> {
        if let Some(e) = self.fail_write {
            return Err(e);
        }
        self.file = Some(data.to_vec());
        Ok(())
    }
    fn delete(&mut self) -> bool {
        self.file.take().is_some()
    }
}

/// The save file on disk, in `folder` (made when missing, as `0x4ebe80` does) under the name `name`.
#[derive(Debug, Clone)]
pub struct DirCard {
    pub folder: std::path::PathBuf,
    pub file: std::path::PathBuf,
}

impl DirCard {
    pub fn new(folder: impl Into<std::path::PathBuf>, name: &str) -> DirCard {
        let folder = folder.into();
        let file = folder.join(name);
        DirCard { folder, file }
    }

    /// The original location, `<Documents>\EA Games Saves\<name>\<name>`. **[I]** the documents folder is
    /// `%USERPROFILE%\Documents` on Windows and `$HOME/Documents` elsewhere (the original asks the shell, CSIDL 5).
    pub fn documents(name: &str) -> Option<DirCard> {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
        Some(DirCard::new(std::path::Path::new(&home).join("Documents").join(SAVE_FOLDER).join(name), name))
    }
}

impl CardIo for DirCard {
    fn free_bytes(&mut self) -> Option<u64> {
        // **[I]** the standard library cannot ask for free space; the folder is made here, and any space counts
        // as enough
        std::fs::create_dir_all(&self.folder).ok().map(|_| u64::MAX)
    }
    fn exists(&mut self) -> bool {
        std::fs::File::open(&self.file).is_ok()
    }
    fn read(&mut self, len: usize) -> Result<Vec<u8>, CardError> {
        use std::io::Read;
        let f = std::fs::File::open(&self.file).map_err(|_| CardError::Open)?;
        let mut b = Vec::new();
        f.take(len as u64).read_to_end(&mut b).map_err(|_| CardError::Transfer)?;
        Ok(b)
    }
    fn write(&mut self, data: &[u8]) -> Result<(), CardError> {
        use std::io::Write;
        let _ = std::fs::create_dir_all(&self.folder);
        let mut f = std::fs::File::create(&self.file).map_err(|_| CardError::Open)?;
        f.write_all(data).and_then(|_| f.sync_all()).map_err(|_| CardError::Transfer)
    }
    fn delete(&mut self) -> bool {
        std::fs::remove_file(&self.file).is_ok()
    }
}

/// The manager's state (the object at `0x60e908`).
pub struct MemCard {
    /// `+0x40` and `+0x48`: set by `Memcard_EnableMemoryCards(1)`, cleared by `(0)`; the machine runs while set.
    pub enabled: bool,
    /// `+0x04`: 1 idle; script state `n` is the three states `3n - 1` (enter), `3n` (work), `3n + 1` (report).
    pub state: u32,
    /// `+0x3c`: the step inside a work state.
    pub sub: u32,
    /// `+0x28`: the result the state callback reports.
    pub result: u32,
    /// `+0x30`: frames left on a notice.
    pub timer: i32,
    /// `+0x34`: a message waits for `Memcard_Reply`.
    pub waiting: bool,
    /// `+0x38`: the reply, an index into `answers`.
    pub reply: u32,
    /// `+0xfc`: what each reply means (3 retry or yes, 4 the other button).
    pub answers: [u32; 4],
    /// `+0x68`/`+0x6c` and `+0x70`/`+0x74`: the message and state callbacks, (layer, function hash).
    pub message_cb: Option<(usize, u32)>,
    pub state_cb: Option<(usize, u32)>,
    /// `+0xe0`, `+0x78`: the save buffer, made on the first enable (empty before).
    pub buffer: Vec<u8>,
    /// `+0x2c`: the buffer a load reads into.
    pub work: Option<Vec<u8>>,
    /// `+0xf0`: the slot `Memcard_ActivateSlot` was last given.
    pub active_slot: u32,
    /// `+0xa0`: the save's display name (`Memcard_SetSaveGameName`).
    pub save_name: Vec<u8>,
    /// `Memcard_CheckCard`'s countdown (`[0x104390c]`).
    pub check: u32,
    pub io: Box<dyn CardIo>,
    /// Not the original's: whether a card message is up (the last message callback's kind was not -1, the clear).
    /// The remaster draws the boot's card check only then.
    pub message_up: bool,
    /// The state `QWC_MC_LOG` last printed.
    pub logged: Option<u32>,
}

impl Default for MemCard {
    fn default() -> Self {
        MemCard {
            enabled: false,
            state: 0,
            sub: 0,
            result: 0,
            timer: 0,
            waiting: false,
            reply: 0,
            answers: [0; 4],
            message_cb: None,
            state_cb: None,
            buffer: Vec::new(),
            work: None,
            active_slot: 0,
            save_name: Vec::new(),
            message_up: false,
            logged: None,
            check: 0,
            io: Box::new(MemoryCard::default()),
        }
    }
}

impl std::fmt::Debug for MemCard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemCard")
            .field("enabled", &self.enabled)
            .field("state", &self.state)
            .field("sub", &self.sub)
            .field("result", &self.result)
            .field("waiting", &self.waiting)
            .field("buffer", &self.buffer.len())
            .finish_non_exhaustive()
    }
}

/// `Memcard_ChangeState` (`0x4eb740`): script state `n` to the machine's state.
pub fn machine_state(n: u32) -> Option<u32> {
    match n {
        0 => Some(1),
        1..=15 => Some(3 * n - 1),
        _ => None,
    }
}

/// The script state a machine state belongs to (`0x4eb8d0`).
pub fn script_state(state: u32) -> u32 {
    (state + 1) / 3
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map_or(0, |s| u32::from_le_bytes(s.try_into().unwrap()))
}

fn put_u32(b: &mut [u8], o: usize, v: u32) {
    if let Some(s) = b.get_mut(o..o + 4) {
        s.copy_from_slice(&v.to_le_bytes());
    }
}

fn find(ds: &[Entry], key: u32) -> Option<Entry> {
    ds.iter().find(|e| e.key == key).copied()
}

/// An entry's byte size as the save code computes it, a 16-bit product.
fn entry_len(e: &Entry) -> u32 {
    e.count.wrapping_mul(e.stride) as u32
}

/// A record's size in a slot: 12 bytes of header and the data rounded up to 4 (`0x4eb240`, `0x4eb510`).
fn record_len(e: &Entry) -> u32 {
    (entry_len(e) + 0xf) & !3
}

/// The slot data size (`0x4eb510`): 0 when any key is missing from the DataStore.
pub fn slot_size(ds: &[Entry], keys: &[u32]) -> u32 {
    let mut n = 0u32;
    for &k in keys {
        match find(ds, k).map(|e| record_len(&e)) {
            Some(l) if l != 0 => n += l,
            _ => return 0,
        }
    }
    n
}

/// The schema checksum (`0x4eb5e0`): count, type and key of each listed entry, summed; missing keys add nothing.
pub fn schema_checksum(ds: &[Entry], keys: &[u32]) -> u32 {
    keys.iter()
        .filter_map(|&k| find(ds, k))
        .fold(0u32, |s, e| s.wrapping_add(e.count as u32).wrapping_add(e.kind).wrapping_add(e.key))
}

/// A fresh save buffer (`0x4eb560`): zeros under a header with the slot count, checksum and slot size; CRC 0.
pub fn new_buffer(ds: &[Entry], keys: &[u32], slots: u32) -> Vec<u8> {
    let size = slot_size(ds, keys);
    let mut b = vec![0u8; ((size + 8) * slots + HEADER as u32) as usize];
    put_u32(&mut b, 4, slots);
    put_u32(&mut b, 8, schema_checksum(ds, keys));
    put_u32(&mut b, 0xc, size);
    b
}

/// The CRC a buffer should carry (`0x4eb240`).
pub fn crc(b: &[u8]) -> u32 {
    name_hash_seed(b.get(4..).unwrap_or(&[]), CRC_SEED)
}

/// `0x4eb620`: a loaded save is bad when its CRC fails or its schema checksum differs from ours.
pub fn is_bad(loaded: &[u8], ours: &[u8]) -> bool {
    crc(loaded) != u32_at(loaded, 0) || u32_at(ours, 8) != u32_at(loaded, 8)
}

/// Where slot `slot` starts.
fn slot_offset(b: &[u8], slot: u32) -> usize {
    (u32_at(b, 0xc).wrapping_add(8).wrapping_mul(slot) as usize).wrapping_add(HEADER)
}

/// The DataStore entry's first element, a byte (the summary reads) or a word.
fn first_u8(ds: &[Entry], mem: &Memory, key: u32) -> u8 {
    find(ds, key).and_then(|e| mem.u8(e.addr).ok()).unwrap_or(0)
}

fn first_u32(ds: &[Entry], mem: &Memory, key: u32) -> u32 {
    find(ds, key).and_then(|e| mem.u32(e.addr).ok()).unwrap_or(0)
}

/// Store a word in an entry's first element, as the summary code does whatever the entry's type (a fault on a
/// short entry is ignored).
fn set_first_u32(ds: &[Entry], mem: &mut Memory, key: u32, v: u32) {
    if let Some(e) = find(ds, key) {
        let _ = mem.set_u32(e.addr, v);
    }
}

/// Pack the DataStore into the current slot (`0x4eb240`, `Memcard_PrepareSlot` and the save states): the summary,
/// then a record per listed key that fits, then the header's current slot and CRC (plus one when `spoil`).
///
/// Not the original's: a save on the way to the first card's intro (the results' choice has set
/// [`PREVIOUS_SCREEN`] to [`CARD_INTRO`]) keeps [`CARDS_INTRO_SHOWN`] as 1. The flow saves from the results before
/// the intro sets it, so in the original a game loaded from that save shows the intro again with the next card.
pub fn pack(b: &mut [u8], ds: &[Entry], mem: &Memory, keys: &[u32], spoil: bool) {
    let slot = first_u32(ds, mem, CURRENT_SLOT);
    let at = slot_offset(b, slot);
    let mut remaining = u32_at(b, 0xc);
    // **[I]** a slot past the buffer is skipped (the original writes past it)
    if at.checked_add(8 + remaining as usize).is_none_or(|end| end > b.len()) {
        return;
    }
    let house_position = first_u8(ds, mem, summary::HOUSE_POSITION);
    b[at] = 1;
    b[at + 1] = first_u8(ds, mem, summary::QCARDS);
    b[at + 2] = first_u8(ds, mem, summary::HOUSE);
    b[at + 3] = if house_position == 4 { 0 } else { house_position.wrapping_add(1) };
    b[at + 4] = first_u8(ds, mem, summary::HOUSE_CUP);
    b[at + 5] = first_u8(ds, mem, summary::EXHIBITION);
    b[at + 6] = first_u8(ds, mem, summary::EXHIBITION_POSITION);
    b[at + 7] = 1;
    let mut p = at + 8;
    for &k in keys {
        let Some(e) = find(ds, k) else {
            continue;
        };
        let len = record_len(&e);
        if remaining < len {
            continue;
        }
        put_u32(b, p, e.key);
        put_u32(b, p + 4, e.kind);
        b[p + 8..p + 10].copy_from_slice(&e.count.to_le_bytes());
        b[p + 10..p + 12].copy_from_slice(&e.stride.to_le_bytes());
        let n = entry_len(&e) as usize;
        if let Ok(data) = mem.read(e.addr, n as u32) {
            b[p + 12..p + 12 + n].copy_from_slice(data);
        }
        if e.key == CARDS_INTRO_SHOWN && n != 0 && first_u32(ds, mem, PREVIOUS_SCREEN) == CARD_INTRO {
            b[p + 12] = 1;
        }
        remaining -= len;
        p += len as usize;
    }
    put_u32(b, 0x10, slot);
    let c = crc(b);
    put_u32(b, 0, if spoil { c.wrapping_add(1) } else { c });
}

/// Restore one record at `b[p..]` into the DataStore (`0x4d6cb0`), creating the entry when missing; the entry's own
/// size is copied. Returns the record's size, or 0 when it could not be read.
fn restore(b: &[u8], p: usize, ds: &mut Vec<Entry>, mem: &mut Memory) -> u32 {
    let Some(h) = b.get(p..p + 12) else {
        return 0;
    };
    let (key, kind) = (u32_at(h, 0), u32_at(h, 4));
    let (count, stride) = (u16::from_le_bytes([h[8], h[9]]), u16::from_le_bytes([h[10], h[11]]));
    let e = match find(ds, key) {
        Some(e) => e,
        None => {
            // `0x4d6bc0`, as `DataStore_CreateEntry`
            let stride = if stride == 0 { TYPE_SIZE.get(kind as usize).copied().unwrap_or(0) } else { stride };
            let addr = mem.alloc(count as usize * stride as usize);
            let e = Entry { key, kind, count, stride, addr };
            ds.push(e);
            e
        }
    };
    let n = entry_len(&e) as usize;
    if let Some(data) = b.get(p + 12..p + 12 + n) {
        let _ = mem.write(e.addr, data);
    }
    ((count as u32 * stride as u32 + 3) & !3) + 12
}

/// `Memcard_ActivateSlot` (`0x4eb4a0`): make `slot` current and, when it holds a save, restore its records.
/// 0 when restored, 1 when the slot is empty.
pub fn activate(b: &[u8], ds: &mut Vec<Entry>, mem: &mut Memory, slot: u32) -> u32 {
    set_first_u32(ds, mem, CURRENT_SLOT, slot);
    let at = slot_offset(b, slot);
    if b.get(at).copied().unwrap_or(0) == 0 {
        return 1;
    }
    let mut remaining = u32_at(b, 0xc) as i64;
    let mut p = at + 8;
    while remaining > 0 {
        let n = restore(b, p, ds, mem);
        // **[I]** a record that cannot be read ends the slot (the original would loop for ever)
        if n == 0 {
            break;
        }
        remaining -= n as i64;
        p += n as usize;
    }
    0
}

/// `Memcard_DeleteSlot` (`0x4d3c40`): mark a slot unused, make it current, spoil the CRC (0) and refresh the
/// summaries.
pub fn delete_slot(b: &mut [u8], ds: &[Entry], mem: &mut Memory, slot: u32) {
    let at = slot_offset(b, slot);
    for o in [at, at + 7] {
        if let Some(x) = b.get_mut(o) {
            *x = 0;
        }
    }
    put_u32(b, 0x10, slot);
    put_u32(b, 0, 0);
    summaries(b, ds, mem);
}

/// Copy every slot's summary into the `dAutosave_Summary_NN_*` entries and the header's slot into the current-slot
/// entry (`0x4eaea0`, after loading, preparing and deleting).
pub fn summaries(b: &[u8], ds: &[Entry], mem: &mut Memory) {
    let position = |i: u8| POSITIONS.get(i as usize).map_or(0, |s| name_hash(s));
    let mut at = HEADER;
    let stride = u32_at(b, 0xc) as usize + 8;
    for n in 1..=u32_at(b, 4) {
        // **[I]** a slot count past the buffer stops at its end
        let Some(s) = b.get(at..at + 8) else {
            break;
        };
        let set = |mem: &mut Memory, field: &str, v: u32| {
            set_first_u32(ds, mem, name_hash(&format!("{}Autosave_Summary_{n:02}_{field}", prefix(field))), v)
        };
        set(mem, "QCards", s[1] as u32);
        set(mem, "House", s[2] as u32);
        set(mem, "HousePosition", position(s[3]));
        set(mem, "HouseCup", s[4] as u32);
        set(mem, "Exhibition", s[5] as u32);
        set(mem, "ExhibitionPosition", position(s[6]));
        set(mem, "Counter", s[7] as u32);
        at += stride;
    }
    set_first_u32(ds, mem, CURRENT_SLOT, u32_at(b, 0x10));
}

/// The names' prefix: `str` for the two position strings, `d` for the rest.
fn prefix(field: &str) -> &'static str {
    if field.ends_with("Position") {
        "str"
    } else {
        "d"
    }
}

/// The message kind of the two-button question a few states ask (`0x4eb870` with `(kind, -1, -1, 0, 2)`).
const ASK_FAILED: u32 = 2;
const ASK_CORRUPT: u32 = 0x11;

impl FrontEnd {
    /// One frame of the manager (`0x4eb660`, called at the end of the front-end manager's update `0x4e2950`): a
    /// step of the state machine (`0x4ebb30`) while enabled.
    pub fn memcard_frame(&mut self, vm: &mut Vm) {
        if !self.memcard.enabled {
            return;
        }
        let s = self.memcard.state;
        if std::env::var_os("QWC_MC_LOG").is_some() && self.memcard.logged != Some(s) {
            self.memcard.logged = Some(s);
            eprintln!("memcard: state {s:#x} (result {}, sub {})", self.memcard.result, self.memcard.sub);
        }
        match s {
            // enter states that only reset (`0x4ebb30` inline)
            2 | 8 | 0x23 => self.mc_goto(s + 1, true),
            5 | 6 | 0x20 => self.mc_goto(s + 1, false),
            0x21 => self.memcard.state = 0x22,
            3 => self.mc_check_space(vm),
            9 => self.mc_delete_file(vm),
            0xb | 0xe => self.mc_begin_load(s + 1),
            0xc => self.mc_load(vm, 0xd, true),
            0xf => self.mc_load(vm, 0x10, false),
            0xd | 0x10 => self.mc_finish_load(vm, s),
            // `0x4ec890`: both save states go to 0x12 (so script state 7 reports as 6)
            0x11 | 0x14 => self.mc_begin_save(&vm.mem),
            0x12 => self.mc_save(vm, 0x13),
            0x15 => self.mc_save(vm, 0x16),
            0x17 => self.mc_ask(vm, 0x18, 10),
            0x1a => self.mc_ask(vm, 0x1b, 0xb),
            0x26 => self.mc_ask(vm, 0x27, 0xd),
            0x29 => self.mc_ask(vm, 0x2a, 0xe),
            0x2c => self.mc_ask(vm, 0x2d, 0x14),
            0x18 => self.mc_answer(0x19, 3),
            0x1b => self.mc_answer(0x1c, 4),
            0x27 => self.mc_answer(0x28, 4),
            0x2a => self.mc_answer(0x2b, 4),
            0x2d => self.mc_answer(0x2e, 4),
            0x1d => self.mc_notice(vm),
            0x1e => self.mc_wait(0x1f),
            // `0x4ecd10`
            0x24 => {
                self.memcard.state = 0x25;
                self.memcard.result = if self.memcard.io.exists() { 0 } else { 10 };
            }
            4 | 7 | 10 | 0x13 | 0x16 | 0x19 | 0x1c | 0x1f | 0x22 | 0x25 | 0x28 | 0x2b | 0x2e => self.mc_report(vm, s),
            _ => {}
        }
    }

    /// `Memcard_EnableMemoryCards` (`0x4d3b40`): on, the first time (or after an off) make the save buffer when
    /// there is none and go idle (`0x4eb690`); off stops the machine and keeps the buffer.
    pub(crate) fn mc_enable(&mut self, on: bool) {
        let c = &mut self.memcard;
        if !on {
            c.enabled = false;
            return;
        }
        if c.enabled {
            return;
        }
        if c.buffer.is_empty() {
            c.buffer = new_buffer(&self.datastore, &SAVE_KEYS, SLOTS);
        }
        c.state = 1;
        c.result = 0;
        c.work = None;
        c.enabled = true;
    }

    fn mc_goto(&mut self, state: u32, reset_sub: bool) {
        let c = &mut self.memcard;
        c.state = state;
        c.result = 0;
        if reset_sub {
            c.sub = 0;
        }
    }

    /// The answers a two-button message offers (the four words at `+0xfc`).
    fn mc_offer(&mut self) {
        self.memcard.answers = [3, 4, 0, 0];
    }

    fn mc_answered(&self) -> u32 {
        self.memcard.answers.get(self.memcard.reply as usize).copied().unwrap_or(0)
    }

    /// Run a registered callback (`0x4eb810`): its layer must be alive (`+0x54`); errors go to the log.
    fn mc_call(&mut self, vm: &mut Vm, cb: Option<(usize, u32)>, args: &[u32]) {
        let Some((l, hash)) = cb else {
            return;
        };
        if hash == 0 || !self.layers.get(l).is_some_and(|x| x.alive) {
            return;
        }
        let mut a = vec![layer::layer_handle(l)];
        a.extend_from_slice(args);
        let script = self.layers[l].script;
        if let Err(e) = vm.execute(self, script, hash, &a) {
            self.log.push(format!("memcard callback {hash:08x}: {e}"));
        }
    }

    /// The message callback with `(kind, six words)` (`0x4eb870` and the states that build the block themselves).
    fn mc_message(&mut self, vm: &mut Vm, words: [u32; 7]) {
        self.memcard.message_up = words[0] != u32::MAX;
        let cb = self.memcard.message_cb;
        self.mc_call(vm, cb, &words);
    }

    /// `0x4eb870(kind, -1, -1, 0, 2)`: the question with two buttons. **[I]** its last two words are whatever the
    /// caller's stack held; 0 here.
    fn mc_ask_two(&mut self, vm: &mut Vm, kind: u32) {
        self.mc_message(vm, [kind, u32::MAX, u32::MAX, 0, 2, 0, 0]);
    }

    /// A state's report (`0x4eb8d0` and the states that call it): the state callback with `(result, script
    /// state)`, then idle unless the callback changed the state.
    fn mc_report(&mut self, vm: &mut Vm, state: u32) {
        let (cb, result) = (self.memcard.state_cb, self.memcard.result);
        self.mc_call(vm, cb, &[result, script_state(state)]);
        if self.memcard.state == state {
            self.memcard.state = 1;
        }
    }

    /// `0x4ec030`: room for the save, then whether the file is there (result 0 yes, 10 no); without room, ask
    /// (kind 4) to retry or give up (result 0x14).
    fn mc_check_space(&mut self, vm: &mut Vm) {
        match self.memcard.sub {
            0 => {
                let need = self.memcard.buffer.len() as u64;
                if self.memcard.io.free_bytes().is_none_or(|f| f < need) {
                    self.memcard.sub = 1;
                    return;
                }
                let found = self.memcard.io.exists();
                self.memcard.state = 4;
                self.memcard.result = if found { 0 } else { 10 };
            }
            1 => {
                self.mc_offer();
                self.mc_message(vm, [4, 1, 2, u32::MAX, u32::MAX, 0, 2]);
                self.memcard.waiting = true;
                self.memcard.sub = 2;
            }
            2 if !self.memcard.waiting => {
                if self.mc_answered() == 3 {
                    self.memcard.sub = 0;
                } else {
                    self.memcard.result = 0x14;
                    self.memcard.state = 4;
                }
            }
            _ => {}
        }
    }

    /// `0x4ec1d0`: ask (kind 0x11), then delete the file (result 0); a failed delete asks again (kind 2). A "no"
    /// gives result 1.
    fn mc_delete_file(&mut self, vm: &mut Vm) {
        match self.memcard.sub {
            0 | 3 => {
                self.mc_offer();
                let kind = if self.memcard.sub == 0 { ASK_CORRUPT } else { ASK_FAILED };
                self.mc_ask_two(vm, kind);
                self.memcard.waiting = true;
                self.memcard.sub += 1;
            }
            1 | 4 if !self.memcard.waiting => {
                let retry = self.mc_answered() == 3;
                // after the first question 3 means keep the file; after a failure it means try again
                if (self.memcard.sub == 1) != retry {
                    self.memcard.sub = 2;
                } else {
                    self.memcard.result = 1;
                    self.memcard.state = 10;
                }
            }
            2 => {
                if self.memcard.io.delete() {
                    self.memcard.result = 0;
                    self.memcard.state = 10;
                } else {
                    self.memcard.sub = 3;
                }
            }
            _ => {}
        }
    }

    /// `0x4ec340` / `0x4ec5f0`: a buffer to load into; the result starts at -1.
    fn mc_begin_load(&mut self, next: u32) {
        let c = &mut self.memcard;
        c.state = next;
        c.sub = 0;
        c.result = u32::MAX;
        // **[I]** the original's buffer is not cleared
        c.work = Some(vec![0; c.buffer.len()]);
    }

    /// `0x4ec380` / `0x4ec630`: read the file. No file gives result 10; a read error asks (kind 0x11) to retry or
    /// give up (result 1); a bad save asks (kind 0x11) to retry or give up (result 0xe, and the first variant
    /// deletes the file).
    fn mc_load(&mut self, vm: &mut Vm, done: u32, delete_bad: bool) {
        match self.memcard.sub {
            0 => {
                let len = self.memcard.buffer.len();
                match self.memcard.io.read(len) {
                    Err(CardError::Open) => {
                        self.memcard.result = 10;
                        self.memcard.state = done;
                    }
                    Err(CardError::Transfer) => self.memcard.sub = 1,
                    Ok(bytes) => {
                        let w = self.memcard.work.get_or_insert_with(Vec::new);
                        w.resize(len, 0);
                        w[..bytes.len()].copy_from_slice(&bytes);
                        if is_bad(w, &self.memcard.buffer) {
                            self.memcard.sub = 3;
                        } else {
                            self.memcard.result = 0;
                            self.memcard.state = done;
                        }
                    }
                }
            }
            1 | 3 => {
                self.mc_offer();
                self.mc_ask_two(vm, ASK_CORRUPT);
                self.memcard.sub += 1;
                self.memcard.waiting = true;
            }
            2 | 4 if !self.memcard.waiting => {
                if self.mc_answered() == 3 {
                    self.memcard.sub = 0;
                } else if self.memcard.sub == 2 {
                    self.memcard.result = 1;
                    self.memcard.state = done;
                } else {
                    if delete_bad {
                        self.memcard.io.delete();
                    }
                    self.memcard.result = 0xe;
                    self.memcard.state = done;
                }
            }
            _ => {}
        }
    }

    /// `0x4ec580` / `0x4ec820`: a good load becomes the save buffer and its summaries go to the DataStore; then the
    /// report.
    fn mc_finish_load(&mut self, vm: &mut Vm, state: u32) {
        let work = self.memcard.work.take();
        if self.memcard.result == 0 {
            if let Some(w) = work {
                let n = self.memcard.buffer.len().min(w.len());
                self.memcard.buffer[..n].copy_from_slice(&w[..n]);
                summaries(&self.memcard.buffer, &self.datastore, &mut vm.mem);
            }
        }
        self.mc_report(vm, state);
    }

    /// `0x4ec890`: pack the current slot and go to the save.
    fn mc_begin_save(&mut self, mem: &Memory) {
        self.memcard.state = 0x12;
        self.memcard.sub = 0;
        pack(&mut self.memcard.buffer, &self.datastore, mem, &SAVE_KEYS, false);
        self.memcard.result = 0;
    }

    /// `0x4ec8d0` / `0x4eca80`: write the file. A failure asks (kind 2) to retry or give up (result 1); success
    /// clears the message (kind -1) and waits 0x60 frames before reporting 0.
    fn mc_save(&mut self, vm: &mut Vm, done: u32) {
        match self.memcard.sub {
            0 => {
                let r = self.memcard.io.write(&self.memcard.buffer);
                self.memcard.sub = match r {
                    Ok(()) => 4,
                    Err(CardError::Open) => 1,
                    Err(CardError::Transfer) => 3,
                };
            }
            1 | 3 => {
                self.mc_offer();
                self.mc_ask_two(vm, ASK_FAILED);
                self.memcard.waiting = true;
                self.memcard.sub = 2;
            }
            2 if !self.memcard.waiting => {
                if self.mc_answered() != 3 {
                    self.memcard.result = 1;
                    self.memcard.state = done;
                } else {
                    self.memcard.sub = 0;
                }
            }
            4 => {
                // **[I]** the last two words, as in `mc_ask_two`
                self.mc_message(vm, [u32::MAX, u32::MAX, u32::MAX, u32::MAX, 0, 0, 0]);
                self.memcard.sub = 5;
                self.memcard.timer = 0x60;
            }
            5 => self.mc_wait(done),
            _ => {}
        }
    }

    /// Count the timer down past 0, then result 0 and `next` (`0x4ece00`, the save's last step).
    fn mc_wait(&mut self, next: u32) {
        let c = &mut self.memcard;
        if c.timer >= 0 {
            c.timer -= 1;
        } else {
            c.result = 0;
            c.state = next;
        }
    }

    /// `0x4ecc30` and its four copies: a two-button question of `kind` (words `0xf, 0x10, -1, -1, 1, 2`).
    fn mc_ask(&mut self, vm: &mut Vm, next: u32, kind: u32) {
        let c = &mut self.memcard;
        c.state = next;
        c.waiting = true;
        c.result = 0;
        self.mc_offer();
        self.mc_message(vm, [kind, 0xf, 0x10, u32::MAX, u32::MAX, 1, 2]);
    }

    /// `0x4eccc0` and copies: once answered, the result is 1 when the answer means `yes`.
    fn mc_answer(&mut self, next: u32, yes: u32) {
        if !self.memcard.waiting {
            let a = self.mc_answered();
            self.memcard.state = next;
            self.memcard.result = (a == yes) as u32;
        }
    }

    /// `0x4ecda0`: a notice with no buttons (kind 0xc), shown for 0x60 frames.
    fn mc_notice(&mut self, vm: &mut Vm) {
        let c = &mut self.memcard;
        c.state = 0x1e;
        c.result = 0;
        c.waiting = false;
        self.mc_message(vm, [0xc, u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, 0]);
        self.memcard.timer = 0x60;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A DataStore of `(key, type, count, stride, bytes)` in fresh memory.
    fn store(entries: &[(u32, u32, u16, u16, &[u8])]) -> (Vec<Entry>, Memory) {
        let mut mem = Memory::new();
        let ds = entries
            .iter()
            .map(|&(key, kind, count, stride, data)| {
                let addr = mem.alloc(count as usize * stride as usize);
                mem.write(addr, data).unwrap();
                Entry { key, kind, count, stride, addr }
            })
            .collect();
        (ds, mem)
    }

    fn bytes(ds: &[Entry], mem: &Memory, key: u32) -> Vec<u8> {
        let e = find(ds, key).unwrap();
        mem.read(e.addr, entry_len(&e)).unwrap().to_vec()
    }

    #[test]
    fn states_map_as_change_state_does() {
        assert_eq!(machine_state(0), Some(1));
        assert_eq!(machine_state(1), Some(2));
        assert_eq!(machine_state(6), Some(0x11));
        assert_eq!(machine_state(15), Some(0x2c));
        assert_eq!(machine_state(16), None);
        assert_eq!([1, 2, 4, 5, 0x13, 0x2e].map(script_state), [0, 1, 1, 2, 6, 15]);
    }

    #[test]
    fn sizes_and_checksum() {
        // records: 12 + 3 rounded to 16, 12 + 8 = 20
        let (ds, _) = store(&[(7, 0, 3, 1, &[1, 2, 3]), (9, 2, 2, 4, &[0; 8])]);
        assert_eq!(slot_size(&ds, &[7, 9]), 36);
        assert_eq!(slot_size(&ds, &[7, 8]), 0, "a missing key makes the size 0");
        assert_eq!(schema_checksum(&ds, &[7, 9, 8]), 3 + 7 + 2 + 2 + 9);
        let b = new_buffer(&ds, &[7, 9], 4);
        assert_eq!(b.len(), (36 + 8) * 4 + HEADER);
        assert_eq!([u32_at(&b, 0), u32_at(&b, 4), u32_at(&b, 8), u32_at(&b, 0xc), u32_at(&b, 0x10)], [0, 4, 23, 36, 0]);
    }

    #[test]
    fn pack_then_activate_restores_the_slot() {
        let keys = [7, 9];
        let (mut ds, mut mem) = store(&[
            (7, 0, 3, 1, &[1, 2, 3]),
            (9, 2, 2, 4, &[1, 0, 0, 0, 2, 0, 0, 0]),
            (CURRENT_SLOT, 2, 1, 4, &[2, 0, 0, 0]),
            (summary::QCARDS, 2, 1, 4, &[17, 0, 0, 0]),
            (summary::HOUSE_POSITION, 0, 1, 1, &[4]),
        ]);
        let mut b = new_buffer(&ds, &keys, 4);
        pack(&mut b, &ds, &mem, &keys, false);
        let at = HEADER + 2 * (36 + 8);
        // used, QCards 17, house 0, position 4 wraps to 0, ..., counter 1
        assert_eq!(&b[at..at + 8], &[1, 17, 0, 0, 0, 0, 0, 1]);
        assert_eq!(u32_at(&b, at + 8), 7);
        assert_eq!(&b[at + 16..at + 20], &[3, 0, 1, 0]);
        assert_eq!(&b[at + 20..at + 23], &[1, 2, 3]);
        assert_eq!(u32_at(&b, at + 24), 9);
        assert_eq!(u32_at(&b, 0x10), 2);
        assert_eq!(u32_at(&b, 0), crc(&b));
        assert!(!is_bad(&b, &b));

        // wipe a value, then restore slot 2; slot 0 is empty
        let k7 = find(&ds, 7).unwrap().addr;
        mem.write(k7, &[0, 0, 0]).unwrap();
        assert_eq!(activate(&b, &mut ds, &mut mem, 0), 1);
        assert_eq!(activate(&b, &mut ds, &mut mem, 2), 0);
        assert_eq!(bytes(&ds, &mem, 7), [1, 2, 3]);
        assert_eq!(bytes(&ds, &mem, CURRENT_SLOT), [2, 0, 0, 0]);

        // a key the DataStore lacks is created on restore
        let mut ds2: Vec<Entry> = ds.iter().copied().filter(|e| e.key != 9).collect();
        activate(&b, &mut ds2, &mut mem, 2);
        assert_eq!(bytes(&ds2, &mem, 9), [1, 0, 0, 0, 2, 0, 0, 0]);
    }

    #[test]
    fn a_save_on_the_way_to_the_card_intro_keeps_it_shown() {
        let keys = [CARDS_INTRO_SHOWN];
        let (mut ds, mut mem) = store(&[
            (CARDS_INTRO_SHOWN, 0, 1, 1, &[0]),
            (PREVIOUS_SCREEN, 2, 1, 4, &name_hash("ChallengeResults").to_le_bytes()),
            (CURRENT_SLOT, 2, 1, 4, &[0; 4]),
        ]);
        assert_eq!(name_hash("ChallengeQuidditchCardIntro"), CARD_INTRO);
        assert_eq!(name_hash("QuidditchCardsIntroShown"), CARDS_INTRO_SHOWN);
        assert_eq!(name_hash("HogwartsPreviousScreenID"), PREVIOUS_SCREEN);
        let mut b = new_buffer(&ds, &keys, 1);
        pack(&mut b, &ds, &mem, &keys, false);
        assert_eq!(b[HEADER + 8 + 12], 0);
        let at = find(&ds, PREVIOUS_SCREEN).unwrap().addr;
        mem.set_u32(at, CARD_INTRO).unwrap();
        pack(&mut b, &ds, &mem, &keys, false);
        assert_eq!(b[HEADER + 8 + 12], 1);
        // the game's own value is left as it is, for the intro to read
        assert_eq!(bytes(&ds, &mem, CARDS_INTRO_SHOWN), [0]);
        activate(&b, &mut ds, &mut mem, 0);
        assert_eq!(bytes(&ds, &mem, CARDS_INTRO_SHOWN), [1]);
    }

    #[test]
    fn bad_saves_and_deleted_slots() {
        let (ds, mut mem) = store(&[(7, 0, 3, 1, &[1, 2, 3]), (CURRENT_SLOT, 2, 1, 4, &[0; 4])]);
        let mut b = new_buffer(&ds, &[7], 2);
        pack(&mut b, &ds, &mem, &[7], false);
        let mut flipped = b.clone();
        flipped[HEADER + 9] ^= 1;
        assert!(is_bad(&flipped, &b), "CRC");
        let mut other = b.clone();
        put_u32(&mut other, 8, 1);
        let c = crc(&other);
        put_u32(&mut other, 0, c);
        assert!(is_bad(&other, &b), "schema checksum");
        let mut spoilt = b.clone();
        pack(&mut spoilt, &ds, &mem, &[7], true);
        assert!(is_bad(&spoilt, &b), "spoilt CRC");

        delete_slot(&mut b, &ds, &mut mem, 0);
        assert_eq!((b[HEADER], b[HEADER + 7], u32_at(&b, 0)), (0, 0, 0));
        assert_eq!(activate(&b, &mut ds.clone(), &mut mem, 0), 1);
    }

    #[test]
    fn summaries_fill_the_named_entries() {
        let names =
            ["dAutosave_Summary_02_QCards", "strAutosave_Summary_02_HousePosition", "dAutosave_Summary_02_Counter"];
        let mut list: Vec<(u32, u32, u16, u16, &[u8])> =
            names.iter().map(|n| (name_hash(n), 2u32, 1u16, 4u16, &[0u8; 4][..])).collect();
        list.push((CURRENT_SLOT, 2, 1, 4, &[0; 4]));
        let (ds, mut mem) = store(&list);
        let mut b = vec![0u8; HEADER + 2 * 8];
        put_u32(&mut b, 4, 2);
        put_u32(&mut b, 0x10, 1);
        b[HEADER + 8..HEADER + 16].copy_from_slice(&[1, 9, 0, 3, 0, 0, 0, 1]);
        summaries(&b, &ds, &mut mem);
        let v = |n: &str| u32_at(&bytes(&ds, &mem, name_hash(n)), 0);
        assert_eq!(v(names[0]), 9);
        assert_eq!(v(names[1]), name_hash("Third"));
        assert_eq!(v(names[2]), 1);
        assert_eq!(u32_at(&bytes(&ds, &mem, CURRENT_SLOT), 0), 1);
    }

    /// Run frames until the machine is idle again (at most `n`); the number of frames it took.
    fn settle(fe: &mut FrontEnd, vm: &mut Vm, n: usize) -> usize {
        for i in 0..n {
            fe.memcard_frame(vm);
            if fe.memcard.state == 1 {
                return i + 1;
            }
        }
        n
    }

    /// Answer a waiting message with button `i`, as `Memcard_Reply` does.
    fn reply(fe: &mut FrontEnd, i: u32) {
        assert!(fe.memcard.waiting);
        fe.memcard.waiting = false;
        fe.memcard.reply = i;
    }

    #[test]
    fn check_save_and_load_on_a_memory_card() {
        let mut vm = Vm::new();
        let mut fe = FrontEnd::new();
        fe.mc_enable(true);
        assert_eq!((fe.memcard.state, fe.memcard.buffer.len()), (1, HEADER + 8 * SLOTS as usize));

        // check: no file
        fe.memcard.state = machine_state(1).unwrap();
        assert_eq!(settle(&mut fe, &mut vm, 10), 3);
        assert_eq!(fe.memcard.result, 10);

        // save: pack, write, the notice, 0x61 frames of timer, the report
        fe.memcard.state = machine_state(6).unwrap();
        assert_eq!(settle(&mut fe, &mut vm, 200), 1 + 1 + 1 + 0x62 + 1);
        assert_eq!(fe.memcard.result, 0);
        fe.memcard.state = machine_state(12).unwrap();
        settle(&mut fe, &mut vm, 10);
        assert_eq!(fe.memcard.result, 0, "the file exists now");

        // load: a good file loads; a spoilt one asks, and giving up deletes it (result 0xe)
        fe.memcard.state = machine_state(4).unwrap();
        settle(&mut fe, &mut vm, 10);
        assert_eq!(fe.memcard.result, 0);
        let mut bad = fe.memcard.buffer.clone();
        bad[0] ^= 1;
        fe.memcard.io = Box::new(MemoryCard { file: Some(bad), ..Default::default() });
        fe.memcard.state = machine_state(4).unwrap();
        for _ in 0..4 {
            fe.memcard_frame(&mut vm);
        }
        assert_eq!(fe.memcard.sub, 4);
        reply(&mut fe, 1);
        settle(&mut fe, &mut vm, 10);
        assert_eq!(fe.memcard.result, 0xe);
        fe.memcard.state = machine_state(12).unwrap();
        settle(&mut fe, &mut vm, 10);
        assert_eq!(fe.memcard.result, 10, "deleted");
    }

    #[test]
    fn no_room_and_failed_writes_ask() {
        let mut vm = Vm::new();
        let mut fe = FrontEnd::new();
        fe.memcard.io = Box::new(MemoryCard { free: Some(0), ..Default::default() });
        fe.mc_enable(true);
        fe.memcard.state = machine_state(1).unwrap();
        for _ in 0..3 {
            fe.memcard_frame(&mut vm);
        }
        assert_eq!(fe.memcard.answers, [3, 4, 0, 0]);
        reply(&mut fe, 1);
        settle(&mut fe, &mut vm, 10);
        assert_eq!(fe.memcard.result, 0x14);

        fe.memcard.io = Box::new(MemoryCard { fail_write: Some(CardError::Transfer), ..Default::default() });
        fe.memcard.state = machine_state(7).unwrap();
        for _ in 0..3 {
            fe.memcard_frame(&mut vm);
        }
        reply(&mut fe, 1);
        fe.memcard_frame(&mut vm);
        // script state 7 runs the state-6 save, so it reports from 0x13
        assert_eq!((fe.memcard.state, fe.memcard.result), (0x13, 1));
    }

    #[test]
    fn questions_wait_for_the_reply_and_disable_stops() {
        let mut vm = Vm::new();
        let mut fe = FrontEnd::new();
        fe.mc_enable(true);
        fe.memcard.state = machine_state(8).unwrap();
        fe.memcard_frame(&mut vm);
        assert_eq!(fe.memcard.state, 0x18);
        fe.memcard_frame(&mut vm);
        assert_eq!(fe.memcard.state, 0x18, "waits for the reply");
        reply(&mut fe, 0);
        settle(&mut fe, &mut vm, 3);
        assert_eq!(fe.memcard.result, 1);
        fe.mc_enable(false);
        fe.memcard.state = machine_state(1).unwrap();
        fe.memcard_frame(&mut vm);
        assert_eq!(fe.memcard.state, 2, "stopped while disabled");
    }
}
