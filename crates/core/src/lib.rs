//! afkconvoy's decisions: pure functions of what the effect layer read, with
//! the clock passed in. Nothing here reads or writes anything (docs/adr/0007).

pub mod claims;
pub mod closing;
pub mod config;
pub mod gate;
pub mod lease;
pub mod plan;
pub mod prompt;
pub mod record;
#[cfg(flux)]
mod rejected;
pub mod tick;
pub mod train;
pub mod verdict;
pub mod worker;
