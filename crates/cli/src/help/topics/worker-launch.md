How the fleet starts each harness's worker: the line a worker is started with, what each part of it is for, and what loads from where. Read this to find out why a worker started as it did, or before writing a role's limits for a harness. Where a harness's worker launch command comes from, and what a start checks of it before any worker is started, is `afkconvoy run --help`'s to say; which harness and model a worker runs is its role's range's ([`afkconvoy help roster`](help:roster)).

## Every worker

Each launch reads the latest host kit, including edits made while the fleet instance runs. An explicit command wins. Where a known harness has no entry or its command is empty, the fleet resolves that harness's stock command in the worker's login shell, checks its login and the flags its role needs, and merges the command into the host kit. It reports the discovery as a warning. No model is inferred or probed; a new harness is appended after those held. Discovery installs nothing and logs nobody in. A failing explicit command is reported as given, so fixing it means editing its entry in the host kit.

Every worker is started under `nice` — as fast as before on a host with nothing else to do, and giving way to you and to the gate when it is busy — with its own `TMPDIR`: `/tmp/afkconvoy-<uid>/<16 hex digits>`, a link to `.afkconvoy-tmp` in its worktree, which git ignores and which goes with it. The link is short enough for a socket's path however long the worktree's is — a tool such as sccache binds one under its `TMPDIR` — and is named for the worktree's whole path. It goes with the worktree, but for a server the worker started, which keeps it until it ends: the fleet turns it to its own temporary directory first. An unbound repo's worker launch command also receives its selected model; Codex additionally receives the user directory and effort described below.

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

Codex uses a fresh `CODEX_HOME` for each session. The fleet selects and explicitly passes the model; each launch reads reasoning effort from your own ~/.codex/config.toml, or uses Codex's model default. Saved `/model` choices stay in that session. A train worker or an unbound worker trusts its worktree on the launch line; a role's worker keeps its guard. Launch commands, rules, skills and settings evidence: [`afkconvoy help codex-workers`](help:codex-workers).

A Codex command must take `--add-dir`, `--dangerously-bypass-hook-trust`, `--config` and `--model`.

## Kimi Code

A Kimi Code worker (`harness: kimi`) is an interactive `kimi --auto` session, not `kimi --prompt`: the fleet sends its brief and later messages to that session. It starts in its role's copy, whose `AGENTS.md` links to the role's `CLAUDE.md`:

```
cd <the role's copy> && export KIMI_CODE_TRUST_WORKSPACE=1
  KIMI_CODE_NO_AUTO_UPDATE=1 KIMI_DISABLE_TELEMETRY=1
  KIMI_CODE_HOME=<the fleet's own, for this role> <the stage's facts>
  && <worker launch command> --add-dir <the issue's worktree>
  --add-dir <the team's copy> --skills-dir <the role's copy>/.claude/skills
  [--skills-dir <the fleet's own>/kimi-skills
   --skills-dir <the fleet's own>/agent-skills]
  [--model <its model>]
```

Kimi reads hooks only from its user configuration, so the fleet gives each role its own generated home under harness/kimi or harness/kimi-host-user in the cache. Its `config.toml` keeps only your providers, models and default model, then adds the role's `.kimi-code/limits.toml`, the guard and orca's report. Your credentials directory is linked; your hooks, permissions, MCP servers, plugins and other settings are not inherited. Separate role homes keep concurrent workers from sharing limits. The guard hook turns a missing or failed guard into a refusal too: Kimi's ordinary hook failure would otherwise allow the call. The report uses ~/.orca/agent-hooks/kimi-hook.sh and never refuses a call.

The generated `SYSTEM.md` preserves Kimi's base prompt and adds the repo's own rules and the team rules, each named with its file. The role's rules come from its `AGENTS.md` link. Explicit `--skills-dir` arguments replace automatic skill discovery: the role's are named first, then your ~/.kimi-code/skills and ~/.agents/skills when `host_user` allows them. Your Kimi `AGENTS.md` is linked into that home. Generic ~/.agents resources can also be discovered by Kimi wherever its data home is; keep none there on a host whose workers must have no host-specific instructions.

A command you give for Kimi must take `--add-dir`, `--skills-dir` and `--model`. Its model aliases come from `kimi provider list --json`; probes use `kimi --prompt ... --output-format stream-json` without the stock command's `--auto`, which Kimi rejects in prompt mode.

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
