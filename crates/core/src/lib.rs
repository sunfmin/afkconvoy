//! afkconvoy's decisions: pure functions of what the effect layer read, with
//! the clock passed in. Nothing here reads or writes anything (docs/adr/0007).

// Flux's refinements of the standard library — what gives a `Vec` its length.
// Naming the crates is what brings them into the check; under a plain build
// both are empty.
extern crate flux_alloc;
extern crate flux_core;

pub mod account;
pub mod accounts;
pub mod admission;
pub mod apart;
pub mod before;
pub mod blocked;
pub mod board;
pub mod candidate;
pub mod casting;
pub mod checkouts;
pub mod claims;
pub mod closing;
pub mod config;
pub mod cut;
pub mod distill;
pub mod ending;
pub mod exclusive;
pub mod gate;
pub mod graph;
pub mod guard;
pub mod harness;
pub mod host_runner;
pub mod host_watch;
pub mod hot;
pub mod intake;
pub mod landing;
pub mod landing_worker;
pub mod launch;
pub mod layout;
pub mod lease;
pub mod left;
pub mod legacy;
pub mod line;
pub mod lock;
pub mod patrol;
pub mod plan;
pub mod position;
pub mod prompt;
pub mod proxy;
pub mod question;
pub use afkconvoy_gate::record;
pub mod recovery;
#[cfg(flux)]
mod rejected;
pub mod resources;
pub mod role;
pub mod roster;
pub mod selection;
pub mod settle;
pub mod slow_gate;
pub mod stage_record;
pub mod stopped;
pub mod takeover;
pub mod team;
pub mod tick;
pub mod tools;
pub mod tracker;
pub mod transition;
pub mod turn;
pub mod unworked;
pub mod verdict;
pub mod waiting;
pub mod wake;
pub mod worker;
pub mod worker_health;
