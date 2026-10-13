# afkconvoy

A fleet of AFK coding agents that works a GitHub backlog. `afkconvoy run` takes the issues a repo marks ready, starts one agent per issue in its own worktree, and lands each finished PR through a landing train — unattended, until you stop it.

Everything the fleet knows lives on GitHub and the repo's remote: no process holds state, so a fleet can be stopped and started at will, and several hosts can work one repo.

The [user manual](docs/manual.md) ([中文](docs/manual.zh.md)) goes from nothing to a repo worked by a team of roles, with one worked example. The vocabulary is in [`GLOSSARY.md`](GLOSSARY.md); the decisions behind it are in [`docs/adr/`](docs/adr/).

## Install

macOS, Apple Silicon or Intel:

```sh
brew install sunfmin/tap/afkconvoy
```

Homebrew installs `git` and `gh` with it. Three things are yours to set up:

- **orca** — afkconvoy starts every worker through [orca](https://github.com/stablyai/orca): `brew install --cask stablyai/orca/orca`. Give the tap's name: the cask Homebrew itself calls `orca` is another program. Then start it — `orca open` — and, in each checkout a fleet is to work, give it the repo once: `orca repo add --path "$PWD"`.
- **GitHub** — `gh auth login`, as a user who may push to the repo.
- **Claude Code**, when it is what starts your workers — `curl -fsSL https://claude.ai/install.sh | bash`, then `claude auth login`.
- **pi**, when a stage or a patrol your fleet runs is on it — `curl -fsSL https://pi.dev/install.sh | sh`, then `pi`, and `/login` at its prompt. A team that names no pi stage is asked nothing about pi. What is checked is that pi holds credentials for a provider (`pi --list-models` lists a model): that it is the provider of each stage's model is yours to see to.

`run`, `tick` and `plan` check all of this before anything else, and refuse with the line to run for each thing missing, logged out, not running or not given the checkout. afkconvoy installs nothing, starts and adds nothing, and logs nobody in.

`afkconvoy --version` prints the release you have. A build of anything else says what it was built from — `afkconvoy 0.2.0+11.g9f58035` is eleven commits past the newest tag, at commit `9f58035`, and `.dirty` after it is a tree that had changes — and the status board on an issue names the build of the fleet that works it.

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
lock_label: fleet-lock            # an open issue carrying this is a fleet lock
```
````

`base_branch` and `gate_command` have no default; the rest default to the values shown. `afkconvoy config` prints what was read.

A repo may also name a team, which makes it a bound repo: its issues then pass through the stages of a workflow, each performed by a worker in its own role. See [Bind a repo to a team](#bind-a-repo-to-a-team). A repo that names no team is unbound and works as described here.

When the fault is one every issue shares — the base branch is broken — put the lock label on the issue that carries the fix, or have a worker run `afkconvoy lock --issue <n>`. While that issue is open the fleet works lock issues only: nothing else is dispatched, no PR of another issue joins the landing train, and the workers already running are left alone, what they declared being acted on afterwards — but for a stage that declared `escalate` or went silent before the lock closed: a broken base branch explains that, so the stage runs again instead of going to a human. A worker whose gate is red while a lock stands is told so by `afkconvoy gate`. Closing the issue ends the lock; `plan` and every tick say which lock stands.

Then:

- **Label the issues** the fleet may take with the ready label. An issue GitHub records as blocked by an open issue waits for it.
- **Leave the base branch pushable.** The landing train lands by pushing a commit the gate passed. A base branch that requires status checks or pull request reviews, restricts who may push, or is locked is refused when the fleet starts, naming the rule.
- **Make the gate one command** that exits 0 only when the tree is good. It is the only machine check a change passes before it lands.

## Bind a repo to a team

A **team** is a directory — a whole repo, or a subdirectory of one — that names **roles** and holds **workflows**; a **role** is a directory too — the rules one kind of worker always works under, the limits it cannot cross, and the skills it can be asked to perform. A repo whose config names a team is a **bound repo**: an issue of it follows the workflow its flow label names, stage by stage, each stage one skill of one role performed by a new worker. The vocabulary is [`GLOSSARY.md`](GLOSSARY.md)'s; the decisions are [ADR-0008](docs/adr/0008-roles-and-workflows-are-part-of-afkconvoy.md) to [ADR-0012](docs/adr/0012-a-worker-is-limited-by-guards-not-by-identity.md).

### From nothing to a bound repo

```sh
afkconvoy scaffold team team --role dev --role reviewer --repo acme/team   # a team and its roles, in one repo created on GitHub
afkconvoy check --team acme/team                                          # does it load? every fault, with its file and key
afkconvoy scaffold role roles/tester                                      # in the team's checkout: one more role, and the line to add to team.md
```

`scaffold team` writes the team file (`team.md`), the team rules (`CLAUDE.md`: a heading, and what belongs there and what belongs in a repo's own rules or a role's), one example workflow (`workflows/feature.md`) and, under `roles/<name>/`, the skeleton of each role given with `--role <name>`: a rules file (`CLAUDE.md`), one example skill (`.claude/skills/implement/SKILL.md`) and a hooks file (`.claude/settings.json`). The rules and the skills are written once, whatever harness runs the role; its limits are written once for each harness — hooks in `.claude/settings.json` for Claude Code, extensions in `.pi/extensions/` for pi — and `afkconvoy check` refuses a stage on a harness its role holds no limits for while it holds some for another. `team.md` names each as `./roles/<name>`, a path in the team's own repo. `--repo owner/repo` commits the skeleton and creates that one repo on GitHub, private, with `gh repo create`; it passes `afkconvoy check` as written. Without `--repo` only files are written, and `check` passes once they are pushed: it reads a role from GitHub, at the head of its repo's default branch.

`scaffold role <dir>` writes one role's skeleton. In a checkout it creates nothing on GitHub, and its answer holds the line to add under `roles` in `team.md` — `"team_line": "tester: ./roles/tester"`. A role several teams share is a repo of its own instead: `scaffold role dev --repo acme/dev` creates it, and a team names it with `--role dev=acme/dev`, or `dev=owner/repo/path` for a directory of a repo.

To read a finished team first, or to start from one: `afkconvoy scaffold team hospital --example hospital --repo acme/hospital` writes a whole team the binary holds — the hospital this project demonstrates itself with: seven roles, three workflows, four patrols, team rules and precedents, the team at the root and each role under `roles/` — and `afkconvoy help example` goes through it file by file. Skills by other authors that its roles load are not written: the topic says which, and how to add them.

From there a team is designed by editing those files: edit, run `afkconvoy check --dir <the team's checkout> --team <owner/repo>`, repeat until it exits 0 — `--team` says the repo a role named `./path` is read from. `afkconvoy help team`, `afkconvoy help workflows` and `afkconvoy help patrols` lay out every file of a team and of a role and every key; `afkconvoy show` draws a workflow as a graph, and `afkconvoy rehearse <workflow> <verdict>…` says where an issue would be after each verdict, with no worker started.

**A role's skills** are added with the skills installer, run in the role's directory, in a checkout of its repo, and committed there:

```sh
cd team/roles/dev && npx skills add <owner/repo of the skills> && git add -A && git commit -m "Add a skill" && git push
```

A skill a role can be asked to perform is an entry of `.claude/skills/` in its repo, so every host that runs the role runs the same one. afkconvoy installs no skill and wraps no installer. Pushing is the only way a change to a role or a team reaches a worker.

When the team loads, push it, name it in the repo's config on the base branch, and run `afkconvoy check` in that checkout.

### The keys

A bound repo's config keeps the keys only the repo knows — `base_branch`, `gate_command`, `epic_labels`, `concurrency`, `lock_label` — and reads four more:

````markdown
```yaml
team: owner/repo                  # the team's repo: naming it is what binds this repo
patrols: [triage]                 # which of the team's patrols run here, by name (default: none)
intake_limit: 50                  # open issues taken in by workers, at most (default: 50)
harness: claude-code              # the harness of a worker whose stage or patrol names none (default: claude-code)
```
````

`ready_label` means nothing in a bound repo: its issues carry a flow label instead. The escalate label is the team's (`escalate_label` in `team.md`, default `ready-for-human`). `patrols`, `intake_limit` and `harness` are errors where no `team` is. A stage a role performs and a patrol may name a `harness` of their own beside their `model`; a consult, a landing and the proposal of a precedent run on the config's. Claude Code, `claude-code`, is the only harness there is.

### The flow label

A workflow is a file of the team, `workflows/<name>.md`, and its flow label is `flow:<name>`. An issue is the fleet's to work only while it carries one: put `flow:feature` on an issue and it enters the workflow `feature` at its first stage; an issue with no flow label is left alone. `afkconvoy flows`, run in a checkout of the repo, says what each workflow is for — the prose of its file above the yaml — and the line that opens an issue into it, and creates the flow labels the repo lacks: `gh issue create --label` refuses a label that is not there. Putting it there is intake. A human does it on GitHub, outside any limit. A worker does it with `afkconvoy intake`, which leaves a record of the role that did and the issue it was on; the open issues taken in that way are held under `intake_limit`, and `afkconvoy plan` reports the count.

### What a human is asked, and when

Nothing, by default: a workflow runs from intake to the landing train with no human in it. A human is reached by exception, and each time the same way — the fleet says why in a comment on the issue, swaps the flow label for the team's escalate label, and releases its claim:

- **A stage declared `escalate`** and no consult ruled on it. A stage may name a more senior role to consult (`escalate: { to, do }`): its `escalate` verdict then starts a worker in that role, in one slot of `concurrency`, which rules with `afkconvoy rule` — the stage that asked runs again under the ruling, as one more of the issue's rework in total — or lets the escalation through to a human with its own `escalate`. A stage that names none escalates to a human at once. A consult writes no code: one that left a commit has failed, and the issue escalates.
- **A breaker tripped.** An issue whose rework would pass one of its workflow's two breakers (`max_rework`, `max_consecutive_rework`) is escalated instead of dispatched, with every round's stage and reason in the comment.
- **A failure.** A worker that crashed or stayed silent, a gate that could not be turned green, a stage that ended with no verdict or a verdict of another stage, a commit left by a stage marked `writes: false`, a workflow changed under an issue in flight. Nothing is retried.

The comment of an escalation ends with the line that answers it, the issue's number filled in, and `afkconvoy doctor` lists every open issue that waits for a human under `waiting_on_human` — each with that line, and as a finding once it has waited for more than an hour. A human answers an escalated issue with `afkconvoy rule <n> --text …` (or `--issue <n>`: every command about one issue takes its number either way): the stage it stopped at runs again under the ruling, with both rework counts started over, and the escalate label is swapped back for the flow label. `afkconvoy send --issue <n> --stage <name> --text …` puts an issue at any stage of its workflow instead.

Two more things wait for a human, and only where the team chose them:

- **A stage given to a human** (`by: human`) starts no worker and holds no claim: the issue waits there until a human declares its verdict with `afkconvoy verdict`.
- **A patrol's run that failed** — it left a commit or opened a PR. The fleet opens an issue with the team's escalate label that says so, touches no other issue, and does not run the patrol again until a human closes it.

### How an issue moves

A worker ends its stage with `afkconvoy verdict`, which leaves one stage record on the issue; `afkconvoy show --issue <n>` reads an issue's position, what follows and the rework it has left off those records, and each tick keeps a `stage:<name>` label on the issue for humans to filter by — the fleet never reads it.

A stage that finds its issue too large splits it: it opens child issues, takes each in — `afkconvoy intake --blocked-by <n,…>` says which of the others a child waits for — and declares `blocked` on all of them; the issue is parked, its children are worked in the order their blockers close, and when the last has closed the stage that split it runs again and judges the whole.

A workflow whose last stage passes to `land` names the role that lands (`land: { by, do }`): the tick after that `pass` starts a worker in it, which gates the issue's PR and joins the landing train, and the train lands it as it lands any PR.

A team's patrols are files too (`patrols/<name>.md`: `by`, `do`, `every`, `harness`, `model`, `max_open`), and a repo runs the ones its `patrols` names: the tick that finds one due — its interval passed since its last recorded run, no run of it still going, no fleet lock standing, fewer than `max_open` open issues it opened — claims the run on the remote, so of several hosts one runs it, and starts a worker in the patrol's role in a checkout of the base branch that is linked to no issue. That worker has `AFKCONVOY_PATROL` in its environment, opens issues and takes them in with `afkconvoy intake`; a run takes one slot of `concurrency` until its worker declares it over with `afkconvoy over` — the last line of its brief — or, where it never does, until it is gone or has been stopped for five minutes; a missed run is not made up, and `afkconvoy plan` lists how each patrol stands. The tick that ends a run closes its worker's terminal and removes its checkout — the next run starts in a new one — unless the run failed: then the checkout is kept for the human its report is opened for.

### Precedents

A team may name who proposes precedents (`precedents: { by, do, max_open }` in `team.md`). A human's ruling on an escalated issue then starts one worker in that role, with that skill, in a checkout of the base branch linked to no issue. It opens one PR on the team's repo that appends one entry to `precedents.md` — the ruling in a sentence, when it applies, and the link to the ruling. A human merging that PR is what accepts the precedent: the fleet never merges on the team's repo. Once the worker has stopped, the fleet closes, with the reason, a PR that changes anything but that file or does anything there but append that entry.

A ruling is proposed at most once, and none while the open proposals have reached `max_open`; `afkconvoy plan` lists how each stands. The issue goes on through its workflow without waiting. A verdict a human declares for a `by: human` stage is no ruling, and neither is a consult's. A team without the key has no precedents proposed.

Every worker's brief says where the precedents are, to read them before declaring `escalate`, and to say in that verdict that it did.

How a bound repo's worker is started — in its role, under the guard — is under [Start a fleet](#start-a-fleet).

## Start a fleet

In a checkout of the repo:

```sh
afkconvoy plan    # what a tick would act on; changes nothing
afkconvoy run     # the fleet: tick after tick, until you stop it
```

`run` prints one line per tick. Stop it with Ctrl-C: it finishes the step it is in, no claim is released on the way out, and the next `run` on the same host takes its claims back. Holding no claim, it takes its heartbeat off the remote as it goes.

**The worker launch command.** A worker starts in a fresh login shell that inherits nothing from the shell `run` was started in. With no `ANTHROPIC_BASE_URL` set, workers are started with `claude --dangerously-skip-permissions --settings '{"skipDangerousModePermissionPrompt":true}'`: Claude Code in bypass-permissions mode, with the warning it opens in that mode answered — a worker has nobody at its terminal to accept it, and stops there. A command of your own that starts Claude Code in that mode needs the same setting, on a host where you never accepted the warning by hand; in a bound repo the fleet adds it to whatever command you say. If you run Claude through a provider wrapper, say what starts a worker the same way:

```sh
afkconvoy run --worker-command kim
```

Without `--worker-command`, `run` lists your login shell's aliases that start Claude and asks. The command is handed to orca exactly as given; it must resolve in a login shell, or nothing is started.

A fleet instance has one worker launch command for each harness its workers run, and the flag has two forms. The bare one above says the command of the repo's default harness. `--worker-command <harness>=<command>` says the command of one harness by its key, and is given once for each:

```sh
afkconvoy run --worker-command claude-code=kim --worker-command pi=mypi
```

The harnesses are `claude-code` and `pi`. A harness nobody said a command for is started with its stock one: `claude --dangerously-skip-permissions --settings '{"skipDangerousModePermissionPrompt":true}'`, and `pi`. A command given twice for one harness, or for a name that is no harness, is an error. A value is read as `<harness>=<command>` when its first word holds an `=`.

**In a bound repo** a worker starts in its role, so the worker launch command is no longer opaque: it must end in its harness taking flags, because the fleet appends to it. The terminal is opened on the issue's worktree as ever, and the line it runs for a Claude Code worker is:

```sh
cd <the role's copy> && export <the stage's facts> && <worker launch command> \
  --add-dir <the issue's worktree> --add-dir <the team's copy> --setting-sources project \
  --settings <the guard and orca's status report, as hooks, and the bypass warning answered> [--model <the stage's>]
```

- The command must take `--add-dir`, `--setting-sources`, `--settings` and `--model`. A start runs `<worker launch command> --help` in a login shell and refuses, naming what is missing, when the answer does not name all four.
- Which harness a worker runs is the team's to say: a stage or a patrol names one, or takes the repo's default. Before any worker is started, a start, `plan` and `doctor` check the command of every harness the repo's default, the team's workflows and the patrols the repo runs call for. A fault is said with its harness and the stage or patrol that calls for it, and a start with one harness that cannot be started starts no worker: none is ever started in another's place. An alias or a wrapper is fine as long as what follows it reaches Claude Code: `kim`, not `kim ; true`.
- What is added is fixed, and nothing of it is configurable. The role's rules, skills and hooks load because the worker starts in the role's directory; the repo's own rules (`CLAUDE.md`) load from the added worktree, its hooks do not — what a repo must enforce belongs in its gate; the team rules (the `CLAUDE.md` in the team's directory, where it has one) load from the team's copy, for every worker of the team; your own Claude Code settings, rules, skills and hooks are not loaded at all.
- The warning Claude Code opens with in bypass-permissions mode is answered in the settings the fleet adds, whatever the worker launch command: of two `--settings` Claude Code takes the last, whole, which is the fleet's. Nothing is written to your own settings.
- The role's and the team's directories are the fleet's own read-only copies, one per commit, under `~/.cache/afkconvoy/copies/` (or `$XDG_CACHE_HOME`). A stage starts under the commit at the head of each repo's default branch; a copy already there is not fetched again, and a repo is fetched once however many roles it holds. A role that is a subdirectory of its repo is copied on its own, so the worker is given that directory and nothing else of the repo: no `CLAUDE.md` above it but the team rules, and no neighbour's directory. The team's copy is its directory without the directories of its roles, and without a `.claude/` at its top: of what a repo's root holds, the `CLAUDE.md` is the one meant to reach every worker, and `afkconvoy check` notes a `.claude/` there, which no worker loads. Pushing is the only way a change to a role reaches a worker.
- The stage's facts are in the worker's environment: `AFKCONVOY_ISSUE`, `AFKCONVOY_FLOW`, `AFKCONVOY_STAGE`, `AFKCONVOY_ROLE`, `AFKCONVOY_SKILL`, `AFKCONVOY_WRITES`, `AFKCONVOY_CONSULT`, `AFKCONVOY_BASE_BRANCH`, `AFKCONVOY_ESCALATE_LABEL`, `AFKCONVOY_LOCK_LABEL`, `AFKCONVOY_ROLE_COMMIT` and `AFKCONVOY_TEAM_COMMIT`.
- Every worker is started with the guard: `afkconvoy guard`, added as a hook that Claude Code runs before each tool call, whatever the role's repo holds. It refuses, with a reason the worker is shown, what no worker may do — add or remove a flow, stage, escalate or lock label, write a stage record by hand, merge a PR, push the base branch — and, in a stage marked `writes: false` and for every consult, committing and pushing. A role's own limits are hooks in its repo; they run beside the guard. A guard recognises commands, so it stops a worker that errs, not one that means to get past it: after a stage marked `writes: false`, and after a consult, the fleet checks the branch, and a commit that was not there before escalates the issue. `afkconvoy guard --help` lists what is refused.
- Every worker is started reporting to orca too. orca hears whether an agent is working or has stopped through hooks it keeps in your own Claude Code settings, which a worker in its role does not load; the fleet adds the same hook (`~/.orca/agent-hooks/claude-hook.sh`, where orca installed it) beside the guard, so `orca worktree ps` shows an agent for a stage's worker as it does for the train's. Where orca has no report all the same — a host whose orca installed no hook, a runtime that sends none — the worker's terminal speaks for it: one that wrote within the grace period (five minutes) is at work, and one quiet longer has stopped. A worker is also left a grace period from its start, whatever its terminal and its worktree show.

