---
status: accepted
---

# The harness crate may be depended on by another program

Amends ADR-0013 and ADR-0044 (sunfmin/control#694).

What afkconvoy knows of each harness — how one starts, where it reads its rules and skills, how a hook reaches it and is answered, where it keeps its session and which of its lines start or end a turn, what its usage lines count, which screen words mean a quota, a capacity limit or a refusal — is wanted by another program of this host's user (Guild, sunfmin/control#693), which uses none of afkconvoy's team, role or workflow. The host kit is the one place that knows the harnesses (control ADR 0009): a second copy of that knowledge would drift from the first.

That knowledge is its own crate, `crates/harness` (`afkconvoy-harness`), which `afkconvoy-core` depends on and re-exports as `harness`, `turn`, the token usage of `account`, the shell `Line`, the guard's `Call` and the team's `RULES`, `SKILLS` and `SKILL_FILE`. Another program depends on it as a git dependency pinned to a revision of this repo; it is not published.

Its API stays free of fleet concepts: no team, role, stage, issue, claim, patrol, intake or tracker type appears in it, and it does not depend on `afkconvoy-core`. A session is started from a neutral `Start`: the directory it starts in, the worktree, a directory whose rules and skills are added, the model, the hook run before each tool call, the directory kept for the harness, the rules it is told and the variables of its environment. afkconvoy-core maps a worker's start in its role onto it (`role::Start::in_role`), putting the stage's facts, the harness and the model in that environment, in the order they always were. What the crate names by itself is what a hook or a session of a harness reads: the variable naming the harness a hook runs in, and the brief pointer by which a session is found to be the one handed a brief.

The crate keeps its Flux refinements (ADR-0007). `./verify.sh` checks it as the core's dependency, at the Flux revision the workspace pins once, and the gate holds it, as the core, to no function taken on trust.

## Considered options

- **Let the other program depend on afkconvoy-core.** Refused: the harness modules took the role's start, the team's DSH profile and the team's rule and skill paths, so a caller would build a team's values to start a session, and every change to the fleet's types would reach it.
- **Copy the harness modules into the other program.** Refused: two places would know each harness, and the copy would drift (ADR-0044's reason, across repos).
- **Publish the crate.** Not needed: a git dependency pinned by revision is reproducible, and nothing outside this host's user depends on it.

## Consequences

- A flag, a path, a hook event or a session format of one harness is still spelled only in its module, now of the harness crate (ADR-0044).
- A change to the crate's API is a change to another program's dependency: it stays neutral, and a fleet concept a start needs is put in `Start::env` or mapped by the core, not named in the crate.
- A test of the crate from outside builds a start line and reads a recorded session's turn and usage for Claude Code and Codex through its public API alone, and checks that the crate depends on nothing of the fleet.
- Parsing a quota's reset time, Claude Code's quota words and window prediction are for the other program to add to this crate, in their own issues.
- For the other program's soft stop (sunfmin/control#734), the crate says per harness what a hook before a call answers to end the session's turn, its session kept (`Harness::stop_answer`), whether the harness is known to honour it (`stop_reliable`: Claude Code alone), the line that continues that session in its role with a new instruction (`resumed`) and the session's id read off its file (`session_id`). afkconvoy's own workers use none of it.
