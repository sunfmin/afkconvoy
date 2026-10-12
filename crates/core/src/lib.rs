//! afkconvoy's decisions: pure functions of what the effect layer read, with
//! the clock passed in. Nothing here reads or writes anything (docs/adr/0007).

pub mod abandon;
pub mod blocked;
pub mod board;
pub mod claims;
pub mod closing;
pub mod config;
pub mod gate;
pub mod joining;
pub mod launch;
pub mod layout;
pub mod lease;
pub mod plan;
pub mod prompt;
pub mod record;
pub mod recovery;
#[cfg(flux)]
mod rejected;
pub mod settle;
pub mod stopped;
pub mod takeover;
pub mod team;
pub mod tick;
pub mod tools;
pub mod train;
pub mod transition;
pub mod verdict;
pub mod wake;
pub mod worker;
