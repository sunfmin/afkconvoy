//! Candidates, candidate chains, rounds and landing (docs/adr/0060), with
//! the forge behind an interface and no fleet concept in their API: a
//! change's immutable merge onto its predecessor, the chain that orders the
//! candidates and cancels a failed or invalid suffix, the round that
//! slow-gates once at its tip, and the ordered fast-forward that lands it,
//! all on one record the remote holds. afkconvoy depends on it, and so may
//! another program, by a git dependency pinned to a revision of this repo.

// Flux's refinements of the standard library — what gives a `Vec` its length.
// Naming the crates is what brings them into the check; under a plain build
// both are empty.
extern crate flux_alloc;
extern crate flux_core;

pub mod chain;
pub mod forge;
pub mod land;
pub mod lease;
pub mod turn;
