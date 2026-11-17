---
status: accepted
---

# Refinement types on a pure core; effects through git, gh and orca

afkconvoy keeps afk-fleet's two layers: a pure decision core with no I/O and the clock passed in, and a thin effect layer. The core is checked with Flux (Liquid Rust) refinement types — both its numeric invariants (free slots never negative, claims held after a tick bounded by what was held before, freshness within the lease and the clock-skew tolerance, sleep at most half the lease) and the order of a claim's transitions (the release is the last step of every terminal transition; only a commit the gate passed on is pushed to the base branch).

The effect layer runs `git`, `gh` and `orca` as subprocesses rather than calling the GitHub API directly: no token is handled, and tests stand in a `gh` and an `orca` on PATH with a bare repo as the remote.

## Flux comes from the tap's newest daily toolchain, pinned nowhere

Flux checking needs `cargo-flux`, `flux`, the Flux sysroot, `fixpoint` and `z3`. Built from source on each host — liquid-fixpoint downloads and builds a GHC first — they took many minutes, again on every new machine and every change of pin. So afkconvoy installs the whole toolchain from Homebrew's `sunfmin/tap/flux-toolchain`, the newest the tap has: `./verify.sh install` installs or upgrades it, and `./verify.sh --toolchain`, the gate need `docs/agents/afkconvoy.md` declares, fails a host that lacks it or is behind the tap. Nothing here pins a version of any of its tools or builds one; where the newest breaks something, it is fixed then.

The tap builds the toolchain daily from the newest of everything — Flux's `main`, liquid-fixpoint's `nightly`, z3's latest release — and publishes at most one version a day, named by its UTC date, only when its inputs changed. How it is built is sunfmin/flux-toolchain's own decision.

Rejected: a pin in each project with upstream's fixpoint binary downloaded. It removes the GHC build, but cargo-flux is still built on every machine, and every project keeps its own pin.

## Consequences

- Flux needs a nightly toolchain; it runs in the gate, and the released binary is built on stable. Flux's driver links the nightly it was built with, which the `cargo-flux` and `flux` wrappers install through rustup on first run, so every host that checks Flux needs rustup. The toolchain is built for Apple silicon macOS alone.
- `flux-rs`, `flux-alloc` and `flux-core` are a git dependency by `rev` in `Cargo.toml`, while the tap's Flux changes daily. Where the two disagree enough to break the gate, the `rev` moves to the Flux commit the formula names.
- The steps a transition has taken are indices on a runtime value, not type parameters: an interrupted transition is finished by a later tick that reads how far it got, which a typestate cannot follow.
- Flux proves what a call requires, not that a call is made. That a transition runs to its release is held by ordinary Rust: the transition returns a value only the release produces.
- The proof starts at what the effect layer reports. That a read returns what GitHub holds is not proved.
- What Flux infers of a refined value it carries through every loop that follows in the same function, so a long function makes and joins none: each is made in a function of its own, under a spec.
- `git`, `gh` and orca are the user's to install; the Homebrew formula depends on none of them.
