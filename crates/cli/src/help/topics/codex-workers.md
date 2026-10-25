How the fleet starts Codex workers. Shared launch rules are in [`afkconvoy help worker-launch`](help:worker-launch).

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
  --model <the fleet's selected model>
  [--config 'model_reasoning_effort="<your current effort>"']
```

`projects` and `--dangerously-bypass-hook-trust` answer on the line what Codex would ask at its terminal — whether it trusts the folder, and a review of its hooks — and have it load the role's own limits (`.codex/hooks.json`). The next two have it load its role's rules — the CLAUDE.md of the directory it starts in — and those of no directory above. Codex loads rules from no directory added to it, so `developer_instructions` says the other two to it, each as what it is and with its file: the repo's own rules — the worktree's AGENTS.md, or its CLAUDE.md where it holds none — and the team rules (the team's CLAUDE.md), as they stand when the worker starts. A repo and a team with none are told none.

The hook named asks `afkconvoy guard` before every tool call: one the guard refuses, or cannot be asked of, is not made. It is on the line and in no file, so no worker starts without it. The report is the hook orca installed to hear of a Codex session's state (~/.orca/agent-hooks/codex-hook.sh), which orca names in your own hooks and a worker loads none of: it is given ten seconds, and refuses no call, whatever it does; where orca installed none, nothing is told, and a worker's state is read off its terminal.

Every Codex session has a fresh user directory under its worktree's `.afkconvoy-tmp`, so a `/model` choice saved by Codex is private to that session. The directory of a bound worker holds links to your login (auth.json), your skills (skills) and your rules (AGENTS.override.md, AGENTS.md), and no hook or configuration of yours, so no provider you defined in config.toml. Under a team or role that says `host_user: false` it holds your login alone. The shared cache directories harness/codex-host-user and harness/codex still serve discovery and probes; workers never load their config.toml, including model or effort left there by older workers. A login Codex keeps in a keyring is in no file, and is no worker's: the start refuses, and `doctor` says so. A bound worker's memories are off.

The fleet selects the model by host kit order, role eligibility and casting, and passes it explicitly with `--model` for both bound and unbound repos. No config.toml selects a worker's model. A kit entry with no Codex models starts no Codex session: `afkconvoy kit` lists and probes them now, and the next tick also repairs such an entry without waiting for the daily refresh, with an hour between unsuccessful tries. Discovery filters the server catalog to model names before it reaches terminal scrollback.

Reasoning effort has a different source: every launch reads `model_reasoning_effort` from your own `~/.codex/config.toml` and passes it with `--config`. Changing that file changes the next worker without restarting the fleet. With no setting, Codex uses its selected model's default. This applies even with `host_user: false`. The fleet keeps no effort setting of its own. An unbound worker's fresh user directory holds the same allowed login, skill and rule links, and no other user settings; its `/model` writes change neither your configuration nor another worker's. Existing workers keep the settings they started with.

The same model and effort rules hold for a train worker, a patrol, a consult, an inspection and the caster's headless Codex call. The caster uses a fresh login-only directory and reads the user's effort when it makes that call. Model-discovery probes and model research remain host capability checks rather than worker sessions.

`plan` and `doctor` list started sessions under `codex_workers`: issue, worktree, model, effort, model_source (`fleet_selection`) and effort_source (your config file and key, or `codex_default`). Each row also names its harness and codex_home, the private session directory. A later edit of your own config does not relabel an existing worker. If `/model` saves a different choice, it is shown with that session's private config as the source. `plan` also gives a Codex frontier issue's next-launch settings under `codex`. The panel shows each Codex worker's model and effort with the same sources. A malformed Codex configuration file or a non-string effort refuses the start and names the file to fix.

A Codex train worker, or any Codex worker of an unbound repo, starts in its worktree with the same allowed user links in a fresh directory, and the same trust and session setup. Nothing is written to your Codex configuration, even when Codex has never seen the worktree:

```
export CODEX_HOME=<this session's directory> && <worker launch command> --model <the fleet's selected model>
  --dangerously-bypass-hook-trust
  --config 'projects={"<the worktree>"={trust_level="trusted"}}'
  --config 'project_doc_fallback_filenames=["CLAUDE.md"]'
  --config features.memories=false --config skills.bundled.enabled=false
```

There is no role or team to add, no stage facts and no fleet guard hook: these workers start where their repo's own rules and skills load. Rule discovery keeps its normal roots, so the worktree's AGENTS.md, or its CLAUDE.md fallback, is read normally. The host user's skills and rules are linked as above. The fleet checks the command's flags before starting these Codex workers too.

Codex finds skills in `.agents/skills/` and `.codex/skills/` of the directory it starts in, and not in `.claude/skills/`, where a role's are: the role's copy a Codex worker starts in is Codex's own (<commit>-role+codex-<path>), and holds a link, `.agents/skills` to `../.claude/skills`, made once with the copy. The role's repo holds nothing for it; a role that holds either directory itself is left as it is, and a worker is shown the skills there. `skills.bundled.enabled=false` leaves out the skills Codex carries. Codex shows both of two skills of one name, so a skill of yours named as one of the role's is turned off on the line, by the path of its file (`--config 'skills.config=[{path="<its SKILL.md>",enabled=false}]'`): the role's is the one a worker is shown. One thing of yours a Codex worker loads even under `host_user: false`: the skills of your home (~/.agents/skills). Codex finds them wherever its user directory is and has no way to be told to leave them out, and the fleet does not work around it — so on two hosts such a worker may be shown different skills besides its role's. Keep none there on a host whose Codex workers must be shown their role's alone.

The stock command runs Codex in no sandbox: a worker is limited by the guard, and a host that wants a sandbox says a command of its own. A command you give for Codex must take `--add-dir`, `--dangerously-bypass-hook-trust`, `--config` and `--model`.
