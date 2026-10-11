//! Misuses Flux must reject. Each is wrong on purpose; `should_fail` makes
//! verification fail if Flux ever accepts one.
#![allow(dead_code)]

use flux_rs::attrs::*;

use crate::{
    escalate::{Escalation, Released},
    landing::{Commit, RECORD_TTL, push_to_base, read_record, run_gate},
    slots::free_slots,
};

// --- rule 1 ---

#[should_fail]
fn released_before_the_relabel(mut e: Escalation, reason: &str) -> Released {
    if !e.board() {
        e.post_board();
    }
    if !e.commented() {
        e.comment(reason);
    }
    e.release()
}

#[should_fail]
fn released_as_found(e: Escalation) -> Released {
    e.release()
}

#[should_fail]
fn progress_assumed_instead_of_read() -> Released {
    Escalation::read(7, true, true, true).release()
}

#[should_fail]
fn relabelled_before_the_comment(mut e: Escalation) {
    e.post_board();
    e.relabel();
}

#[should_fail]
fn a_step_skipped_on_one_path(mut e: Escalation, reason: &str, quiet: bool) -> Released {
    e.post_board();
    if !quiet {
        e.comment(reason);
    }
    e.relabel();
    e.release()
}

// --- rule 2 ---

#[should_fail]
fn pushed_on_another_commits_run(a: &Commit, b: &Commit, command: u64, now: u64) {
    if let Some(run) = run_gate(a, command, now, true) {
        push_to_base(b, &run, command, now);
    }
}

#[should_fail]
fn pushed_on_a_run_of_another_command(tip: &Commit, old: u64, configured: u64, now: u64) {
    if let Some(run) = run_gate(tip, old, now, true) {
        push_to_base(tip, &run, configured, now);
    }
}

#[should_fail]
fn pushed_on_a_record_of_any_age(tip: &Commit, command: u64, now: u64, recorded_at: Option<u64>) {
    if let Some(record) = read_record(tip, command, recorded_at) {
        push_to_base(tip, &record, command, now);
    }
}

#[should_fail]
fn pushed_on_a_record_from_the_future(tip: &Commit, command: u64, now: u64, recorded_at: Option<u64>) {
    if let Some(record) = read_record(tip, command, recorded_at) {
        if record.at() >= now && record.at() - now <= RECORD_TTL {
            push_to_base(tip, &record, command, now);
        }
    }
}

// --- rule 3 ---

#[should_fail]
#[spec(fn(concurrency: usize, held: usize) -> usize{after: after <= concurrency || after == held})]
fn a_claim_taken_without_a_free_slot(concurrency: usize, held: usize) -> usize {
    let _ = free_slots(concurrency, held);
    held + 1
}

#[should_fail]
#[spec(fn(concurrency: usize, held: usize) -> usize[concurrency - held])]
fn free_slots_gone_negative(concurrency: usize, held: usize) -> usize {
    concurrency - held
}
