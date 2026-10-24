//! The PC key bindings (`PadManager`, `0x508a00`-`0x509c00`; the natives `0x4d46b0`-`0x4d4a90`), as traced in
//! `docs/FECK_VM.md` ("Key bindings").
//!
//! One table of words (`0x609478`) holds four groups separated by `0xf0f0f0f0` and ended by `0xffffffff`. Each
//! entry maps an action (low 16 bits) to a key (high 16 bits). Each player uses a group, chosen by an engine
//! setting that start-up sets to group 0 for player 0 and group 1 for player 1. The bindings are saved to
//! `KeyConfig data` next to the memory-card file.

use std::path::PathBuf;

/// Separates the groups.
pub const SEPARATOR: u32 = 0xf0f0f0f0;
/// Ends the table.
pub const END: u32 = 0xffffffff;
/// The file's first word (`0x609350`); a file with another version is ignored.
pub const VERSION: u32 = 0x50440101;
/// The file name in the save folder (`0x5a9218`).
pub const FILE_NAME: &str = "KeyConfig data";

/// The defaults the start-up copies in (`0x609358`, 284 bytes).
pub const DEFAULTS: [u32; 71] = [
    0x00000003, 0x00000000, 0x00f20001, 0x00fa0002, 0x00f50004, 0x00f70005, 0x00020006, 0x00010009, 0x0076000c,
    0x00770010, 0x00770011, 0x0075000f, 0x0072000e, 0x00710012, 0x00730013, 0x00460016, 0x000b000c, 0x000c0010,
    0x000c0011, 0xf0f0f0f0, 0x00000003, 0x00000000, 0x003b0001, 0x00490002, 0x00480004, 0x004a0005, 0x00000006,
    0x00000009, 0x004e000c, 0x004f0010, 0x004f0011, 0x004d000f, 0x0040000e, 0x003f0012, 0x00410013, 0x00630016,
    0xf0f0f0f0, 0x00040003, 0x00050000, 0x00230001, 0x00270002, 0x00290004, 0x00250005, 0x00000006, 0x00000009,
    0x0013000c, 0x00140010, 0x00140011, 0x0015000f, 0x0016000e, 0x00180012, 0x00190013, 0x00170016, 0xf0f0f0f0,
    0x00040003, 0x00050000, 0x00230001, 0x00270002, 0x00290004, 0x00250005, 0x00000006, 0x00000009, 0x0013000c,
    0x00140010, 0x00140011, 0x0015000f, 0x0016000e, 0x00180012, 0x00190013, 0x00170016, 0xffffffff,
];

/// Per action, its type (`0x609990`): `General_IsKeyTypeCompatible` wants a pad button exactly for type 2.
/// **[I]** the table's length is not known; actions past these read 0.
pub const ACTION_TYPES: [u8; 24] = [2, 0, 0, 2, 0, 0, 2, 0, 0, 2, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2];

/// The actions a player binds from the keyboard and mouse, as the remaster's launcher lists them (the action, a
/// name): the moves, then the buttons. Shoot (`0x10`) also binds the special move (`0x11`), as [`Keys::set`] does.
/// The pass directions (actions 6-11) are the pad's second stick and are not listed.
pub const ACTIONS: [(u32, &str); 11] = [
    (1, "Move up"),
    (2, "Move down"),
    (4, "Move left"),
    (5, "Move right"),
    (0xc, "Pass / tackle"),
    (0x10, "Shoot"),
    (0xf, "Boost"),
    (0xe, "Dodge"),
    (0x12, "Combo left"),
    (0x13, "Combo right"),
    (0x16, "Uber move"),
];

