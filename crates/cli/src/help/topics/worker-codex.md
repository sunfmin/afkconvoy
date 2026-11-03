How the fleet starts Codex workers. Shared launch rules are in [`afkconvoy help worker-launch`](help:worker-launch).

## A bound repo

A Codex worker — of a role whose range runs it Codex — is the session you would have at a terminal, not `codex exec`: the fleet sends to a worker after it starts. It starts in its role's directory like any other:

```
cd <the role's copy> && export CODEX_HOME=<the fleet's own>
  <the stage's facts> && <worker launch command> --add-dir <the worktree>
  --dangerously-bypass-hook-trust
  --config 'projects={"<the role's copy>"={trust_level="trusted"}}'
  --config 'project_doc_fallback_filenames=["CLAUDE.md"]'
  --config 'project_root_markers=[]'
  [--config 'developer_instructions="<the repo's own rules, the team rules>"']
  --config 'hooks={PreToolUse=[<what asks afkconvoy guard, and orca's status
    report>], <each other event a session's state changes at>=[<the report>]}'
  --config features.memories=false
  --config skills.bundled.enabled=false
  [--config 'skills.config=[<each skill of yours the role's stands for, off>]']
  [--model <your current Codex selection>]
  [--config 'model_reasoning_effort="<your current effort>"']
  [--config 'service_tier="<your current service tier>"']
  --config 'goals.max_goal_token_budget={const:CODEX_GOAL_TOKENS}'
```

