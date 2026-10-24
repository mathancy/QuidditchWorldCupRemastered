//! The front end's session without a GPU, shared by the game (`qwc-game`) and the HUD editor's Screens mode:
//!
//! - [`session`]: every layer script loaded and linked, the strings and the DataStore built, and the screen manager
//!   over `game.flo` and `Screens.lst` started at a node ([`session::Flow`]), with the load-set table and the cards'
//!   numbers from a decrypted `QWC.exe` when one is found;
//! - [`disc`]: the textures and fonts the scripts see ([`disc::Disc`]): the disc's through the resource index and
//!   the load sets held, or the remastered PNGs standing in for them, and the vector fonts drawing the bitmap ones;
//! - [`layouts`]: the remaster's changes to where the screens' objects are drawn, one RON file per layer, in the
//!   resource packs (none without a pack on);
//! - [`crops`]: pictures the layouts cut out of the disc's textures, kept as recipes, never as pixels.
//!
//! What draws the lists (the overlay, the world behind) is the host's.

pub mod crops;
pub mod disc;
pub mod layouts;
pub mod names;
pub mod session;

pub use disc::{Disc, Shown};
pub use layouts::{LayerLayout, Layouts, ObjectLayout};
pub use session::{language_tables, start_flow, start_flow_in, Flow};
