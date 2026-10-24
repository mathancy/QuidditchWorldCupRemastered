//! Asset loading for the remaster: a virtual file system over the original game data ([`vfs`]) and loaders that
//! turn decoded formats into engine-ready data ([`model`], [`texture`]).

pub mod anim;
pub mod compress;
pub mod frontend;
pub mod glb;
pub mod gltf_out;
pub mod groper;
pub mod model;
pub mod remaster;
pub mod texture;
pub mod vfs;

pub use model::Model;
pub use texture::{Texture, TextureIndex};
pub use vfs::{Bytes, Vfs};
