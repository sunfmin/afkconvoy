# The tool runs its own loop and asks no one anything

In afk-fleet the loop is an LLM session that calls `afk cycle`, carries an opaque state between calls and answers four kinds of judgment. Every one of those judgments disappears with something else afkconvoy cuts — `no_checks` and `adversarial_verify` with the checks gate and the verify (ADR-0004), `reason` with the retry ladder (ADR-0005) — or becomes a rule code can apply (`empty_diff`). With nothing left to judge, `afkconvoy run` is a plain long-running process: tick, sleep, tick.

## Consequences

- No state crosses a process boundary: the instance id and the worker launch command live in the process, and die with it.
- A wake cannot be a line typed at a session; it is `afkconvoy wake`, which signals the `run` process of this host for this repo.
- Anything that would need judgment in future has no one to ask. It must be a rule, or an escalation.
