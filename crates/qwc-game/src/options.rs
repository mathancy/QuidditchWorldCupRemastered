//! The in-game sound options kept with the player's settings (`qwc_settings::Sound`). Not the original's: the
//! original saved them and never applied them.
//!
//! **[T]** The options screen (`TEAMTENTOPTIONSAUDIO.VSO`) and the match's pause menu (`MATCH.VSO`) keep three
//! DataStore floats, 0 to 1 (defaults 1, `InitialiseOptions`), and post `General_AudioVolumeEvent(channel, value)`
//! when a slider moves (`OnSetSliderValue`): channel 1 the commentary and speech, 2 the effects, 3 the music. The
//! Autosave scripts' `LoadData` posts all three after a save is loaded or made. The retail handler of the event
//! (router `0x1043938`, the mixer's listener `0x4b30e0`) is an empty function, and the floats are only kept in the
//! save (`SAVE_KEYS` 25-27).
//!
//! **[I]** The remaster's rule, so that the launcher's Settings page and the game's own options agree: one channel in
//! a tick is the player's slider, kept in `settings.ron` and heard at once; all three in a tick is a save loading,
//! whose values give way to the settings' (written back into the DataStore, so the sliders show them and the next
//! save keeps them).

use qwc_feck::frontend::{EngineEvent, FrontEnd};
use qwc_feck::memory::Memory;
use qwc_settings::Sound;

/// The channels and their DataStore keys: `AudioSpeechVolume`, `AudioEffectsVolume`, `AudioMusicVolume`.
pub const CHANNELS: [(u8, u32); 3] = [(1, 0xc1a1827b), (2, 0xb152b5c4), (3, 0x4d804136)];

/// The level of `channel` in the settings.
fn level(sound: &mut Sound, channel: u8) -> Option<&mut f32> {
    match channel {
        1 => Some(&mut sound.speech),
        2 => Some(&mut sound.effects),
        3 => Some(&mut sound.music),
        _ => None,
    }
}

/// The volume events among `events`: (channel, level).
pub fn volume_events(events: &[EngineEvent]) -> Vec<(u8, f32)> {
    events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::AudioVolume { channel, value } => Some((*channel, f32::from_bits(*value))),
            _ => None,
        })
        .collect()
}

/// Write the settings' levels into the DataStore's three floats.
pub fn push(sound: &Sound, fe: &FrontEnd, mem: &mut Memory) {
    let mut s = *sound;
    for (channel, key) in CHANNELS {
        if let Some(v) = level(&mut s, channel) {
            fe.set_datastore_u32(mem, key, v.clamp(0.0, 1.0).to_bits());
        }
    }
}

/// A tick's volume events: the player's slider changes `sound` (answered true: to be kept); a save loading gets the
/// settings' levels written back into the DataStore (answered false).
pub fn take(events: &[(u8, f32)], sound: &mut Sound, fe: &FrontEnd, mem: &mut Memory) -> bool {
    let all = CHANNELS.iter().all(|(c, _)| events.iter().any(|(e, _)| e == c));
    if all {
        push(sound, fe, mem);
        return false;
    }
    let mut changed = false;
    for &(channel, value) in events {
        if let Some(l) = level(sound, channel) {
            let v = if value.is_finite() { value.clamp(0.0, 1.0) } else { 1.0 };
            changed |= *l != v;
            *l = v;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slider_changes_its_level_and_a_load_does_not() {
        let fe = FrontEnd::default();
        let mut mem = Memory::default();
        let mut sound = Sound { master: 0.8, music: 0.5, effects: 0.5, speech: 0.5 };
        assert!(take(&[(3, 0.25)], &mut sound, &fe, &mut mem));
        assert_eq!((sound.music, sound.effects, sound.master), (0.25, 0.5, 0.8));
        assert!(!take(&[(3, 0.25)], &mut sound, &fe, &mut mem), "the same level again changes nothing");
        // a save loading posts all three: the settings stay
        assert!(!take(&[(2, 1.0), (3, 1.0), (1, 1.0)], &mut sound, &fe, &mut mem));
        assert_eq!((sound.music, sound.effects, sound.speech), (0.25, 0.5, 0.5));
        assert!(take(&[(1, 7.0)], &mut sound, &fe, &mut mem));
        assert_eq!(sound.speech, 1.0, "kept in range");
        assert!(!take(&[(9, 0.0)], &mut sound, &fe, &mut mem), "an unknown channel is ignored");
    }

    #[test]
    fn events_are_floats() {
        let events = [
            EngineEvent::AudioVolume { channel: 2, value: 0.5f32.to_bits() },
            EngineEvent::DeInitMatch,
            EngineEvent::AudioVolume { channel: 1, value: 1.0f32.to_bits() },
        ];
        assert_eq!(volume_events(&events), [(2, 0.5), (1, 1.0)]);
    }
}
