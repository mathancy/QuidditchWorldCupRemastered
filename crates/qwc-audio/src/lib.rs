//! Audio for the remaster: the game's sound data ([`library`]), a software mixer ([`mixer`]), interactive music
//! ([`music`]) and, with the `playback` feature, output to the system's audio device ([`output`]).
//!
//! Decoding is `qwc_formats::ea_audio`. Sounds are decoded on the game thread and handed to the mixer, which only
//! resamples and mixes, so the device callback never waits on a decoder.

pub mod library;
pub mod mixer;
pub mod music;
#[cfg(feature = "playback")]
pub mod output;

use std::fmt;

pub use library::Library;
pub use mixer::{Bus, Mixer, Params, Sound, VoiceId};
pub use music::{Cue, MusicPlayer, Score};
#[cfg(feature = "playback")]
pub use output::Output;

#[derive(Debug)]
pub enum Error {
    Assets(qwc_assets::vfs::Error),
    Format(String, qwc_formats::Error),
    Missing(String),
    Playback(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Assets(e) => write!(f, "{e}"),
            Error::Format(what, e) => write!(f, "{what}: {e}"),
            Error::Missing(what) => write!(f, "not found: {what}"),
            Error::Playback(e) => write!(f, "audio output: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<qwc_assets::vfs::Error> for Error {
    fn from(e: qwc_assets::vfs::Error) -> Error {
        Error::Assets(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
