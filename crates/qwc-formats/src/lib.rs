//! Readers for the data formats of *Harry Potter: Quidditch World Cup* (PC, 2003).
//!
//! Each module is a port of the matching Python reference tool in `tools/`; the Python tools were verified
//! against the real data and remain the reference. Layouts are documented in `docs/RE_NOTES.md`.

pub mod audio;
pub mod btf;
pub mod ccd;
pub mod cs_script;
pub mod dump;
pub mod ea_audio;
pub mod exe;
pub mod fas;
pub mod ffn;
pub mod flow;
pub mod formotion;
pub mod fpm;
pub mod fsh;
pub mod heightmap;
pub mod matchdesc;
pub mod mpf;
pub mod pop;
pub mod qsp;
pub mod refpack;
pub mod skl;
pub mod sound_map;
pub mod speech;
pub mod sts;
pub mod text_defs;
pub mod tqi;
mod util;
pub mod vso;
pub mod wpe;

pub use util::{crc32, Error, Result};
