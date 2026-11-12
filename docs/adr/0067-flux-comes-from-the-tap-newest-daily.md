---
status: accepted
---

# Flux comes from the tap's newest daily toolchain, pinned nowhere

Formerly the control repo's. How the toolchain is built and published is sunfmin/flux-toolchain's ADR (sunfmin/control#771); this is afkconvoy's side.

Flux checking (ADR-0007) needs `cargo-flux`, `flux`, the Flux sysroot, `fixpoint` and `z3`. Built from source on each host — liquid-fixpoint through stack, which first downloads a GHC, and cargo-flux too — they took 732 seconds on Studio64 on 2026-11-12, again on every new machine and on every change of pin. Felix, that day in the control repo's session: 「每次 liquid fix point 都要自己编译是不对的」「这个不要盯了，每次都用最新的。出问题的话再改」「每天把这个工具链所需要的所有的东西都编译成 homebrew 我的 tap 下发布，我以后所有的项目都在这 tap 下安装。安装最新版。每天如果有变化的话，就一个版本」.

So afkconvoy installs the whole Flux toolchain from Homebrew's `sunfmin/tap/flux-toolchain`, the newest the tap has: `./verify.sh install` installs or upgrades it, and `./verify.sh --toolchain` — the gate need `docs/agents/afkconvoy.md` declares — fails a host that lacks it or is behind the tap. Nothing here pins a version of any of its tools or builds one. Where the newest breaks something, it is fixed then.

The toolchain is built daily from the newest of everything — Flux's `main`, liquid-fixpoint's `nightly`, z3's latest release — and published as one version a day at most, named by its UTC date, only when its inputs changed; a public repo's Actions cost nothing, and ADR-0063 is about private repos alone.

Sources: sunfmin/control#735 (this repo's side), sunfmin/control#732 (a pin with upstream's fixpoint binaries, the step before it).

## Considered options

- **A pin in each project, with upstream's fixpoint binary downloaded** (sunfmin/control#732). It removes the GHC build, but cargo-flux is still built on every machine, and every project keeps its own pin. Refused.

## Consequences

- `flux-rs` and its crates are still a git dependency by `rev` in `Cargo.toml`, while the tap's Flux changes daily. Where the two disagree enough to break the gate, the `rev` moves to the Flux commit the formula's caveats name.
- Flux's driver links the Rust nightly it was built with; the `cargo-flux` and `flux` wrappers install that nightly through rustup on first run, so every host that checks Flux needs rustup.
- The toolchain is built for Apple silicon macOS alone.
