//! afkconvoy's decisions: pure functions of what the effect layer read, with
//! the clock passed in. Nothing here reads or writes anything (docs/adr/0007).

pub mod claims;
pub mod config;
pub mod lease;
pub mod record;
#[cfg(flux)]
mod rejected;