/// The keys the port reads (`qwc-game`'s `controls`), by their id (a DirectInput scan code plus 42; 11-13 the
/// mouse's buttons), with the remaster's names for them (the original names keys in its own text).
pub const KEY_NAMES: &[(u32, &str)] = &[
    (11, "Left mouse button"),
    (12, "Right mouse button"),
    (13, "Middle mouse button"),
    (42 + 0x01, "Escape"),
    (42 + 0x02, "1"),
    (42 + 0x03, "2"),
    (42 + 0x04, "3"),
    (42 + 0x05, "4"),
    (42 + 0x06, "5"),
    (42 + 0x07, "6"),
    (42 + 0x08, "7"),
    (42 + 0x09, "8"),
    (42 + 0x0a, "9"),
    (42 + 0x0b, "0"),
    (42 + 0x0c, "-"),
    (42 + 0x0d, "="),
    (42 + 0x0e, "Backspace"),
    (42 + 0x0f, "Tab"),
    (42 + 0x10, "Q"),
    (42 + 0x11, "W"),
    (42 + 0x12, "E"),
    (42 + 0x13, "R"),
    (42 + 0x14, "T"),
    (42 + 0x15, "Y"),
    (42 + 0x16, "U"),
    (42 + 0x17, "I"),
    (42 + 0x18, "O"),
    (42 + 0x19, "P"),
    (42 + 0x1a, "["),
    (42 + 0x1b, "]"),
    (42 + 0x1c, "Return"),
    (42 + 0x1d, "Left Ctrl"),
    (42 + 0x1e, "A"),
    (42 + 0x1f, "S"),
    (42 + 0x20, "D"),
    (42 + 0x21, "F"),
    (42 + 0x22, "G"),
    (42 + 0x23, "H"),
    (42 + 0x24, "J"),
    (42 + 0x25, "K"),
    (42 + 0x26, "L"),
    (42 + 0x27, ";"),
    (42 + 0x28, "'"),
    (42 + 0x29, "`"),
    (42 + 0x2a, "Left Shift"),
    (42 + 0x2b, "\\"),
    (42 + 0x2c, "Z"),
    (42 + 0x2d, "X"),
    (42 + 0x2e, "C"),
    (42 + 0x2f, "V"),
    (42 + 0x30, "B"),
    (42 + 0x31, "N"),
    (42 + 0x32, "M"),
    (42 + 0x33, ","),
    (42 + 0x34, "."),
    (42 + 0x35, "/"),
    (42 + 0x36, "Right Shift"),
    (42 + 0x37, "Numpad *"),
    (42 + 0x38, "Left Alt"),
    (42 + 0x39, "Space"),
    (42 + 0x47, "Numpad 7"),
    (42 + 0x48, "Numpad 8"),
    (42 + 0x49, "Numpad 9"),
    (42 + 0x4a, "Numpad -"),
    (42 + 0x4b, "Numpad 4"),
    (42 + 0x4c, "Numpad 5"),
    (42 + 0x4d, "Numpad 6"),
    (42 + 0x4e, "Numpad +"),
    (42 + 0x4f, "Numpad 1"),
    (42 + 0x50, "Numpad 2"),
    (42 + 0x51, "Numpad 3"),
    (42 + 0x52, "Numpad 0"),
    (42 + 0x53, "Numpad ."),
    (42 + 0x9c, "Numpad Enter"),
    (42 + 0x9d, "Right Ctrl"),
    (42 + 0xb5, "Numpad /"),
    (42 + 0xb8, "Right Alt"),
    (42 + 0xc7, "Home"),
    (42 + 0xc8, "Up"),
    (42 + 0xc9, "Page Up"),
    (42 + 0xcb, "Left"),
    (42 + 0xcd, "Right"),
    (42 + 0xcf, "End"),
    (42 + 0xd0, "Down"),
    (42 + 0xd1, "Page Down"),
    (42 + 0xd2, "Insert"),
    (42 + 0xd3, "Delete"),
];

/// Key `id`'s name: [`KEY_NAMES`]', a pad's button, or its number.
pub fn key_name(id: u32) -> String {
    match KEY_NAMES.iter().find(|(k, _)| *k == id) {
        Some((_, n)) => n.to_string(),
        None if id == 0 => "none".into(),
        None if is_button(id) => format!("pad button {id}"),
        None => format!("key {id}"),
    }
}

/// Whether key `k` is a pad button: the key descriptors (`0x5f6bf4`) give 1-10 kind `0x20000` and 11-298 kind
/// `0x200000` (keyboard); `IsKeyTypeCompatible` treats kinds `0x10000` and `0x20000` as buttons.
pub fn is_button(k: u32) -> bool {
    (1..=10).contains(&k)
}

