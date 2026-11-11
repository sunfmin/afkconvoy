//! What afkconvoy knows of each harness it starts sessions of (docs/adr/0013,
//! docs/adr/0058), kept apart so another program can depend on it: nothing in
//! its API is a fleet's — no team, role, stage, issue, claim or patrol.

// Flux's refinements of the standard library — what gives a `Vec` its length.
// Naming the crates is what brings them into the check; under a plain build
// both are empty.
extern crate flux_alloc;
extern crate flux_core;

pub mod line;
