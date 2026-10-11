//! What Flux does NOT catch. Every function here is wrong, and verifies.
#![allow(dead_code)]

use crate::escalate::Escalation;

/// A step that never happens. Flux checks what a call requires, not that a
/// call is made: an escalation dropped half way leaves its claim held.
/// What does catch it is ordinary Rust — a transition whose return type is
/// `Released` has no other way to end.
fn abandoned_half_way(mut e: Escalation, reason: &str) {
    e.post_board();
    e.comment(reason);
}

// The other limit has no function to show it: the order is proved from what
// `Escalation::read` and `read_record` return. That they return what GitHub
// holds is the effect layer's word, which no type checks.