/// The bindings and their file.
#[derive(Debug, Clone)]
pub struct Keys {
    pub table: Vec<u32>,
    /// Each player's group (the engine setting `0x1043748`). **[I]** players past these use group 0.
    pub groups: [u8; 2],
    /// `KeyConfig data`, when the host gave a save folder.
    pub file: Option<PathBuf>,
    /// The key just pressed, for `General_GetRawKey` (the host's input; 0 for none).
    pub raw_key: u32,
}

impl Default for Keys {
    fn default() -> Self {
        Keys { table: DEFAULTS.to_vec(), groups: [0, 1], file: None, raw_key: 0 }
    }
}

impl Keys {
    /// The defaults with `file`, then the file read when it is there (as start-up does).
    pub fn with_file(file: PathBuf) -> Keys {
        let mut k = Keys { file: Some(file), ..Keys::default() };
        k.load();
        k
    }

    /// Bind `key` to `action` for `player` as the game's Controls menu does: the key taken from wherever it was
    /// bound (`General_IsKeyFree`, which clears it), then set (`General_SetControlKey`).
    pub fn rebind(&mut self, player: u32, action: u32, key: u32) {
        self.is_free(player, key);
        self.set(player, action, key);
    }

    fn group(&self, player: u32) -> u32 {
        self.groups.get(player as u8 as usize).copied().unwrap_or(0) as u32
    }

    /// The index of the first entry after `n` separators, or `None` when the table ends first.
    fn start(&self, mut n: u32) -> Option<usize> {
        let mut i = 0;
        while n != 0 {
            match *self.table.get(i)? {
                END => return None,
                SEPARATOR => n -= 1,
                _ => {}
            }
            i += 1;
        }
        Some(i)
    }

    /// The first entry for `action` from the player's group on, past separators and into the later groups, as
    /// `0x4d46b0` and `0x4d4770` search.
    fn find(&self, player: u32, action: u32) -> Option<usize> {
        let mut i = self.start(self.group(player))?;
        loop {
            match *self.table.get(i)? {
                END => return None,
                SEPARATOR => {}
                w if w & 0xffff == action => return Some(i),
                _ => {}
            }
            i += 1;
        }
    }

    /// `General_GetControlKey(player, action)` (`0x4d46b0`): the bound key, 0 when none.
    pub fn get(&self, player: u32, action: u32) -> u32 {
        self.find(player, action).map_or(0, |i| self.table[i] >> 16)
    }

    /// `General_SetControlKey(player, action, key)` (`0x4d4770`): rebind; action 0x10 also sets 0x11 (first).
    pub fn set(&mut self, player: u32, action: u32, key: u32) {
        if action == 0x10 {
            self.set(player, 0x11, key);
        }
        if let Some(i) = self.find(player, action) {
            self.table[i] = (self.table[i] & 0xffff) | key << 16;
        }
    }

    /// `0x4d4870`: whether `group` binds no action to `key`. A binding found is cleared (and, for action 0x10,
    /// the next entry's key too). This search stops at the group's separator.
    fn free_in(&mut self, group: u32, key: u32) -> bool {
        let Some(mut i) = self.start(group) else {
            return true;
        };
        while let Some(&w) = self.table.get(i) {
            if w == END || w == SEPARATOR {
                break;
            }
            if w >> 16 == key {
                self.table[i] = w & 0xffff;
                if self.table[i] == 0x10 {
                    if let Some(n) = self.table.get_mut(i + 1) {
                        *n &= 0xffff;
                    }
                }
                return false;
            }
            i += 1;
        }
        true
    }

    /// `General_IsKeyFree(player, key)` (`0x4d48d0`): whether neither the player's group nor (except for keys 4-10
    /// and 19-42) the other player's binds `key`. As in the original, each binding found is cleared, so the key
    /// is free afterwards.
    pub fn is_free(&mut self, player: u32, key: u32) -> bool {
        let own = self.group(player);
        if (4..=10).contains(&key) || (19..=42).contains(&key) {
            return self.free_in(own, key);
        }
        let other = self.group((player as u8 == 0) as u32);
        let a = self.free_in(own, key);
        let b = self.free_in(other, key);
        a && b
    }

    /// `General_IsKeyTypeCompatible(action, key)` (`0x4d4a00`).
    pub fn compatible(action: u32, key: u32) -> bool {
        let pad_action = ACTION_TYPES.get(action as usize).copied().unwrap_or(0) == 2;
        pad_action == is_button(key)
    }

