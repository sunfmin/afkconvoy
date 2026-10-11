# Refinement types on a pure core; effects through git, gh and orca

afkconvoy keeps afk-fleet's two layers: a pure decision core with no I/O and the clock passed in, and a thin effect layer. The core is checked with Flux (Liquid Rust) refinement types — both its numeric invariants (free slots never negative, claims held after a tick bounded by what was held before, freshness within the lease and the clock-skew tolerance, sleep at most half the lease) and the order of a claim's transitions (the release is the last step of every terminal transition; only a commit the gate passed on is pushed to the base branch).

The effect layer runs `git`, `gh` and `orca` as subprocesses, as afk-fleet does, rather than calling the GitHub API directly: no token is handled, behaviour maps one-to-one onto afk-fleet's, and its test approach carries over (stand-in `gh` and `orca` on PATH, a bare repo as the remote).

## Consequences

- Flux needs a nightly toolchain; it runs in the gate, and the released binary is built on stable.
- Whether Flux can express the transition-order rules is unproven. It is to be tried on one rule before the design depends on it.
- The Homebrew formula depends on `git` and `gh`; orca is the user's to install.
