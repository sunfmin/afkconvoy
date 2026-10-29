---
status: accepted
---

# Worker health is judged by Jev or by the operator

On 25–26 October 2026 a Codex worker stopped at once on `■ Usage limit reached` (sunfmin/control#74). For twenty minutes every worker-health check answered that no harness had a tool-free diagnosis launch, or that one's answer was not JSON. After three attempts the fleet waited for new evidence, and nothing came until the operator switched the worker by hand (sunfmin/control#80). A harness diagnosis can fail three ways that a typed model cannot: no tool-free launch, an answer that is not JSON, and a quota shared with the worker it is diagnosing.

Rules still class first: structured error codes and Codex's explicit usage limit are classed by rule, with or without a key.

What the rules cannot class goes to Jev, TypeSafe's System One model, only where the host settings name a **Jev key command** (`run --jev-key-command`, or `AFKCONVOY_JEV_KEY_COMMAND`). The command runs at each judgment, and its stdout is the key. The key reaches only curl's standard input and TypeSafe; it is in no argument, file, answer or log line. One request per check carries the worker's latest screen as its state. It asks one Noul per judgment and one Choice that selects the evidence line from the screen's lines. `afkconvoy_core::worker_health` holds the questions, their criteria, the thresholds (0.7 yes, 0.3 no), the structured codes the rules recognize, and the one table that gives each class its scope and action, whether a rule or Jev's answers settled the class (sunfmin/control#211). The effect layer supplies the evidence and carries the decision out; it keeps no table of its own. Its tests replay answers Jev gave on recorded real screens. The tick log holds every request and its answers, so any judgment can be sent again. No harness is started to diagnose.

The worker waits for the operator:

- without a key command;
- where the command fails or prints nothing;
- where the call fails or its answer is malformed;
- where Jev leaves the table unsettled.

None of these counts as an answer. `doctor` names the worker as waiting for the operator, with the reason and the last line of its screen, so the watch starts on it. The wait checks again every five minutes and keeps the one-hour recovery window: after an hour on unchanged evidence the issue is escalated with that evidence.

## Considered options

- **Keep the harness diagnosis as a fallback without a key.** It is the path that failed above, and a fallback that rarely runs is the one nobody notices broken. Without a key the operator is asked at once.
- **The key in an environment variable or in the host settings.** The host runner takes its environment from a login shell, so the key would live in a shell rc file. `doctor` prints the host settings as they are said. A command keeps the key with the tool that keeps secrets.
- **Wait for the operator indefinitely.** Nothing would escalate a worker the operator never sees. The one-hour window is kept, as ADR-0020 keeps it for every other wait.

## Consequences

- Supersedes, of ADR-0020's worker-health paragraphs, the tool-free diagnosis by another kit harness, its 30 seconds per candidate, three attempts per episode and 300 seconds between them, and that the source records which harness answered. The rules, scopes, command identity, the one-hour recovery window and the hourly recovery probes stand.
- The `diagnose_worker` example is gone. `docs/verification/control-19-diagnosis.json` records what it found then.
