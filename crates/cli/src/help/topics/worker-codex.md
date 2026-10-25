How the fleet starts Codex workers, loads their rules and skills, and sets up trust and hooks. For what every worker shares, and the flags a worker launch command must accept, read [`afkconvoy help worker-launch`](help:worker-launch).

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
  [--model <the stage's>]
```

`projects` and `--dangerously-bypass-hook-trust` answer on the line what Codex would ask at its terminal — whether it trusts the folder, and a review of its hooks — and have it load the role's own limits (`.codex/hooks.json`). The next two have it load its role's rules — the CLAUDE.md of the directory it starts in — and those of no directory above. Codex loads rules from no directory added to it, so `developer_instructions` says the other two to it, each as what it is and with its file: the repo's own rules — the worktree's AGENTS.md, or its CLAUDE.md where it holds none — and the team rules (the team's CLAUDE.md), as they stand when the worker starts. A repo and a team with none are told none.

The hook named asks `afkconvoy guard` before every tool call: one the guard refuses, or cannot be asked of, is not made. It is on the line and in no file, so no worker starts without it. The report is the hook orca installed to hear of a Codex session's state (~/.orca/agent-hooks/codex-hook.sh), which orca names in your own hooks and a worker loads none of: it is given ten seconds, and refuses no call, whatever it does; where orca installed none, nothing is told, and a worker's state is read off its terminal.

Codex's user directory is the fleet's own (harness/codex-host-user under the cache directory), not yours (~/.codex): it holds links to your login (auth.json), your skills (skills) and your rules (AGENTS.override.md, AGENTS.md), and nothing else of yours — no hook or configuration, so no provider you defined in config.toml. Under a team file that says `host_user: false` it is another (harness/codex), with your login alone. A login Codex keeps in a keyring is in no file, and is no worker's: the start refuses, and `doctor` says so. Its memories are off.

## An unbound repo or the train worker

A Codex train worker, or any Codex worker of an unbound repo, starts in its worktree with the same user directory (harness/codex-host-user), trust and session setup. Nothing is written to your Codex configuration, even when Codex has never seen the worktree:

```
export CODEX_HOME=<the fleet's own> && <worker launch command> [--model <the host kit's>]
  --dangerously-bypass-hook-trust
  --config 'projects={"<the worktree>"={trust_level="trusted"}}'
  --config 'project_doc_fallback_filenames=["CLAUDE.md"]'
  --config features.memories=false --config skills.bundled.enabled=false
```

There is no role or team to add, no stage facts and no fleet guard hook: these workers start where their repo's own rules and skills load. Rule discovery keeps its normal roots, so the worktree's AGENTS.md, or its CLAUDE.md fallback, is read normally. The host user's skills and rules are linked as above. The fleet checks the command's flags before starting these Codex workers too.

## Skill discovery

Codex finds skills in `.agents/skills/` and `.codex/skills/` of the directory it starts in, and not in `.claude/skills/`, where a role's are: the role's copy a Codex worker starts in is Codex's own (<commit>-role+codex-<path>), and holds a link, `.agents/skills` to `../.claude/skills`, made once with the copy. The role's repo holds nothing for it; a role that holds either directory itself is left as it is, and a worker is shown the skills there. `skills.bundled.enabled=false` leaves out the skills Codex carries. Codex shows both of two skills of one name, so a skill of yours named as one of the role's is turned off on the line, by the path of its file (`--config 'skills.config=[{path="<its SKILL.md>",enabled=false}]'`): the role's is the one a worker is shown. One thing of yours a Codex worker loads even under `host_user: false`: the skills of your home (~/.agents/skills). Codex finds them wherever its user directory is and has no way to be told to leave them out, and the fleet does not work around it — so on two hosts such a worker may be shown different skills besides its role's. Keep none there on a host whose Codex workers must be shown their role's alone.

The stock command runs Codex in no sandbox: a worker is limited by the guard, and a host that wants a sandbox says a command of its own.
