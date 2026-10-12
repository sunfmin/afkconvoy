# afkconvoy

A fleet of AFK coding agents that works a GitHub backlog. `afkconvoy run` takes the issues a repo marks ready, starts one agent per issue in its own worktree, and lands each finished PR through a landing train — unattended, until you stop it.

Everything the fleet knows lives on GitHub and the repo's remote: no process holds state, so a fleet can be stopped and started at will, and several hosts can work one repo.

The vocabulary is in [`GLOSSARY.md`](GLOSSARY.md); the decisions behind it are in [`docs/adr/`](docs/adr/).

## Install

macOS, Apple Silicon or Intel:

```sh
brew install sunfmin/tap/afkconvoy
```

Homebrew installs `git` and `gh` with it. Three things are yours to set up:

- **orca** — afkconvoy starts every worker through [orca](https://github.com/stablyai/orca): `brew install --cask stablyai/orca/orca`. Give the tap's name: the cask Homebrew itself calls `orca` is another program.
- **GitHub** — `gh auth login`, as a user who may push to the repo.
- **Claude Code**, when it is what starts your workers — `curl -fsSL https://claude.ai/install.sh | bash`, then `claude auth login`.

`run`, `tick` and `plan` check all of this before anything else, and refuse with the line to run for each thing missing or logged out. afkconvoy installs nothing and logs nobody in.

`afkconvoy --version` prints the release you have.

**For an AI agent**: `afkconvoy --help` is written as the guide an agent learns the tool from. A skill that sends an agent there installs with:

```sh
npx skills add sunfmin/afkconvoy
```

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

A repo may also name a team — `team: owner/repo` — which makes it a bound repo (see [`GLOSSARY.md`](GLOSSARY.md)); `patrols`, `lock_label` and `intake_limit` are read beside it. `afkconvoy check` reads the team and its workflows and reports every fault with its file and key; `afkconvoy check --help` gives the files and keys of a team. `afkconvoy show` draws a workflow as a graph, and `afkconvoy rehearse <workflow> <verdict>…` says where an issue would be after each verdict, with no worker started. A worker ends its stage with `afkconvoy verdict`, which leaves one stage record on the issue; `afkconvoy show --issue <n>` reads an issue's position, what follows and the rework it has left off those records, and each tick keeps a `stage:<name>` label on the issue for humans to filter by — the fleet never reads it. A worker takes an issue in to a workflow with `afkconvoy intake`, which leaves a record of the role that did and the issue it was on; the open issues taken in that way are held under `intake_limit`, `afkconvoy plan` reports the count, and an issue a human labels is outside it. A stage that finds its issue too large splits it: it opens child issues, takes each in — `afkconvoy intake --blocked-by <n,…>` says which of the others a child waits for — and declares `blocked` on all of them; the issue is parked, its children are worked in the order their blockers close, and when the last has closed the stage that split it runs again and judges the whole. A workflow whose last stage passes to `land` names the role that lands (`land: { by, do }`): the tick after that `pass` starts a worker in it, which gates the issue's PR and joins the landing train, and the train lands it as it lands any PR. An issue whose rework would pass one of its workflow's two breakers is escalated instead of dispatched, with every round's stage and reason in the comment. A human answers an escalated issue with `afkconvoy rule --issue <n> --text …`: the stage it stopped at runs again under the ruling, with both rework counts started over, and the team's escalate label is swapped back for the flow label. `afkconvoy send --issue <n> --stage <name> --text …` puts an issue at any stage of its workflow. A stage a workflow gives to a human (`by: human`) starts no worker and holds no claim: the issue waits there until a human declares its verdict with the same `afkconvoy verdict`. A repo that names no team is unbound and works as described here.

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

**In a bound repo** a worker starts in its role, so the worker launch command is no longer opaque: it must end in Claude Code taking flags, because the fleet appends to it. The terminal is opened on the issue's worktree as ever, and the line it runs is:

```sh
cd <the role's copy> && export <the stage's facts> && <worker launch command> \
  --add-dir <the issue's worktree> --add-dir <the team's copy> --setting-sources project \
  --settings <the guard, as a hook> [--model <the stage's>]
```

- The command must take `--add-dir`, `--setting-sources`, `--settings` and `--model`. A start runs `<worker launch command> --help` in a login shell and refuses, naming what is missing, when the answer does not name all four. An alias or a wrapper is fine as long as what follows it reaches Claude Code: `kim`, not `kim ; true`.
- What is added is fixed, and nothing of it is configurable. The role's rules, skills and hooks load because the worker starts in the role's directory; the repo's own rules (`CLAUDE.md`) load from the added worktree, its hooks do not — what a repo must enforce belongs in its gate; your own Claude Code settings, rules, skills and hooks are not loaded at all.
- The role's and the team's directories are the fleet's own read-only copies, one per commit, under `~/.cache/afkconvoy/copies/` (or `$XDG_CACHE_HOME`). A stage starts under the commit at the head of each repo's default branch; a copy already there is not fetched again. Pushing is the only way a change to a role reaches a worker.
- The stage's facts are in the worker's environment: `AFKCONVOY_ISSUE`, `AFKCONVOY_FLOW`, `AFKCONVOY_STAGE`, `AFKCONVOY_ROLE`, `AFKCONVOY_SKILL`, `AFKCONVOY_WRITES`, `AFKCONVOY_CONSULT`, `AFKCONVOY_BASE_BRANCH`, `AFKCONVOY_ESCALATE_LABEL`, `AFKCONVOY_LOCK_LABEL`, `AFKCONVOY_ROLE_COMMIT` and `AFKCONVOY_TEAM_COMMIT`.
- Every worker is started with the guard: `afkconvoy guard`, added as a hook that Claude Code runs before each tool call, whatever the role's repo holds. It refuses, with a reason the worker is shown, what no worker may do — add or remove a flow, stage, escalate or lock label, write a stage record by hand, merge a PR, push the base branch — and, in a stage marked `writes: false`, committing and pushing. A role's own limits are hooks in its repo; they run beside the guard. A guard recognises commands, so it stops a worker that errs, not one that means to get past it: after a stage marked `writes: false` the fleet checks the branch, and a commit that was not there before escalates the issue. `afkconvoy guard --help` lists what is refused.

An unbound repo's command stays opaque and may start any agent.

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
