# afkconvoy

A fleet of AFK coding agents that works a GitHub backlog. `afkconvoy run` takes the issues a repo marks ready, starts one agent per issue in its own worktree, and lands each finished PR in a merge, gate, record and landing turn — unattended, until you stop it. Everything the fleet knows lives on GitHub and the repo's remote: no process holds state, so a fleet can be stopped and started at will, and several hosts can work one repo.

## Install

macOS, Apple Silicon or Intel:

```sh
brew install sunfmin/tap/afkconvoy
```

The following are yours to set up:

- **git and gh** — have both on PATH; install them if needed.
- **orca** — afkconvoy starts every worker through [orca](https://github.com/stablyai/orca): `brew install --cask stablyai/orca/orca`. Give the tap's name: the cask Homebrew itself calls `orca` is another program. Orca's SSH remote runtime works as it is: on a host the desktop Orca on another machine reaches over SSH, its CLI at `~/.orca/cli/bin/orca` is found with no `orca` on your PATH, and an `orca` on the PATH wins over it. Then start it — `orca open` — and, in each checkout a fleet is to work, give it the repo once: `orca repo add --path "$PWD"`.
- **GitHub** — `gh auth login`, as a user who may push to the repo.
- **Claude Code**, when it is what starts your workers — `curl -fsSL https://claude.ai/install.sh | bash`, then `claude auth login`.
- **pi**, when it is what starts some of your workers — `curl -fsSL https://pi.dev/install.sh | sh`, then `pi`, and `/login` at its prompt.
- **Codex**, when it is what starts some of your workers — `brew install --cask codex`, then `codex login`.
- **Kimi Code**, when it is what starts some of your workers — `curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash`, then `kimi login`.
- **Grok**, when it is what starts some of your workers — `curl -fsSL https://x.ai/cli/install.sh | bash`, then `grok login`.

`run`, `tick` and `plan` check all of this before anything else, and refuse with the line to run for each thing missing, logged out, not running or not given the checkout. afkconvoy installs nothing, starts and adds nothing, and logs nobody in.

`afkconvoy --version` prints the release you have. A build of anything else says what it was built from — `afkconvoy 0.2.0+11.g9f58035` is eleven commits past the newest tag, at commit `9f58035`, and `.dirty` after it is a tree that had changes — and the status board on an issue names the build of the fleet that works it.

**For an AI agent**: a skill that sends an agent to `afkconvoy --help` installs with:

```sh
npx skills add sunfmin/afkconvoy
```

`afkconvoy skill` prints that same file, for an agent the installer does not know: `afkconvoy skill > ~/.claude/skills/afkconvoy/SKILL.md` for a user, or under a project's own `.claude/skills/afkconvoy/`. `afkconvoy skill --help` says where it goes.

## Run `afkconvoy --help`

```sh
afkconvoy --help
```

It answers the index of the tool's own manual, in Markdown, written for a human and for an agent alike: the guide — what it needs, what to run first — then every command and every topic by its line. `afkconvoy <command> --help` says the rest of a command, `afkconvoy help <topic>` prints a topic, and `afkconvoy help all` the whole manual, the only place afkconvoy is explained. Nothing here repeats it.

To read the whole manual, run `afkconvoy help all`; there is no copy of it outside the binary.

## Develop

```sh
./gate.sh fast   # fmt, Flux on the core crate, clippy, the manual, the build, and every test not marked slow
./gate.sh slow   # the tests marked slow, and nothing else
./gate.sh        # both: the gate
```

Pass the fast gate before opening or updating a PR. A test that took 2 seconds or more is marked slow, by name, in [`.config/slow-tests`](.config/slow-tests); a fast gate test still running at 6 seconds is ended, and the gate says to mark it (ADR-0047). Telling the two apart needs nextest: without it only `./gate.sh` runs, every test with `cargo test`.

The gate runs the tests with [cargo-nextest](https://nexte.st) when it is installed (`brew install cargo-nextest`), which takes about a third of the time; without it, it runs them with `cargo test`. Under nextest one test has a limit, 240 seconds, which [`.config/nextest.toml`](.config/nextest.toml) gives: a test that runs past it is ended and fails by name, as `TIMEOUT`. `cargo test` has no limit for one test, and the gate says so when it runs them that way.

The gate ends with the functions of the core crate that Flux took longest to check, and their times.

Flux on the core, harness, gate and landing crates is Homebrew's [`sunfmin/tap/flux-toolchain`](https://github.com/sunfmin/flux-toolchain), the newest the tap has: `brew install sunfmin/tap/flux-toolchain`, or `./verify.sh install`, which installs it or upgrades it. It is built every day, and brings the `cargo-flux`, `fixpoint`, `z3` and Flux sysroot it was built with; its first run adds, through [rustup](https://rustup.rs), the Rust nightly its Flux names. `./verify.sh` refuses to run with it missing or older than the tap's newest, and pins and builds none of them. The `flux-rs` dependency in [`Cargo.toml`](Cargo.toml) is at the Flux revision the tap's was built from while that one checks this tree: `brew info sunfmin/tap/flux-toolchain` names it.

The decisions live in `crates/core` as pure functions checked by [Flux](https://github.com/flux-rs/flux) (`./verify.sh`); `crates/cli` carries them out through `git`, `gh` and `orca` ([ADR-0007](docs/adr/0007-refinement-types-on-a-pure-core.md)). What afkconvoy knows of each harness — how one starts, where it reads rules and skills, how a hook reaches it, where it keeps its session and what its usage lines count — is `crates/harness`, which the core depends on and another program may too, by a git dependency pinned to a revision; its API names nothing of the fleet ([ADR-0058](docs/adr/0058-the-harness-crate-may-be-depended-on-by-another-program.md)). The gate and the exclusive queue — the fast and slow gate a repo declares, recorded gate runs, heavy commands and the host's lanes, and the holder of the queue a host process embeds — are `crates/gate`, on the same terms ([ADR-0059](docs/adr/0059-the-gate-crate-may-be-depended-on-by-another-program.md)). Candidates, candidate chains, rounds and landing, with the forge a program's changes are asked on behind an interface that afkconvoy implements for GitHub, are `crates/landing`, on the same terms again ([ADR-0060](docs/adr/0060-candidates-rounds-and-landing-may-be-depended-on-by-another-program.md)). What the host kit does to a machine — finding its harnesses, giving each the operator's instructions, skills and guards, installing tmux on a Guild machine, and saying where the machine differs from the declaration — is `crates/host-kit`, on the same terms, which `afkconvoy kit` and `doctor` go through ([ADR-0062](docs/adr/0062-the-host-kit-is-a-crate-of-its-own-and-guild-replaces-the-fleet.md)). The daybook — the entry's shape and its lesson, one discussion a day with the lowest numbered winning two created at once, and the lesson index — is `crates/daybook`, on the same terms, with GitHub behind an interface; `afkconvoy daybook` and the weekly distillation go through it ([ADR-0071](docs/adr/0071-the-daybook-crate-may-be-depended-on-by-another-program.md)).

Another program uses the host kit with no afkconvoy installed by depending on it at a revision of this repo, and giving it the declaration:

```toml
[dependencies]
afkconvoy-host-kit = { git = "https://github.com/sunfmin/afkconvoy", rev = "<revision>" }
```

```rust
use afkconvoy_host_kit::{Declared, LoginShell};

let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
let declared = Declared {
    instructions: Some(home.join("dotfiles/claude/CLAUDE.md")),
    skills: Some(home.join("dotfiles/skills.txt")),
    guards: Some(home.join("dotfiles/guards.txt")),
};
let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
// Finds the harnesses, gives each the setup, installs tmux on a Guild machine.
let given = afkconvoy_host_kit::machine(&LoginShell, &home, &declared, now);
// Each harness that still differs from the declaration, and why tmux is missing.
let (differences, no_tmux) = afkconvoy_host_kit::report(&LoginShell, &home, &declared);
```

`LoginShell` runs each line in the user's `$SHELL` as a login shell; a program with a shell of its own implements `Shell`. `give` and `differences` take the harnesses themselves, where a program holds its own list. Guild's `guild install` calls only `tmux::give` and `tmux::missing`: its instances are given none of the operator's agent setup ([ADR-0066](docs/adr/0066-the-host-kit-gives-every-harness-the-agent-setup.md)).

**Cutting a release** is the fleet's: once it goes idle with landed work past the newest `v*` tag, it cuts the next version itself — commits it on a branch of its own from the base tip, gates that commit, tags it, and publishes and installs it on its host, while turns keep landing on the base branch (`afkconvoy cut --help`). `afkconvoy plan` says when the next cut starts. Its steps are the ```` ```yaml cut ```` block of [`docs/agents/afkconvoy.md`](docs/agents/afkconvoy.md), the one place they are written: steps of [`packaging/release.sh`](packaging/release.sh), then Homebrew's. They run on a Mac, and on nothing of GitHub Actions: this repository is private and its Actions has no budget. The archives and the formula go to [`sunfmin/homebrew-tap`](https://github.com/sunfmin/homebrew-tap), which Homebrew downloads from because this repository is private.

`build` uses rustup's stable toolchain whatever `cargo` is first on `PATH`, and names the line that installs it when it, or the Intel target, is missing.

A cut that fails opens an issue labelled `convoy-cut-failed`, which holds no cut back: the next due cut cuts again from the start on the base tip, and a cut that publishes closes the issue, naming the version. Three failures of the same step the same way in a row make it a ticket for the fleet, under the ready label, and the next cut waits for a landing. The host's watch looks for its cause, and `doctor` says what becomes of it. Only a failure of the four kinds left to a human goes to one, under the escalate label. An afkconvoy released before cutting was (#466) does not cut: where the host runs one, a version is cut by hand by the same block — its `set` lines run with `VERSION` set to the version, that commit landed, the landed commit tagged `v<version>` and the tag pushed, then its `publish` lines run with the tag checked out.

On each other host that runs a fleet, the upgrade is all there is to it:

```sh
brew update && brew upgrade sunfmin/tap/afkconvoy
```

The host runner finds the newer afkconvoy installed where it was started from, and moves onto it between ticks once the running exclusive command ends, on the same checkout list. Claims, workers, PRs waiting to land, a cut and a watch carry over; queued commands rejoin the replacement. It waits at most ten minutes for that moment, then lets work go on under the old release, gives a `doctor` finding naming what held it, and keeps trying the move. `afkconvoy checkouts` answers the runner's version under `host_runner`; `doctor` says how the move stands as `host.installed`, including where the replacement did not start.
