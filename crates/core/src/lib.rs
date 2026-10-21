//! afkconvoy's decisions: pure functions of what the effect layer read, with
//! the clock passed in. Nothing here reads or writes anything (docs/adr/0007).

// Flux's refinements of the standard library — what gives a `Vec` its length.
// Naming the crates is what brings them into the check; under a plain build
// both are empty.
extern crate flux_alloc;
extern crate flux_core;

pub mod abandon;
pub mod admission;
pub mod before;
pub mod blocked;
pub mod board;
pub mod claims;
pub mod closing;
pub mod config;
pub mod exclusive;
pub mod gate;
pub mod graph;
pub mod guard;
pub mod harness;
pub mod host_inspection;
pub mod host_sleep;
pub mod hot;
pub mod inspection;
pub mod intake;
pub mod joining;
pub mod launch;
pub mod layout;
pub mod lease;
pub mod left;
pub mod line;
pub mod lock;
pub mod patrol;
pub mod plan;
pub mod position;
pub mod precedent;
pub mod prompt;
pub mod record;
pub mod recovery;
#[cfg(flux)]
mod rejected;
pub mod role;
pub mod settle;
pub mod slow_gate;
pub mod stage_record;
pub mod stock;
pub mod stopped;
pub mod takeover;
pub mod team;
pub mod tick;
pub mod tools;
pub mod train;
pub mod transition;
pub mod verdict;
pub mod waiting;
pub mod wake;
pub mod worker;
