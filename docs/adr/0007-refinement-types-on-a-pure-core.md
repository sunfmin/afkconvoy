# Refinement types on a pure core; effects through git, gh and orca

afkconvoy keeps afk-fleet's two layers: a pure decision core with no I/O and the clock passed in, and a thin effect layer. The core is checked with Flux (Liquid Rust) refinement types — both its numeric invariants (free slots never negative, claims held after a tick bounded by what was held before, freshness within the lease and the clock-skew tolerance, sleep at most half the lease) and the order of a claim's transitions (the release is the last step of every terminal transition; only a commit the gate passed on is pushed to the base branch).

The effect layer runs `git`, `gh` and `orca` as subprocesses, as afk-fleet does, rather than calling the GitHub API directly: no token is handled, behaviour maps one-to-one onto afk-fleet's, and its test approach carries over (stand-in `gh` and `orca` on PATH, a bare repo as the remote).

## Consequences

- Flux needs a nightly toolchain; it runs in the gate, and the released binary is built on stable.
- Flux can express the transition-order rules. A prototype (branch `prototype/flux-transitions`, Flux rev `b2f3e31`) proved an escalation's order — including one resumed from wherever it was cut — and the gate-evidence rule, and refused eleven misuses.
- The steps a transition has taken are indices on a runtime value, not type parameters: an interrupted transition is finished by a later tick that reads how far it got, which a typestate cannot follow.
- Flux proves what a call requires, not that a call is made. That a transition runs to its release is held by ordinary Rust: the transition returns a value only the release produces.
- The proof starts at what the effect layer reports. That a read returns what GitHub holds is not proved.
- Flux knows the length of a `Vec` only through its own `flux-alloc` and `flux-core` crates, so the core depends on both at the revision of `flux-rs`: without them an index into a `Vec` goes unchecked. Both are empty under a plain build.
- What a refined struct is indexed by has to be the value of one of its fields: a stage holds the number of stages its workflow has, which Flux proves is that number.
- What Flux has to infer of a refined value, it carries through every loop that follows in the same function. A long function makes and joins none: each is made in a function of its own, under a spec.
- A loop needs its invariant written as a qualifier, and a function whose result an invariant mentions must return a named value, not a formula.
- `git`, `gh` and orca are the user's to install; the Homebrew formula depends on none of them.
