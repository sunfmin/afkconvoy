How the fleet starts each harness's worker: the line a worker is started with, what each part of it is for, and what loads from where. Read this to find out why a worker started as it did, or before writing a role's limits for a harness. Where a harness's worker launch command comes from, and what a start checks of it before any worker is started, is `afkconvoy run --help`'s to say; which harness and model a worker runs is its role's range's ([`afkconvoy help roster`](help:roster)).

## Every worker

Each launch reads the latest host kit, including edits made while the fleet instance runs. An explicit command wins. Where a known harness has no entry or its command is empty, the fleet resolves that harness's stock command in the worker's login shell, checks its login and the flags its role and worktree-trust setup need, and merges the command into the host kit. It reports the discovery as a warning. No model is inferred or probed; a new harness is appended after those held. Discovery installs nothing and logs nobody in. A failing explicit command is reported as given, so fixing it means editing its entry in the host kit.

Every worker is started under `nice` — as fast as before on a host with nothing else to do, and giving way to you and to the gate when it is busy — with its own `TMPDIR`: `/tmp/afkconvoy-<uid>/<16 hex digits>`, a link to `.afkconvoy-tmp` in its worktree, which git ignores and which goes with it. The link is short enough for a socket's path however long the worktree's is — a tool such as sccache binds one under its `TMPDIR` — and is named for the worktree's whole path. A server the worker started keeps the link after the worktree goes: the fleet first turns it to `retired` beside the links, a private directory independent of any worktree or caller's `TMPDIR`. The link is removed only after a successful process listing finds nothing started with it; a failed listing leaves it for a later check. An unbound repo's worker launch command also receives its selected model and its harness's worktree-trust setup; Codex additionally receives the user directory and effort described below.

Every interactive worker starts already trusting its worktree, in its harness's own way: an issue worker, a role's worker, a consult and a watch. The fleet never answers a folder-trust question at the terminal. One that still appears is a worktree trust gap, reported once with the harness, the complete worker launch command and the worktree. Before closing the terminal, the fleet captures Orca's raw startup wait and screen-read responses, including an unavailable screen or a read failure, and keeps them in the escalation with credential values redacted. Starting the same command again would only repeat the gap.

Every worker, bound or unbound, and the watch carries the asking Terminal input guard. It refuses `orca terminal send` to the asking session's current Terminal, read from its launch record at each call. Outside a role, the guard runs with `--terminal-input-only` and requires no stage facts. Claude Code and Codex receive hooks on the command line, pi receives the fleet's extension, DSH its plugin, and Kimi Code and Grok hooks in a session's own generated home. Outside a role that home is under the worktree's `.afkconvoy-tmp`, and the harness records its turns there, so the fleet reads the session there too. A probe or another session cannot overwrite those hooks. The asking session starts without this guard; the host runner's marked deliveries run outside worker hooks. Input to other Terminals is unchanged.

Before a continuation starts, the fleet closes the old worker's terminals and ends processes left in its worktree's tree, including those that escaped the terminal: SIGTERM first, then SIGKILL after one second. It confirms that the terminals, runtime and process tree are gone within five seconds of closing. A server shared by the host — handed to pid 1 and working outside the worktree — is excluded, as under the hot-worker rules. Each process ended is named in a `start` line of the tick log, with its pid, command and elapsed running time. A process that survives SIGKILL prevents the continuation; a second failed start escalates with that process named.

A worktree trust gap, a worker start that failed again are fleet faults: each escalates at once, skipping the consult even where a stage names one. The escalation and `doctor` say the fix is on the host. Fix the host, then hand the issue back ([`afkconvoy help humans`](help:humans)); another failed start escalates at once again.

A start counts once the harness's own session records that this brief began a turn. Before every line — the brief, a nudge or a rejoin — the fleet requires the harness to be at its input, with no menu blocking it. It sends the line once. Orca's delivery receipt and the terminal screen do not prove the turn: the fleet reads the harness's session for that worktree until the turn is there or the bound passes. A turn already recorded since this worker started counts again for a brief; a nudge counts only a turn that begins with that send. A line the terminal did not accept is a failed delivery, and the session is not read for it. A brief's failed delivery is a failed start: `doctor` names it, and a second failed start hands the issue on with both errors, with the screen's last lines kept as evidence. Later failed deliveries go to worker health; no nudge is recorded until its line starts a turn.

