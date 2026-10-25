# afkconvoy

A fleet of AFK coding agents that works a GitHub backlog. `afkconvoy run` takes the issues a repo marks ready, starts one agent per issue in its own worktree, and lands each finished PR through a landing train — unattended, until you stop it. Everything the fleet knows lives on GitHub and the repo's remote: no process holds state, so a fleet can be stopped and started at will, and several hosts can work one repo.

## Install

macOS, Apple Silicon or Intel:

```sh
brew install sunfmin/tap/afkconvoy
```

Homebrew installs `git` and `gh` with it. The rest is yours to set up:

- **orca** — afkconvoy starts every worker through [orca](https://github.com/stablyai/orca): `brew install --cask stablyai/orca/orca`. Give the tap's name: the cask Homebrew itself calls `orca` is another program. Then start it — `orca open` — and, in each checkout a fleet is to work, give it the repo once: `orca repo add --path "$PWD"`.
- **GitHub** — `gh auth login`, as a user who may push to the repo.
- **Claude Code**, when it is what starts your workers — `curl -fsSL https://claude.ai/install.sh | bash`, then `claude auth login`.
- **pi**, when it is what starts some of your workers — `curl -fsSL https://pi.dev/install.sh | sh`, then `pi`, and `/login` at its prompt.
- **Codex**, when it is what starts some of your workers — `brew install --cask codex`, then `codex login`.

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

The whole manual is also a page, for reading in a browser: [`docs/manual.md`](docs/manual.md), generated from it.

## Develop

```sh
./gate.sh     # fmt, clippy, the manual, the tests, and Flux on the core crate
```

The gate runs the tests with [cargo-nextest](https://nexte.st) when it is installed (`brew install cargo-nextest`), which takes about a third of the time; without it, it runs them with `cargo test`. Under nextest one test has a limit, 240 seconds, which [`.config/nextest.toml`](.config/nextest.toml) gives: a test that runs past it is ended and fails by name, as `TIMEOUT`. `cargo test` has no limit for one test, and the gate says so when it runs them that way.

The gate ends with the functions of the core crate that Flux took longest to check, and their times.

Flux on the core crate needs the checker that [`crates/core/Cargo.toml`](crates/core/Cargo.toml) pins, and `./verify.sh` refuses to run with any other, naming the one to install:

- **`cargo-flux`**, built from the Flux revision of the `flux-rs` dependency: `git clone https://github.com/flux-rs/flux && cd flux && git checkout <rev> && cargo xtask install`. [rustup](https://rustup.rs) fetches the nightly toolchain that revision names. `cargo-flux` reports the first seven digits of its revision, which is what is compared; the driver and libraries it installs into `~/.flux` report none, and are not checked.
- **`fixpoint`**, at the version and commit under `[package.metadata.checker]`: [built from source](https://github.com/ucsd-progsys/liquid-fixpoint#how-to-build-and-install) at that commit. Both are compared.
- **`z3`**, at the version under `[package.metadata.checker]`: from [Z3's releases](https://github.com/Z3Prover/z3/releases), or `brew install z3` while Homebrew's is that version. Only the version is compared: `z3` reports nothing of its build.

The decisions live in `crates/core` as pure functions checked by [Flux](https://github.com/flux-rs/flux) (`./verify.sh`); `crates/cli` carries them out through `git`, `gh` and `orca` ([ADR-0007](docs/adr/0007-refinement-types-on-a-pure-core.md)).

**The manual** is generated, and never edited: after changing the help — the guide, a command's, a topic, or `GLOSSARY.md` — run `cargo run -p afkconvoy --example manual` and commit what it wrote to [`docs/manual.md`](docs/manual.md). The gate fails while the page is not what that command writes.

**Cutting a release** is done by hand, on a Mac: this repository is private and its GitHub Actions has no budget. Set `version` in `Cargo.toml`, merge, then tag the merged commit `v<version>` and push the tag. With the tag's commit checked out, from a shell where `gh` and `git` may write to [`sunfmin/homebrew-tap`](https://github.com/sunfmin/homebrew-tap):

```sh
packaging/release.sh check "$(git describe --tags --exact-match)"   # the commit checked out is v<version>
packaging/release.sh build      # both macOS binaries, Apple Silicon and Intel, on rustup's stable toolchain
packaging/release.sh package    # dist/afkconvoy-<version>-macos-<arch>.tar.gz, each checked
packaging/release.sh publish    # the archives to the tap's release afkconvoy-v<version>, the formula to its Formula/afkconvoy.rb
```

Homebrew downloads from the tap because this repository is private.

`build` uses rustup's stable toolchain whatever `cargo` is first on `PATH`, and names the line that installs it when it, or the Intel target, is missing.

Then, on each host that runs a fleet, the upgrade is all there is to it:

```sh
brew upgrade sunfmin/tap/afkconvoy
```

The host runner finds the newer afkconvoy installed where it was started from, and moves onto it once its fleets are idle — nothing claimed, nothing on a landing train, nothing in the exclusive queue, no inspection running — on the same checkout list: every repo the host worked is worked again, and `afkconvoy checkouts` answers the new version under `host_runner`. Until then `afkconvoy doctor` says what holds the move back, as `host.installed`, and gives a finding where the new one did not start.
