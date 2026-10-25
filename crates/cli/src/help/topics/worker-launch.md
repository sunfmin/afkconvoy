How the fleet starts each harness's worker: the line a worker is started with, what each part of it is for, and what loads from where. Read this to find out why a worker started as it did, or before writing a role's limits for a harness. Where a harness's worker launch command comes from, and what a start checks of it before any worker is started, is `afkconvoy run --help`'s to say; which harness and model a worker runs is its role's range's ([`afkconvoy help roster`](help:roster)).

## Every worker

Each launch reads the latest host kit, including edits made while the fleet instance runs. An explicit command wins. Where a known harness has no entry or its command is empty, the fleet resolves that harness's stock command in the worker's login shell, checks its login and the flags its role needs, and merges the command into the host kit. It reports the discovery as a warning. No model is inferred or probed; a new harness is appended after those held. Discovery installs nothing and logs nobody in. A failing explicit command is reported as given, so fixing it means editing its entry in the host kit.

Every worker is started under `nice` — as fast as before on a host with nothing else to do, and giving way to you and to the gate when it is busy — with its own `TMPDIR`: `/tmp/afkconvoy-<uid>/<16 hex digits>`, a link to `.afkconvoy-tmp` in its worktree, which git ignores and which goes with it. The link is short enough for a socket's path however long the worktree's is — a tool such as sccache binds one under its `TMPDIR` — and is named for the worktree's whole path. It goes with the worktree, but for a server the worker started, which keeps it until it ends: the fleet turns it to its own temporary directory first. Nothing else of an unbound repo's worker launch command is touched.

The stage's facts are in the environment of every worker of a bound repo, and a role's limits may read them: AFKCONVOY_ISSUE, AFKCONVOY_FLOW, AFKCONVOY_STAGE, AFKCONVOY_ROLE, AFKCONVOY_SKILL, AFKCONVOY_WRITES, AFKCONVOY_CONSULT, AFKCONVOY_BASE_BRANCH, AFKCONVOY_ESCALATE_LABEL, AFKCONVOY_LOCK_LABEL, AFKCONVOY_ROLE_COMMIT, AFKCONVOY_TEAM_COMMIT and AFKCONVOY_HARNESS. A patrol's worker has AFKCONVOY_PATROL in place of the first three.

## Claude Code

A Claude Code worker of a bound repo starts in its role's directory, with the rest added to the worker launch command:

```
cd <the role's copy> && export <the stage's facts> && <worker launch command>
  --add-dir <the issue's worktree> --add-dir <the team's copy>
  [--add-dir <the fleet's own, with yours>]
  --setting-sources project --settings <the fleet's> [--model <its model>]
```

The role's rules, skills and hooks load because it starts there. The repo's own rules load from the added worktree, and its hooks do not; the team rules load from the team's copy. Your own skills and rules load too, unless the team file says `host_user: false` ([`afkconvoy help team`](help:team)): the third directory added is the fleet's own (harness/claude-code-host-user under the cache directory), and holds links to your ~/.claude/skills and ~/.claude/CLAUDE.md and nothing else. A skill of the role's wins over one of yours of the same name. Nothing of your settings, hooks, MCP servers or plugins is loaded either way. The settings added hold the guard and orca's status report, as hooks, and the answer to the bypass warning. A command you give must take `--add-dir`, `--setting-sources`, `--settings` and `--model`.

## pi

A pi worker — of a role whose range runs it pi — starts in its role's directory like any other, with what pi does not load from there added:

```
cd <the role's copy> && export PI_CODING_AGENT_DIR=<the fleet's own>
  <the stage's facts> && <worker launch command> --approve --no-skills
  --skill <the role's copy>/.claude/skills [--skill <the fleet's own>/skills]
  --extension <the fleet's own>/afkconvoy.js [--model <its model>]
```

