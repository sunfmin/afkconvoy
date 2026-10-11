//! PROTOTYPE — throwaway. Can Flux express afkconvoy's transition-order rules?
//!
//! `./verify.sh` checks the three rules below, and that every misuse in
//! `rejected.rs` is refused. `cargo run` walks the same code and prints the
//! state after each step.

mod escalate;
mod landing;
#[cfg(flux)]
mod limits;
#[cfg(flux)]
mod rejected;
mod slots;

fn main() {
    println!("== rule 1: an escalation releases last, from wherever it was cut ==");
    for (board, commented, relabelled) in [
        (false, false, false),
        (true, true, false),
        (true, true, true),
    ] {
        println!("-- found on GitHub: board={board} commented={commented} relabelled={relabelled}");
        let e = escalate::Escalation::read(7, board, commented, relabelled);
        let released = escalate::escalate(e, "silent after its nudge");
        println!("   => {released:?}");
    }

    println!("== rule 2: only a commit the gate passed on reaches the base ==");
    for (recorded_at, gate_passes) in [(Some(80_000), true), (Some(1), true), (None, true), (None, false)] {
        println!("-- record at {recorded_at:?}, a run would be green: {gate_passes}");
        let tip = landing::Commit::new(0xabc, 42);
        let outcome = landing::land_train(&tip, 9, 90_000, recorded_at, gate_passes);
        println!("   => {outcome:?}");
    }

    println!("== rule 3: a tick takes claims only into free slots ==");
    for (concurrency, held, candidates) in [(3, 1, 5), (3, 5, 2), (3, 3, 1), (3, 0, 2)] {
        let after = slots::take(concurrency, held, candidates);
        println!("-- concurrency={concurrency} held={held} candidates={candidates} => held={after}");
    }
}
