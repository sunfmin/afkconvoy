//! Rule 2 — the commit pushed to the base branch is one the gate passed on:
//! a green run of the command configured now, on that commit's tree, no more
//! than a day old.
//!
//! The evidence is about a *value* (which tree, which command, when), so a
//! typestate cannot carry it; an index on the evidence can.

use flux_rs::attrs::*;

pub const RECORD_TTL: u64 = 86_400;

#[derive(Debug)]
#[refined_by(tree: int)]
pub struct Commit {
    sha: u64,
    #[field(u64[tree])]
    tree: u64,
}

/// One green gate run. Built only by `run_gate`, which saw the exit code, and
/// by `read_record`, which read what such a run left on the remote.
#[derive(Debug)]
#[refined_by(tree: int, command: int, at: int)]
pub struct GreenRun {
    #[field(u64[tree])]
    tree: u64,
    #[field(u64[command])]
    command: u64,
    #[field(u64[at])]
    at: u64,
}

#[derive(Debug)]
pub enum Outcome {
    Landed { source: &'static str },
    GateRed,
}

impl GreenRun {
    #[spec(fn(&Self[@g]) -> u64[g.at])]
    pub fn at(&self) -> u64 {
        self.at
    }
}

impl Commit {
    #[spec(fn(sha: u64, tree: u64) -> Commit[tree])]
    pub fn new(sha: u64, tree: u64) -> Commit {
        Commit { sha, tree }
    }
}

/// Runs the gate on a commit's tree. Only a green run yields evidence.
#[spec(fn(&Commit[@t], command: u64, now: u64, passes: bool) -> Option<GreenRun[t, command, now]>)]
pub fn run_gate(commit: &Commit, command: u64, now: u64, passes: bool) -> Option<GreenRun> {
    println!("   gate     tree {} with command {command}: {}", commit.tree, if passes { "green" } else { "red" });
    if passes { Some(GreenRun { tree: commit.tree, command, at: now }) } else { None }
}

/// The record on the remote for this tree and command, if there is one. Its
/// age is whatever it is: the caller must look before believing it.
#[spec(fn(&Commit[@t], command: u64, recorded_at: Option<u64>) -> Option<GreenRun{g: g.tree == t && g.command == command}>)]
pub fn read_record(commit: &Commit, command: u64, recorded_at: Option<u64>) -> Option<GreenRun> {
    match recorded_at {
        Some(at) => Some(GreenRun { tree: commit.tree, command, at }),
        None => None,
    }
}

/// The fast-forward push of the train's tip to the base branch.
#[spec(fn(&Commit[@t], &GreenRun[@g], command: u64, now: u64)
    requires g.tree == t && g.command == command && g.at <= now && now - g.at <= RECORD_TTL)]
pub fn push_to_base(commit: &Commit, run: &GreenRun, _command: u64, now: u64) {
    println!("   push     {:x} (gated at {}, now {now})", commit.sha, run.at);
}

/// The train worker's landing: trust a record that stands, else gate.
pub fn land_train(tip: &Commit, command: u64, now: u64, recorded_at: Option<u64>, passes: bool) -> Outcome {
    if let Some(record) = read_record(tip, command, recorded_at) {
        if record.at <= now && now - record.at <= RECORD_TTL {
            push_to_base(tip, &record, command, now);
            return Outcome::Landed { source: "recorded" };
        }
        println!("   record   too old, or from the future: not believed");
    }
    match run_gate(tip, command, now, passes) {
        Some(run) => {
            push_to_base(tip, &run, command, now);
            Outcome::Landed { source: "run" }
        }
        None => Outcome::GateRed,
    }
}