`--approve` answers on the line what pi would ask at its terminal, and has it load the role's own limits (`.pi/extensions/`). The role's skills are named, then yours, and pi looks for no other; of two skills of one name pi keeps the first, the role's. pi's user directory is the fleet's own, not yours (~/.pi/agent): it holds links to your logins and to the models you defined (auth.json, models.json), a link to the extension orca installed to hear of a pi session's state, your skills and rules (skills, AGENTS.override.md, AGENTS.md, CLAUDE.md), and nothing else of yours — no setting or other extension. Under a team file that says `host_user: false` it is another (harness/pi under the cache directory, not harness/pi-host-user), with none of your skills and rules, and yours are not named. The extension named is afkconvoy's own: it asks `afkconvoy guard` before every tool call, refusing one the guard refuses or cannot be asked of, and loads the rules of the issue's worktree (its AGENTS.md, or CLAUDE.md) and the team rules (the team's CLAUDE.md). A command you give for pi must take `--approve`, `--no-skills`, `--skill`, `--extension` and `--model`.

A worker's model is its harness's own name for it: pi's is `<provider>/<id>`. A worker on pi with none gets whichever model pi picks for itself. A provider whose key is in your login shell's environment is reached too, and a rules file in a directory above the fleet's copies (~/AGENTS.md) is one pi loads.

## Codex

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

Codex finds skills in `.agents/skills/` and `.codex/skills/` of the directory it starts in, and not in `.claude/skills/`, where a role's are: the role's copy a Codex worker starts in is Codex's own (<commit>-role+codex-<path>), and holds a link, `.agents/skills` to `../.claude/skills`, made once with the copy. The role's repo holds nothing for it; a role that holds either directory itself is left as it is, and a worker is shown the skills there. `skills.bundled.enabled=false` leaves out the skills Codex carries. Codex shows both of two skills of one name, so a skill of yours named as one of the role's is turned off on the line, by the path of its file (`--config 'skills.config=[{path="<its SKILL.md>",enabled=false}]'`): the role's is the one a worker is shown. One thing of yours a Codex worker loads even under `host_user: false`: the skills of your home (~/.agents/skills). Codex finds them wherever its user directory is and has no way to be told to leave them out, and the fleet does not work around it — so on two hosts such a worker may be shown different skills besides its role's. Keep none there on a host whose Codex workers must be shown their role's alone.

The stock command runs Codex in no sandbox: a worker is limited by the guard, and a host that wants a sandbox says a command of its own. A command you give for Codex must take `--add-dir`, `--dangerously-bypass-hook-trust`, `--config` and `--model`.

## DeepSeek Harness

A DeepSeek Harness worker (`harness: dsh`) runs interactive `dsh-tui`, with `dsh` on PATH too, on the DSH profile its team or role names: `dsh_profile: fleet` (default) or `host-user` ([`afkconvoy help team`](help:team)). The stock command is `dsh-tui`; a host may name the `dsh` command in its host kit's file, which `afkconvoy kit` keeps and which must accept leading `--patch`. It starts in its role's copy, with the stage facts and AFKCONVOY_HARNESS=dsh, DSH_HOME set to harness/dsh-host-user under the fleet's cache (harness/dsh with host_user: false), and the issue's worktree as the TUI's path argument:

```
cd <the role's copy> && export DSH_HOME=<the fleet's own, or yours>
  DSH_AGENTS_HOME=<the fleet's own, or yours> <what the fleet's plugin reads>
  <the stage's facts> && <worker launch command>
  [--patch <the fleet's own>/fleet.patch.yml]
  [--patch <the role's copy>/.dsh/cordis.patch.yml]
  --patch <the fleet's own>/afkconvoy.patch.yml <the issue's worktree>
```

The words after the worker launch command are added by a script of the fleet's own (start.sh, in the fleet's own), which refuses the worker where the files of the guard are missing there.

The fleet's DSH_HOME loads the stock base and TUI bundles, the role's own .dsh/cordis.patch.yml where present, and the fleet's --patch overlay last. Only credentials (.credentials.yaml), skills and rules (skills, AGENTS.md) are linked from your DSH_HOME (default ~/.dsh), beside the skills and AGENTS.md of DSH_AGENTS_HOME (default ~/.agents). The role's native skill provider ranks its canonical skills ahead of these user roots. Dependency resolution uses your installed dsh-tui dependencies; its manifest and patches, and your home patches, MCP servers and plugins, are never loaded or edited. The first patch, fleet.patch.yml, is the fleet's profile's alone: DSH asks for no approval and shuts a worker's commands in no sandbox (DSH_PERMISSION_MODE=danger-full-access), and a worker is limited by the guard.

A team or role may instead choose `dsh_profile: host-user` (independent of host_user): DSH_HOME stays the host user's (default ~/.dsh), DSH_AGENTS_HOME stays theirs (default ~/.agents), and dsh-tui loads their manifest, dependencies, plugins, MCP servers, theme, settings and both profile and home patches. This can differ across hosts and the profile may itself load user skills and rules even with host_user: false. The fleet writes no file there: its plugin stays in its cache and its --patch overlay is still last, after the role's limits, with required guard dependencies. User settings cannot disable it. Neither profile edits the user's DSH_HOME; both keep the role's limits.

The --patch overlay, applied last on either profile, loads the guard and orca state plugin. The fleet's plugin supplies role, repo and team rules, role skills before yours, and the worker's model as written. It asks `afkconvoy guard` before every tool call and reports running/idle, including interruption, to orca. The TUI and agent loop require the guard plugin's loader service, so a missing or broken plugin starts neither. Ctrl+C interrupts the running turn and leaves the interactive session open.

## Where to read on

- `afkconvoy run --help`: where each worker launch command comes from, and what a start checks of it
- [`afkconvoy help team`](help:team): where a role holds its limits for each harness, and `host_user` and `dsh_profile`
- `afkconvoy guard --help`: what the guard refuses, and how each harness asks it
- [`afkconvoy help cache`](help:cache): where the cache directory is, which holds the fleet's own directories named here
- [`afkconvoy help worker-proxy`](help:worker-proxy): which hosts a worker reaches past the proxy