    /// `General_SaveKeyBindings` (`0x509250`): the version, then the table. False when it failed or there is no
    /// file.
    pub fn save(&self) -> bool {
        let Some(f) = &self.file else {
            return false;
        };
        let mut b = VERSION.to_le_bytes().to_vec();
        b.extend(self.table.iter().flat_map(|w| w.to_le_bytes()));
        std::fs::write(f, b).is_ok()
    }

    /// `General_LoadKeyBindings` (`0x5092d0`, also at start-up): a file of our version replaces the table's bytes
    /// (as many as it holds).
    pub fn load(&mut self) -> bool {
        let Some(b) = self.file.as_ref().and_then(|f| std::fs::read(f).ok()) else {
            return false;
        };
        self.load_bytes(&b)
    }

    pub fn load_bytes(&mut self, b: &[u8]) -> bool {
        if b.len() < 4 || u32::from_le_bytes(b[..4].try_into().unwrap()) != VERSION || b.len() == 4 {
            return false;
        }
        for (w, c) in self.table.iter_mut().zip(b[4..].chunks(4)) {
            let mut x = w.to_le_bytes();
            x[..c.len()].copy_from_slice(c);
            *w = u32::from_le_bytes(x);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_by_player() {
        let k = Keys::default();
        assert_eq!(k.table.len() * 4, 284);
        assert_eq!(k.get(0, 1), 0xf2);
        assert_eq!(k.get(1, 1), 0x3b);
        // group 0 lists action 0xc twice: the first wins
        assert_eq!(k.get(0, 0xc), 0x76);
        // an action missing from the group is found in a later one
        assert_eq!(k.get(1, 0x99), 0);
    }

    #[test]
    fn set_and_free() {
        let mut k = Keys::default();
        k.set(1, 0x10, 0x55);
        assert_eq!((k.get(1, 0x10), k.get(1, 0x11)), (0x55, 0x55));
        // key 0x3b is player 1's action 1: not free, and now unbound
        assert!(!k.is_free(1, 0x3b));
        assert_eq!(k.get(1, 1), 0);
        assert!(k.is_free(1, 0x3b));
        // a key of the other player's group counts too (outside 4-10 and 19-42)
        assert!(!k.is_free(1, 0xf2));
        assert_eq!(k.get(0, 1), 0);
        // freeing action 0x10's key also clears 0x11's
        let mut k = Keys::default();
        assert!(!k.is_free(0, 0x77));
        assert_eq!((k.get(0, 0x10), k.get(0, 0x11)), (0, 0), "the first entries, now unbound, still answer");
        assert_eq!(k.table[17] >> 16, 0xc, "the duplicate later in the group keeps its key");
    }

    #[test]
    fn names_and_rebinding() {
        assert_eq!(key_name(0xf2), "Up");
        assert_eq!(key_name(0x76), "Numpad 5");
        assert_eq!(key_name(11), "Left mouse button");
        assert_eq!(key_name(3), "pad button 3");
        assert_eq!(key_name(0), "none");
        // every default keyboard binding of the listed actions has a name
        let k = Keys::default();
        for p in 0..2 {
            for (a, _) in ACTIONS {
                assert!(KEY_NAMES.iter().any(|(id, _)| *id == k.get(p, a)), "player {p} action {a}");
            }
        }
        // W (player 1's up) given to player 0's shoot: taken from player 1, bound to shoot and the special move
        let mut k = Keys::default();
        k.rebind(0, 0x10, 0x3b);
        assert_eq!((k.get(0, 0x10), k.get(0, 0x11), k.get(1, 1)), (0x3b, 0x3b, 0));
    }

    #[test]
    fn types_and_files() {
        assert!(Keys::compatible(0, 3));
        assert!(!Keys::compatible(0, 0x20));
        assert!(Keys::compatible(1, 0x20));
        let mut k = Keys::default();
        k.set(0, 1, 0x30);
        let mut b = VERSION.to_le_bytes().to_vec();
        b.extend(k.table.iter().flat_map(|w| w.to_le_bytes()));
        let mut fresh = Keys::default();
        assert!(fresh.load_bytes(&b));
        assert_eq!(fresh.get(0, 1), 0x30);
        b[0] ^= 1;
        assert!(!Keys::default().load_bytes(&b), "another version");
    }
}
