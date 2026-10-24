//! Gameplay rules of the original game, rebuilt from the traced code in `docs/GAMEPLAY.md`, for a deterministic
//! simulation at the original 24 Hz tick. No rendering or audio.
//!
//! - [`console`]: the original console registry and `.cli` scripts, which carry nearly all gameplay tuning.
//! - [`rng`]: the C runtime `rand()` the game draws from, so decisions replay in the same order.
//! - [`fmath`]: the sines, cosines and exponentials from `libm`, so every platform computes the same bits.
//! - [`passing`]: the AI's possession and pass decisions, aiming and the pass cone.
//! - [`shooting`]: the hoops, the goal area, the AI's shot wind-up and hoop choice, shot flight and contact radii.
//! - [`tackle`]: tackles, lunges, the stun and recharge, and the AI defender's press.
//! - [`formation`]: the Formotions that move the chasers off the ball.
//! - [`keeper`]: the keeper's guarding off the ball and the mover's Bezier step.
//! - [`pitch`]: the floor from the height maps, the end-zone clamp and the cruising height.
//! - [`seeker`]: the Seeker game's catch test and Snitch motion.
//! - [`walls`]: the pitch's collision volumes, the movers' commit against them and the pass target's sweep.
//! - [`world`]: a match as a 24 Hz tick loop of possession, passing, shots, saves and goals, with placeholders
//!   where the code is not traced yet.
//!
//! Most player movement, dodges and the other balls are not ported yet (see [`world`]).

pub mod approach;
pub mod bludger;
pub mod challenge;
pub mod console;
pub mod evade;
pub mod fmath;
pub mod formation;
pub mod handling;
pub mod human;
pub mod keeper;
pub mod passing;
pub mod pitch;
pub mod powerbar;
pub mod race;
mod registry;
pub mod rng;
pub mod seeker;
pub mod shooting;
pub mod spline;
pub mod tackle;
mod vec3;
pub mod walls;
pub mod world;

pub use console::Console;
pub use rng::Rand;
pub use world::Match;
