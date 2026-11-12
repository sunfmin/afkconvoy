//! What afkconvoy knows of each harness it starts sessions of (docs/adr/0013,
//! docs/adr/0058), kept apart so that another program can depend on it: how
//! each starts, where it reads its rules and skills, how a hook reaches it,
//! where it keeps its session, what its usage lines count, what it says of
//! its account's usage windows and the limits it meets, and which of its
//! screen words mean a quota or a refusal. Nothing in its API is a fleet's —
//! no team, role, stage, issue, claim or patrol: whoever starts a session
//! says what it is started with (`Start`).

// Flux's refinements of the standard library — what gives a `Vec` its length.
// Naming the crates is what brings them into the check; under a plain build
// both are empty.
extern crate flux_alloc;
extern crate flux_core;

pub mod brief;
mod harness;
pub mod line;
pub mod start;
pub mod steps;
pub mod tokens;
pub mod turn;
pub mod window;

pub use harness::*;
pub use start::{DshProfile, Rules, Start, Told};