A harness ready once may still be redrawing when its brief's idle check times out. No line was sent: dispatch answers `started: false, brief_pending: true` and keeps the ready terminal under its claim. Each later tick checks that terminal once and hands it the brief once its prompt settles, without another start or a failed-start escalation. A harness whose readiness wait never succeeds still fails its start.

The stage's facts are in the environment of every worker of a bound repo, and a role's limits may read them: AFKCONVOY_ISSUE, AFKCONVOY_FLOW, AFKCONVOY_STAGE, AFKCONVOY_ROLE, AFKCONVOY_SKILL, AFKCONVOY_WRITES, AFKCONVOY_CONSULT, AFKCONVOY_BASE_BRANCH, AFKCONVOY_ESCALATE_LABEL, AFKCONVOY_LOCK_LABEL, AFKCONVOY_ROLE_COMMIT, AFKCONVOY_TEAM_COMMIT and AFKCONVOY_HARNESS. A task with an explicit target also carries AFKCONVOY_TARGET_BRANCH; the guard protects it alongside AFKCONVOY_BASE_BRANCH. A patrol's worker has AFKCONVOY_PATROL in place of the first three. An unbound repo's issue worker has AFKCONVOY_ISSUE too, exported before its worker launch command runs, so its commands name that issue in the tick log and the host's exclusive queue. A watch is on no issue and carries none.

## Claude Code

A Claude Code worker of a bound repo starts in its role's directory, with the rest added to the worker launch command:

```
cd <the role's copy> && export <the stage's facts> && <worker launch command>
  --add-dir <the issue's worktree> --add-dir <the team's copy>
  [--add-dir <the fleet's own, with yours>]
  --setting-sources project --settings <the fleet's> [--model <its model>]
```

The role's rules, skills and hooks load because it starts there. The repo's own rules load from the added worktree, and its hooks do not; the team rules load from the team's copy. Your own skills and rules load too, unless the team file says `host_user: false` ([`afkconvoy help team`](help:team)): the third directory added is the fleet's own (harness/claude-code-host-user under the cache directory), and holds links to your ~/.claude/skills and ~/.claude/CLAUDE.md and nothing else. A skill of the role's wins over one of yours of the same name. Nothing of your settings, hooks, MCP servers or plugins is loaded either way. The settings added hold the guard and orca's status report, as hooks, and the answer to the bypass warning. Every worker exports `CLAUDE_CODE_SANDBOXED=1`, Claude Code's native session trust, before starting: it accepts workspace trust for that session without adding a trust record to your configuration. It does not create an OS sandbox or replace the role's hooks or the guard. Every worker, watch and asking session also exports `CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION=false`, which turns off Claude Code's prompt suggestions: nobody is at its prompt, and the grey text one leaves in the prompt box would read as a person's word to whoever reads its screen. The variable wins over any settings file, and your own ~/.claude/settings.json is left as it is. `--dangerously-skip-permissions` bypasses tool permissions; it does not accept workspace trust. Outside a role that flag and `--settings '{"skipDangerousModePermissionPrompt":true}'`, which answers its unattended warning, are added when absent. In a role the final `--settings` keeps that warning answer beside the fleet hooks, including when the stock command already carries another `--settings`. A command you give must take `--add-dir`, `--setting-sources`, `--settings` and `--model`.

With `host_user: false`, the fleet's settings also exclude ~/.claude/CLAUDE.md and ~/.claude/rules/** through `claudeMdExcludes`. Claude Code can otherwise load those files as project rules while walking above a role's copy under your home, even with user settings disabled. Your login stays accessible in its usual location.

The fleet's Claude Code role copy also holds an empty `.git` directory. It bounds native project skill discovery at the role, so a host profile above the cache cannot supply skills through ancestry when `host_user: false`. It holds no Git metadata or hooks. Opted-in host skills still load from the added directory, and the role's skill keeps its unprefixed name when names collide. Run the opt-in `smoke_claude_skills` test after upgrading Claude Code to verify this native discovery behavior.

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