**A pi worker** — of a stage or a patrol that says `harness: pi` — starts in its role's directory like any other, and the fleet adds what pi does not load from there:

```sh
cd <the role's copy> && export PI_CODING_AGENT_DIR=<the fleet's own> <the stage's facts> && <worker launch command> \
  --approve --no-skills --skill <the role's copy>/.claude/skills \
  --extension <the fleet's own>/afkconvoy.js [--model <the stage's>]
```

- pi's stock command is `pi`: it asks nothing before a tool call. A command you give must take `--approve`, `--no-skills`, `--skill`, `--extension` and `--model`, checked as Claude Code's is.
- The role's rules (`CLAUDE.md`) load because pi starts in the role's directory. Its skills are named (`--skill`), and pi looks for no other (`--no-skills`): pi reads skills from `~/.agents/skills` wherever its user directory is, and none of yours is to be loaded.
- `--approve` answers on the line the question pi asks before it loads a directory's own files. Asked at the terminal it would take the worker's brief for its answer; answered, pi loads the role's own limits, which for pi are extensions in the role's `.pi/extensions/`. A role's Claude Code hooks are not run by pi: a limit is written once per harness.
- pi's user directory is the fleet's own, `~/.cache/afkconvoy/harness/pi` (or under `$XDG_CACHE_HOME`), not yours (`~/.pi/agent`). Each start puts in it links to your `auth.json` and `models.json` — your logins and the models you defined, so a login pi renews is renewed for you too — and a link to the extension orca installed to hear of a pi session (`extensions/orca-agent-status.ts`), where there is one. Nothing else of yours is there: no rule, skill, setting or other extension. Two things it cannot leave out: a provider key in your login shell's environment, and a rules file in a directory above the fleet's copies (`~/AGENTS.md`).
- The extension is afkconvoy's own, written there by each start. It asks `afkconvoy guard` before every tool call — the same guard, refusing the same things for the same reasons — and a call the guard refuses, or could not be asked of, is not made; a pi worker whose extension is missing does not start. It also loads the rules of the issue's worktree (`AGENTS.md`, or `CLAUDE.md`) and the team rules (the `CLAUDE.md` in the team's directory), which pi does not find from the role's directory.
- A stage's model is its harness's own name for it: pi's is `<provider>/<id>`. A stage on pi that names none gets whichever model pi picks for itself.
- orca hears of a pi worker from its first turn on; until then, and on a host where orca installed no extension, its terminal speaks for it, as above.

An unbound repo's command stays opaque and may start any agent.

**What a start checks**, before anything else: the base branch exists and can be pushed to; where the remote keeps the fleet's refs (`refs/convoy/*`, or `convoy-*` branches on a remote that rejects those — push-triggered CI will then fire on them); whether gate runs can be put on record.

**When the fleet gives an issue up** it swaps the ready label for the escalate label and says why in a comment on the issue, which ends with the line that puts the ready label back. Fix what it names and run it. The tick's line says `escalated #<n>` with why in a few words, and `afkconvoy doctor` lists what waits for you under `waiting_on_human`.

**Another host's claims** come free on their own when its heartbeat goes stale. `afkconvoy takeover --list` shows who holds what; `afkconvoy takeover --from <instance> --instance <yours>` takes them now.

**When something looks wrong**, run `afkconvoy doctor` in the checkout the fleet runs in. It reads this host, the log `run` keeps of its ticks, GitHub, the remote and orca, changes nothing, and answers what the fleet is doing and a list of findings — each with the line to run, or the file and key to edit. It exits non-zero while a finding is critical.

Every transition the fleet makes is also a subcommand, for doing one by hand: see `afkconvoy --help`.

## Develop

```sh
./gate.sh     # fmt, clippy, the tests, and Flux on the core crate
```

The gate runs the tests with [cargo-nextest](https://nexte.st) when it is installed (`brew install cargo-nextest`), which takes about a third of the time; without it, it runs them with `cargo test`.

Flux on the core crate needs the checker that [`crates/core/Cargo.toml`](crates/core/Cargo.toml) pins, and `./verify.sh` refuses to run with any other, naming the one to install:

- **`cargo-flux`**, built from the Flux revision of the `flux-rs` dependency: `git clone https://github.com/flux-rs/flux && cd flux && git checkout <rev> && cargo xtask install`. [rustup](https://rustup.rs) fetches the nightly toolchain that revision names. `cargo-flux` reports the first seven digits of its revision, which is what is compared; the driver and libraries it installs into `~/.flux` report none, and are not checked.
- **`fixpoint`**, at the version and commit under `[package.metadata.checker]`: [built from source](https://github.com/ucsd-progsys/liquid-fixpoint#how-to-build-and-install) at that commit. Both are compared.
- **`z3`**, at the version under `[package.metadata.checker]`: from [Z3's releases](https://github.com/Z3Prover/z3/releases), or `brew install z3` while Homebrew's is that version. Only the version is compared: `z3` reports nothing of its build.

The decisions live in `crates/core` as pure functions checked by [Flux](https://github.com/flux-rs/flux) (`./verify.sh`); `crates/cli` carries them out through `git`, `gh` and `orca` ([ADR-0007](docs/adr/0007-refinement-types-on-a-pure-core.md)).

**Cutting a release**: set `version` in `Cargo.toml`, merge, then push the tag `v<version>`. The [release workflow](.github/workflows/release.yml) builds both macOS binaries on stable Rust and publishes them to [`sunfmin/homebrew-tap`](https://github.com/sunfmin/homebrew-tap): the archives to its release `afkconvoy-v<version>`, the formula to `Formula/afkconvoy.rb`. Homebrew downloads from the tap because this repository is private. The workflow needs the repository secret `HOMEBREW_TAP_TOKEN`, a token that may write the tap's contents; without it, build both targets, package them, and run `packaging/release.sh publish` from a shell where `gh` may write to the tap.
