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

Flux on the core, harness, gate and landing crates needs the checker that [`Cargo.toml`](Cargo.toml) and [`crates/core/Cargo.toml`](crates/core/Cargo.toml) pin, and a `fixpoint`: `./verify.sh` refuses to run with any other, or with none, naming the one to install. `./verify.sh install` installs each that is missing under the home directory.

- **`cargo-flux`**, built from the Flux revision of the `flux-rs` dependency: `git clone https://github.com/flux-rs/flux && cd flux && git checkout <rev> && cargo xtask install`. [rustup](https://rustup.rs) fetches the nightly toolchain that revision names. `cargo-flux` reports the first seven digits of its revision, which is what is compared; the driver and libraries it installs into `~/.flux` report none, and are not checked.
- **`fixpoint`**, of no pin: liquid-fixpoint's [nightly release](https://github.com/ucsd-progsys/liquid-fixpoint/releases/tag/nightly), as Flux's own installer takes it, for `aarch64-apple-darwin` and `x86_64-linux-gnu`; on another platform, [built from source](https://github.com/ucsd-progsys/liquid-fixpoint#how-to-build-and-install) by hand. It is checked only to report its version. The day a change of fixpoint turns a gate result wrong, it is pinned again (sunfmin/control#732).
- **`z3`**, at the version under `[package.metadata.checker]`: from [Z3's releases](https://github.com/Z3Prover/z3/releases), or `brew install z3` while Homebrew's is that version. Only the version is compared: `z3` reports nothing of its build.

The decisions live in `crates/core` as pure functions checked by [Flux](https://github.com/flux-rs/flux) (`./verify.sh`); `crates/cli` carries them out through `git`, `gh` and `orca` ([ADR-0007](docs/adr/0007-refinement-types-on-a-pure-core.md)). What afkconvoy knows of each harness — how one starts, where it reads rules and skills, how a hook reaches it, where it keeps its session and what its usage lines count — is `crates/harness`, which the core depends on and another program may too, by a git dependency pinned to a revision; its API names nothing of the fleet ([ADR-0058](docs/adr/0058-the-harness-crate-may-be-depended-on-by-another-program.md)). The gate and the exclusive queue — the fast and slow gate a repo declares, recorded gate runs, heavy commands and the host's lanes, and the holder of the queue a host process embeds — are `crates/gate`, on the same terms ([ADR-0059](docs/adr/0059-the-gate-crate-may-be-depended-on-by-another-program.md)). Candidates, candidate chains, rounds and landing, with the forge a program's changes are asked on behind an interface that afkconvoy implements for GitHub, are `crates/landing`, on the same terms again ([ADR-0060](docs/adr/0060-candidates-rounds-and-landing-may-be-depended-on-by-another-program.md)).

**Cutting a release** is the fleet's: once it goes idle with landed work past the newest `v*` tag, it cuts the next version itself — commits it on a branch of its own from the base tip, gates that commit, tags it, and publishes and installs it on its host, while turns keep landing on the base branch (`afkconvoy cut --help`). `afkconvoy plan` says when the next cut starts. Its steps are the ```` ```yaml cut ```` block of [`docs/agents/afkconvoy.md`](docs/agents/afkconvoy.md), the one place they are written: steps of [`packaging/release.sh`](packaging/release.sh), then Homebrew's. They run on a Mac, and on nothing of GitHub Actions: this repository is private and its Actions has no budget. The archives and the formula go to [`sunfmin/homebrew-tap`](https://github.com/sunfmin/homebrew-tap), which Homebrew downloads from because this repository is private.

`build` uses rustup's stable toolchain whatever `cargo` is first on `PATH`, and names the line that installs it when it, or the Intel target, is missing.

A cut that fails opens an issue under the escalate label, with the line that goes on from the step that failed. An afkconvoy released before cutting was (#466) does not cut: where the host runs one, a version is cut by hand by the same block — its `set` lines run with `VERSION` set to the version, that commit landed, the landed commit tagged `v<version>` and the tag pushed, then its `publish` lines run with the tag checked out.

On each other host that runs a fleet, the upgrade is all there is to it:

```sh
brew update && brew upgrade sunfmin/tap/afkconvoy
```

The host runner finds the newer afkconvoy installed where it was started from, and moves onto it between ticks once the running exclusive command ends, on the same checkout list. Claims, workers, PRs waiting to land, a cut and a watch carry over; queued commands rejoin the replacement. It waits at most ten minutes for that moment, then lets work go on under the old release, gives a `doctor` finding naming what held it, and keeps trying the move. `afkconvoy checkouts` answers the runner's version under `host_runner`; `doctor` says how the move stands as `host.installed`, including where the replacement did not start.