Codex uses a fresh `CODEX_HOME` for each session. An automatic launch reads your current model preference from `~/.codex/config.toml` unless a model label is followed. Every worker launch reads your reasoning effort and explicit service tier from that file, including `default` and `priority`; with no service tier it adds none and leaves Codex's own default in force. These preferences are read afresh for bound and unbound workers regardless of `host_user`, without copying other host settings or changing an already-running session. A followed label is recorded as the issue's model label. Saved session `/model` choices stay in that session. An unbound worker trusts its worktree on the launch line with `--config 'projects={"<worktree>"={trust_level="trusted"}}'`; a role's worker keeps its guard. Launch commands, rules, skills and settings evidence: [`afkconvoy help worker-codex`](help:worker-codex).

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

Every generated Kimi home carries `.skip-migration-from-kimi-cli`, including each fresh role copy's home: Kimi starts without offering to migrate the host's kimi-cli configuration.

The generated `SYSTEM.md` preserves Kimi's base prompt and adds the repo's own rules and the team rules, each named with its file. The role's rules come from its `AGENTS.md` link. Explicit `--skills-dir` arguments replace automatic skill discovery: the role's are named first, then your ~/.kimi-code/skills and ~/.agents/skills when `host_user` allows them. Your Kimi `AGENTS.md` is linked into that home. Generic ~/.agents resources can also be discovered by Kimi wherever its data home is; keep none there on a host whose workers must have no host-specific instructions.

A command you give for Kimi must take `--add-dir`, `--skills-dir` and `--model`. Its model aliases come from `kimi provider list --json`; recovery probes use `kimi --prompt ... --output-format stream-json` without the stock command's `--auto`, which Kimi rejects in prompt mode.

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

## Grok

A Grok worker (`harness: grok`) is an interactive `grok --always-approve` session. It starts in the issue's worktree. Grok 1.0.60 and later reject `--add-dir` (`additionalDirectories`: a session works only in its cwd), so the fleet does not pass it and does not start the worker in the role's copy. The role's `CLAUDE.md` and `.claude/skills` stay where every harness writes them. The rules are told in the generated home, and the skills directory is named there:

```
cd <the issue's worktree> && export GROK_HOME=<the fleet's own, for this role>
  GROK_MEMORY=0 <Claude and Cursor hook and MCP compatibility off> <the stage's facts>
  && <worker launch command> --trust [--model <its model>]
```
`--trust` is on the launch line, so the fleet writes no trusted-folders file and nobody answers a trust question. On Grok 1.0.70 `--help` does not name `--trust`, and the flag check does not require it to. A question that still appears is reported with the command and the worktree. A project `.grok/config.toml` loads denied permissions and does not load hooks, so each role gets a generated home (harness/grok, or harness/grok-host-user) whose config holds the role's hooks and denies, the guard and orca's report. Claude and Cursor hook and MCP compatibility are off, so `.claude/settings.json` and `.cursor/hooks.json` are not a second set of limits. Your `auth.json` is linked; your hooks, MCP servers, plugins, permission mode and memories are not. The guard's `PreToolUse` hook turns a missing or failed guard into exit 2, which is how Grok denies, and a deny the role holds still applies under `--always-approve`. The report uses ~/.orca/agent-hooks/grok-hook.sh and exits 0.
The role's limits are `.grok/config.toml`: a hook, or a permission it denies. A file that names no hook and denies nothing holds no limit. The role's rules, the repo's own rules and the team rules are written to the home's `rules/told.md`. The role's `.claude/skills` is named in `[skills] paths`. Your skills and rules are given unless `host_user: false`; with it on, `~/.grok/rules` is named, Claude skill compatibility stays on, and a skill of yours whose name is one of the role's is ignored so the role's copy is the one that loads. An unbound worker or a watch is the stock command plus `--trust` and `--model` when a model is set, with no role or stage limits and with the asking Terminal input guard in its generated home. Listing asks the worker launch command for `models` and sends no prompt; the id marked `*` and `(default)` is the one workers start on until research places one. A probe uses `-p` and `--output-format json`, and the model that answered is the key of `modelUsage`. Not logged in, or a listing that names no model, is `grok login`. A command you give must take `--model`.

## Where to read on

- `afkconvoy run --help`: where each worker launch command comes from, and what a start checks of it
- [`afkconvoy help team`](help:team): where a role holds its limits for each harness, and `host_user` and `dsh_profile`
- `afkconvoy guard --help`: what the guard refuses, and how each harness asks it
- [`afkconvoy help cache`](help:cache): where the cache directory is, which holds the fleet's own directories named here
- [`afkconvoy help worker-proxy`](help:worker-proxy): which hosts a worker reaches past the proxy
