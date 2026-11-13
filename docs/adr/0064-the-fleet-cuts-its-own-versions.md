---
status: accepted
---

# The fleet cuts its own versions, not a human's session

Formerly the control repo's. How a cut runs is ADR-0028's; this is who cuts.

A repo the fleet lands work in and whose versions are installed through Homebrew is cut by its fleet, and by no human's session. Only the fleet knows when it is idle or when its oldest landing past the newest version tag has waited past the repo's age bound, and at those times nobody is usually at the keyboard: on this repo #451–#454 had all landed by 2026-10-25 01:48Z and no version was cut for over an hour, because no session noticed. A repo turns cutting on in its config's cut block, which holds its own steps (`docs/agents/afkconvoy.md`, "Cutting a version"). A cut that fails opens one issue, which holds no later cut back (ADR-0072). This concept is called a **cut** because "release" already names a claim's last step.

Once a newer afkconvoy is installed, the host runner moves onto it by itself between ticks, without waiting for every fleet to go idle (ADR-0032).

Sources: #466, #467, #468; sunfmin/control#106 (the age bound, so a busy fleet still cuts), sunfmin/control#88 (the host runner moving without waiting for idle).

## Considered options

- **A cut issue given to the fleet.** A worker only lands through a turn and pushes nothing to the base branch, and the issue would hold a claim of the very fleet whose idleness a cut waits on. Refused.
- **The control repo's session cuts**, as an exception to its being read-only to project checkouts. It cuts only once the operator opens a session, and guesses from outside whether the fleet is idle. Refused.

## Consequences

- The operator's global rules say a Homebrew repo is cut by its fleet and never by hand, and a failed cut is read on its escalate issue. Those rules are the operator's own to change; no fleet reaches them.
- Each other Homebrew repo turns cutting on in its own cut block: sunfmin/inspectvideo (sunfmin/control#6) and sunfmin/hkj (sunfmin/control#7). sunfmin/agent-cli is not installed through Homebrew and has no version tags, so it does not.
- What turns cutting on must not make an older afkconvoy refuse the config: an unknown key is an error, so a new key would stop a fleet still running the older release. It is a fenced block of its own, which an older afkconvoy skips, as the gate budget's is (`docs/agents/afkconvoy.md`).
