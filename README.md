# afkconvoy

A fleet of AFK coding agents that works a GitHub backlog. `afkconvoy run` takes the issues a repo marks ready, starts one agent per issue in its own worktree, and lands each finished PR through a landing train — unattended, until you stop it.

Everything the fleet knows lives on GitHub and the repo's remote: no process holds state, so a fleet can be stopped and started at will, and several hosts can work one repo.

The vocabulary is in [`GLOSSARY.md`](GLOSSARY.md); the decisions behind it are in [`docs/adr/`](docs/adr/).

## Install

macOS, Apple Silicon or Intel:

```sh
brew install sunfmin/tap/afkconvoy
```

Homebrew installs `git` and `gh` with it. Two things are yours to set up:

- **orca** — afkconvoy starts every worker through [orca](https://github.com/stablyai/orca), which Homebrew does not install. Install it and make sure `orca` is on your `PATH`.
- **GitHub** — `gh auth login`, as a user who may push to the repo.

`afkconvoy --version` prints the release you have.

## Configure a repo

Add `docs/agents/afkconvoy.md` to the repo's base branch. afkconvoy reads the first `yaml` block in it and nothing else; an unknown key or a wrong type is an error.

````markdown
```yaml
base_branch: main                 # workers cut from it, open their PR against it, land on it
gate_command: ./gate.sh           # what must pass before a PR lands
ready_label: ready-for-agent      # an issue is dispatchable while it carries this
epic_labels: [epic, prd]          # never dispatched
concurrency: 3                    # claims held at once, a worker each
escalate_label: ready-for-human   # swapped in for ready_label when the fleet gives an issue up
```
````

`base_branch` and `gate_command` have no default; the rest default to the values shown. `afkconvoy config` prints what was read.

Then:

- **Label the issues** the fleet may take with the ready label. An issue GitHub records as blocked by an open issue waits for it.
- **Leave the base branch pushable.** The landing train lands by pushing a commit the gate passed. A base branch that requires status checks or pull request reviews, restricts who may push, or is locked is refused when the fleet starts, naming the rule.
- **Make the gate one command** that exits 0 only when the tree is good. It is the only machine check a change passes before it lands.

## Start a fleet

In a checkout of the repo:

```sh
afkconvoy plan    # what a tick would act on; changes nothing
afkconvoy run     # the fleet: tick after tick, until you stop it
```

`run` prints one line per tick. Stop it with Ctrl-C: no claim is released on the way out, and the next `run` on the same host takes its claims back.

**The worker launch command.** A worker starts in a fresh login shell that inherits nothing from the shell `run` was started in. With no `ANTHROPIC_BASE_URL` set, workers are started with `claude --dangerously-skip-permissions`. If you run Claude through a provider wrapper, say what starts a worker the same way:

```sh
afkconvoy run --worker-command kim
```

Without `--worker-command`, `run` lists your login shell's aliases that start Claude and asks. The command is handed to orca exactly as given; it must resolve in a login shell, or nothing is started.

**What a start checks**, before anything else: the base branch exists and can be pushed to; where the remote keeps the fleet's refs (`refs/convoy/*`, or `convoy-*` branches on a remote that rejects those — push-triggered CI will then fire on them); whether gate runs can be put on record.

**When the fleet gives an issue up** it swaps the ready label for the escalate label and says why in a comment on the issue. Fix what it names and put the ready label back.

**Another host's claims** come free on their own when its heartbeat goes stale. `afkconvoy takeover --list` shows who holds what; `afkconvoy takeover --from <instance> --instance <yours>` takes them now.

Every transition the fleet makes is also a subcommand, for doing one by hand: see `afkconvoy --help`.

## Develop

```sh
./gate.sh     # fmt, clippy, the tests, and Flux on the core crate
```

The decisions live in `crates/core` as pure functions checked by [Flux](https://github.com/flux-rs/flux) (`./verify.sh`); `crates/cli` carries them out through `git`, `gh` and `orca` ([ADR-0007](docs/adr/0007-refinement-types-on-a-pure-core.md)).

**Cutting a release**: set `version` in `Cargo.toml`, merge, then push the tag `v<version>`. The [release workflow](.github/workflows/release.yml) builds both macOS binaries on stable Rust and publishes them with the Homebrew formula. With the repository secret `HOMEBREW_TAP_TOKEN` set, it also commits the formula to [`sunfmin/homebrew-tap`](https://github.com/sunfmin/homebrew-tap); without it, commit the release's `afkconvoy.rb` there as `Formula/afkconvoy.rb`.
