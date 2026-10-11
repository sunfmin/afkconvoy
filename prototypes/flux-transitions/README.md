# PROTOTYPE: Flux on afkconvoy's transition-order rules

Throwaway. It answers the question ADR-0007 left open: can Flux (Liquid Rust) express the *order* of a claim's transitions, not only numeric bounds?

**Verdict: yes**, for the two order rules tried and the one numeric rule, with two things to know (below).

    ./verify.sh    # Flux: 3 rules proved, 11 misuses in src/rejected.rs refused (0.7 s)
    cargo run      # the same code on stable Rust, printing the state after each step

Needs `cargo-flux`, `fixpoint` and `z3`; Flux is pinned in `Cargo.toml` (rev `b2f3e31`).

| Rule | File | How it is expressed |
|---|---|---|
| An escalation goes board → comment → relabel → release, and may be resumed from wherever it was cut | `src/escalate.rs` | The steps taken are three `bool` indices on the value. Each step `requires` the one before and `ensures` its own; `release` takes the value by move and requires all three. `escalate` reads the flags at run time and Flux follows each branch. |
| Only a commit the gate passed on is pushed to the base | `src/landing.rs` | A `GreenRun` is indexed by the tree, the command and the time. The push `requires` that they match the commit, the configured command, and a day's age. |
| A tick takes claims only into free slots | `src/slots.rs` | A postcondition on the count, proved through a loop. |

## What to know before depending on it

- **A loop needs its invariant spelled out, and a result that is only a formula cannot be named in one.** `take`'s loop was not proved until two `qualifier!` lines were added, and until `free_slots` returned `usize{free: …}` (a named value) instead of `usize[if … { 0 } else { … }]` (a formula). The errors do not say this; it took four runs to find.
- **Flux proves what a call requires, not that a call is made.** An escalation dropped half way verifies (`src/limits.rs`). What prevents that is ordinary Rust: a transition whose return type is `Released` has no other way to end.
- **The proof starts at what the effect layer reports.** Flux refuses progress that was assumed rather than read (`progress_assumed_instead_of_read`), but that `read` returns what GitHub really holds is nobody's proof.
- **Forged evidence is stopped by field privacy, not by Flux**: `GreenRun` and `Escalation` can only be built inside their modules.

Removing a step from the real `escalate`, or the age check from the real `land_train`, is refused by Flux — tried by hand, both.