`projects` and `--dangerously-bypass-hook-trust` answer on the line what Codex would ask at its terminal — whether it trusts the folder, and a review of its hooks — and have it load the role's own limits (`.codex/hooks.json`). The next two have it load its role's rules — the CLAUDE.md of the directory it starts in — and those of no directory above. Codex loads rules from no directory added to it, so `developer_instructions` says the other two to it, each as what it is and with its file: the repo's own rules — the worktree's AGENTS.md, or its CLAUDE.md where it holds none — and the team rules (the team's CLAUDE.md), as they stand when the worker starts. A repo and a team with none are told none.

The hook named asks `afkconvoy guard` before every tool call: one the guard refuses, or cannot be asked of, is not made. It is on the line and in no file, so no worker starts without it. The report is the hook orca installed to hear of a Codex session's state (~/.orca/agent-hooks/codex-hook.sh), which orca names in your own hooks and a worker loads none of: it is given ten seconds, and refuses no call, whatever it does; where orca installed none, nothing is told, and a worker's state is read off its terminal.

Every Codex session has a fresh user directory under its worktree's `.afkconvoy-tmp`, so a `/model` choice saved by Codex is private to that session. The directory of a bound worker holds links to your login (auth.json), your skills (skills) and your rules (AGENTS.override.md, AGENTS.md), and no hook or configuration of yours, so no provider you defined in config.toml. Under a team or role that says `host_user: false` it holds your login alone. The shared cache directories harness/codex-host-user and harness/codex still serve listing, research and recovery probes; workers never load their config.toml, including model, effort or service tier left there by older workers. A login Codex keeps in a keyring is in no file, and is no worker's: the start refuses, and `doctor` says so. A bound worker's memories are off.

## Its goal

Every Codex worker, of every role, works under a Codex goal: its brief is typed at it as `/goal <objective>`, which sets the goal and starts its first turn. The objective is the brief pointer every harness is given — the brief's file, never a copy of the task — and one sentence more: mark this goal complete once the brief's outcome stands, the PR landed or the verdict comment posted. While the goal is active, Codex begins the next turn by itself each time its model ends one short of it, and its own goal prompt keeps the whole objective, with no smaller task taken for it. The goal ends in one of four ways:

- the worker marks it complete, its outcome standing;
- the fleet ends it: a worker whose verdict stands while Codex still continues its goal is typed `/goal clear` by the next tick, which lets the turn it is in finish and starts no other, and acts on the verdict once it stops. A worker Codex has not continued since its brief costs no GitHub read for this; a bound worker's declared stage closes its terminal, and its goal with it;
- its model marks it `blocked`, after the same blocker on three goal turns in a row;
- it reaches its budget, `goals.max_goal_token_budget` on the launch line: {const:CODEX_GOAL_TOKENS} tokens, uncached input and output, five times what a Codex worker of this fleet used in the week to 2026-11-03. Codex stops the turn and marks the goal `budget_limited`. It bounds a worker that waits — parked, queued, or on a shell of its own — while Codex continues it. Your own Codex configuration does not set it.

A turn Codex begins on its goal is a turn like any other, and a sign of life: Codex begins it at once (11 ms after the last ended, measured on 2026-11-03). A worker whose goal Codex marked `blocked` or `budget_limited` is a stopped worker short of its task: worker health nudges it once, then hands it to a human, as any other. The goal is read from Codex's store in the session's directory (goals_1.sqlite, through `sqlite3`); where it cannot be read, the worker is judged as one with no goal.

The work account records how each Codex worker's goal went, in its `goal` (`afkconvoy accounts --help`): whether it was set and its objective, its goal turns and those after its verdict, its final status or `cleared`, the tokens it used against its budget, and the session record. A goal that ended `budget_limited`, or took goal turns after its verdict, is a `doctor` finding, `worker's goal misbehaved`.

## Model, effort and service tier

Unless a model label is followed, an automatic Codex worker reads `model` from your `~/.codex/config.toml` at each launch and passes it with `--model` when that value is a nonempty string the host kit lists for Codex. Kit order and research do not replace it. The launch records `user_selection`. A followed model label is recorded as the issue's model label (`model_label`); a launch record that already says `casting` is still read as that. With no such preference the worker starts without `--model` on Codex's native default, recorded as `codex_default`. A value that is not a nonempty string, or that the kit does not list, refuses the start. A model outside the role's range refuses too, whether or not a model label was present and not followed.

Reasoning effort has a different source: every launch reads `model_reasoning_effort` from your own `~/.codex/config.toml` and passes it with `--config`. Changing that file changes the next worker without restarting the fleet. With no setting, Codex uses its selected model's default. This applies even with `host_user: false`. The fleet keeps no effort setting of its own. An unbound worker's fresh user directory holds the same allowed login, skill and rule links, and no other user settings; its `/model` writes change neither your configuration nor another worker's. Existing workers keep the settings they started with.

Service tier follows that same launch-time rule: every bound and unbound worker launch reads an explicitly configured `service_tier`, including `default` and `priority`, from your own `~/.codex/config.toml` and passes only that key with `--config`. This applies regardless of `host_user`. With no explicit setting the launch adds no service-tier argument, leaving the choice to Codex's own defaults. Changing the file changes the next worker without restarting the fleet and does not change an already-running session. The fleet keeps no service-tier setting of its own and copies no unrelated host settings into the session's fresh user directory.

The same model and effort rules hold for a patrol, a consult, a watch, a cut and a landing worker; those worker launches other than a watch also follow the service-tier rule. Model listing and recovery probes remain host capability checks. The watch researches models with its web tools and places its checked research through `afkconvoy kit --research <file>`.

`plan` and `doctor` list started sessions under `codex_workers`: issue, worktree, model, effort, model_source (`user_selection`, `model_label`, or `codex_default`; a stored `casting` is read as `model_label`) and effort_source (your config file and key, or `codex_default`). Each row also names its harness and codex_home, the private session directory. A later edit of your own config does not relabel an existing worker. If `/model` saves a different choice, it is shown with that session's private config as the source. `plan` also gives a Codex frontier issue's next-launch model and source under `codex`. The panel shows each Codex worker's model and effort with the same sources. A malformed Codex configuration file, or a non-string model, effort or service tier, refuses the start and names the file to fix.

## An unbound repo 

A Codex worker of an unbound repo, starts in its worktree with the same allowed user links in a fresh directory, and the same trust and session setup. Nothing is written to your Codex configuration, even when Codex has never seen the worktree:

```
export CODEX_HOME=<this session's directory> && <worker launch command> [--model <your current Codex selection>]
  --dangerously-bypass-hook-trust
  --config 'projects={"<the worktree>"={trust_level="trusted"}}'
  --config 'project_doc_fallback_filenames=["CLAUDE.md"]'
  --config features.memories=false --config skills.bundled.enabled=false
  --config 'goals.max_goal_token_budget={const:CODEX_GOAL_TOKENS}'
```

There is no role or team to add, no stage facts and no fleet guard hook: these workers start where their repo's own rules and skills load. Rule discovery keeps its normal roots, so the worktree's AGENTS.md, or its CLAUDE.md fallback, is read normally. The host user's skills and rules are linked as above. The fleet checks the command's flags before starting these Codex workers too.

## Skill discovery

Codex finds skills in `.agents/skills/` and `.codex/skills/` of the directory it starts in, and not in `.claude/skills/`, where a role's are: the role's copy a Codex worker starts in is Codex's own (<commit>-role+codex-<path>), and holds a link, `.agents/skills` to `../.claude/skills`, made once with the copy. The role's repo holds nothing for it; a role that holds either directory itself is left as it is, and a worker is shown the skills there. `skills.bundled.enabled=false` leaves out the skills Codex carries. Codex shows both of two skills of one name, so a skill of yours named as one of the role's is turned off on the line, by the path of its file (`--config 'skills.config=[{path="<its SKILL.md>",enabled=false}]'`): the role's is the one a worker is shown. One thing of yours a Codex worker loads even under `host_user: false`: the skills of your home (~/.agents/skills). Codex finds them wherever its user directory is and has no way to be told to leave them out, and the fleet does not work around it — so on two hosts such a worker may be shown different skills besides its role's. Keep none there on a host whose Codex workers must be shown their role's alone.

The stock command runs Codex in no sandbox: a worker is limited by the guard, and a host that wants a sandbox says a command of its own. A command you give for Codex must take `--add-dir`, `--dangerously-bypass-hook-trust`, `--config` and `--model`.
