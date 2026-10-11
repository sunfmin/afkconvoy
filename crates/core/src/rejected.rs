//! Misuses Flux must refuse. Each is the smallest program that breaks one rule
//! the crate states; `#[should_fail]` makes the check fail if Flux ever accepts
//! it. Compiled only under Flux.
#![allow(dead_code)]

use flux_rs::attrs::*;

use crate::config::Config;

/// A config's concurrency is never zero, so nothing may claim to hold one that is.
#[should_fail]
#[spec(fn(&Config) -> usize[0])]
fn a_config_with_no_slots(config: &Config) -> usize {
    config.concurrency()
}
