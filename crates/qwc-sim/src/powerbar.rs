//! The Seeker power bars (`docs/GAMEPLAY.md`, "Power bar"; game `+0xed4`, `+0xee4`, `+0xeb4`; getter game vtable
//! `+0x2c` `0x4512a0`, adder `+0x30` `0x450e40`). Each team's bar fills with its passes (and combo chains when it
//! scores); when the two bars together reach 1 the Seeker game starts. Four sub-bars per team, each capped by its
//! `SeekerPowerBar.*Pot`, gate the dodge (0), the Bludger move (1), special moves (2) and uber moves (3); each needs
//! an unlock bit of the DataStore value `0x1527016b` (game progress), otherwise it stays empty.

/// Sub-bar indices.
pub const DODGE: usize = 0;
pub const BLUDGER: usize = 1;
pub const SPECIAL: usize = 2;
pub const UBER: usize = 3;
/// The getter's index for the main bar's copy (`+0xee4`).
pub const TOTAL: usize = 4;

/// Combo-chain bar multipliers by chain length (`0x60668c[chain]`, chain 1 to 7).
pub const COMBO: [f64; 8] = [0.0, 2.0, 3.0, 5.0, 7.0, 10.0, 15.0, 20.0];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PowerBar {
    /// `+0xed4[t]` and its copy `+0xee4[t]`.
    pub main: [f64; 2],
    pub total: [f64; 2],
    /// `+0xeb4[t][k]` and their caps (`0xfba57c + t * 0x80 + k * 4`).
    pub sub: [[f64; 4]; 2],
    pub pots: [[f64; 4]; 2],
    /// Which sub-bars the save game has unlocked (`0x445460`); all locked on a new save (inferred default). The
    /// uber move's is each team's own ([`PowerBar::uber`]).
    pub unlocked: [bool; 4],
    /// Each team's uber move unlocked (`0x445460` with k 3: an AI team's by the save's element 3 bit 3, a player's
    /// by its own progress word, `0x4455c0`).
    pub uber: [bool; 2],
    /// Game `+0x450`: the Seeker game is on (no more filling).
    pub seeker_game: bool,
}

impl PowerBar {
    /// `0x4512a0`: sub-bar `k` of team `t`, or with [`TOTAL`] the main bar's copy.
    pub fn get(&self, t: usize, k: usize) -> f64 {
        if k == TOTAL {
            self.total[t]
        } else {
            self.sub[t][k]
        }
    }

    /// `0x450e40`: add `amount` to team `t`'s bar (and, with `subs`, to its unlocked sub-bars up to their caps).
    /// Returns true when this fills the two bars together to 1, which starts the Seeker game.
    pub fn add(&mut self, t: usize, amount: f64, subs: bool) -> bool {
        if self.seeker_game {
            return false;
        }
        let before = self.main[t] + self.main[1 - t];
        self.main[t] = (self.main[t] + amount).clamp(0.0, 1.0);
        self.total[t] = (self.total[t] + amount).clamp(0.0, 1.0);
        if subs {
            for k in 0..4 {
                self.sub[t][k] =
                    if self.open(t, k) { (self.sub[t][k] + amount).clamp(0.0, self.pots[t][k]) } else { 0.0 };
            }
        }
        if self.main[t] + self.main[1 - t] >= 1.0 {
            self.main[t] = 1.0 - self.main[1 - t];
            if before < 1.0 {
                self.seeker_game = true;
                return true;
            }
        }
        false
    }

    /// Whether team `t`'s sub-bar `k` is unlocked (`0x445460`).
    pub fn open(&self, t: usize, k: usize) -> bool {
        if k == UBER {
            self.uber[t]
        } else {
            self.unlocked[k]
        }
    }

    /// `0x451310` after `0x4512f0` (game vtable `+0x40`): take `amount` from team `t`'s bar and its copy, neither
    /// below 0.
    pub fn penalise(&mut self, t: usize, amount: f64) {
        self.total[t] = (self.total[t] - amount).max(0.0);
        self.main[t] = (self.main[t] - amount).max(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_and_starts_the_seeker_game() {
        let mut b = PowerBar { pots: [[0.05, 0.1, 0.2, 0.4]; 2], ..Default::default() };
        assert!(!b.add(0, 0.6, true));
        // locked sub-bars stay empty
        assert_eq!(b.get(0, DODGE), 0.0);
        assert_eq!(b.get(0, TOTAL), 0.6);
        // team 1 reaches 0.4: together 1, the Seeker game starts, team 1's bar is trimmed to fit
        assert!(b.add(1, 0.45, true));
        assert!(b.seeker_game);
        assert!((b.main[1] - 0.4).abs() < 1e-12);
        assert!(!b.add(1, 0.1, true));
    }

    #[test]
    fn unlocked_sub_bars_fill_to_their_pots() {
        let mut b =
            PowerBar { pots: [[0.05, 0.1, 0.2, 0.4]; 2], unlocked: [true; 4], uber: [true; 2], ..Default::default() };
        b.add(1, 0.15, true);
        assert_eq!(b.sub[1], [0.05, 0.1, 0.15, 0.15]);
        b.add(1, 0.1, false);
        assert_eq!(b.sub[1], [0.05, 0.1, 0.15, 0.15]);
        assert!((b.main[1] - 0.25).abs() < 1e-12);
        b.add(1, -1.0, true);
        assert_eq!((b.main[1], b.sub[1][3]), (0.0, 0.0));
    }
}
