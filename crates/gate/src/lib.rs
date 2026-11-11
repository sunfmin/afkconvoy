//! The gate and the exclusive queue (docs/adr/0059), with no fleet concept
//! in their API: what a repo declares of its gate, the evidence a gate run
//! leaves on record, and the queue in which a host runs its heavy commands
//! one lane at a time. afkconvoy depends on it, and so may another program,
//! by a git dependency pinned to a revision of this repo.

// Flux's refinements of the standard library — what gives a `Vec` its length.
// Naming the crates is what brings them into the check; under a plain build
// both are empty.
extern crate flux_alloc;
extern crate flux_core;

pub mod gate;
pub mod layout;
pub mod record;
pub mod slow_gate;
