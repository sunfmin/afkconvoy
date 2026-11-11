//! Ages and the claim lease: every "how old is this stamp" the fleet acts on.
//! The rule is the landing crate's (`afkconvoy_landing::lease`), whose
//! candidate chain tells a live owner from a dead one by it too; the fleet
//! reads its claims' and instances' heartbeats by the same.

pub use afkconvoy_landing::lease::{
    heartbeat_due, is_stale, stamp_age, stamp_due, stamp_forgotten, stamp_stale,
};

/// A fleet instance is live while its heartbeat is this fresh, in seconds:
/// half an hour (`afkconvoy_landing::lease::LEASE`).
pub const LEASE: u64 = afkconvoy_landing::lease::LEASE;
/// How far ahead of the reader a stamp may be and still count as "just now".
pub const SKEW: u64 = afkconvoy_landing::lease::SKEW;
/// An owner refreshes its heartbeat once it is older than this.
pub const BEAT_EVERY: u64 = afkconvoy_landing::lease::BEAT_EVERY;
/// A heartbeat that holds nothing is forgotten once it is this old.
pub const FORGOTTEN: u64 = afkconvoy_landing::lease::FORGOTTEN;
