---
status: accepted
---

# A watch runs interactively and reports by command

Amends ADR-0030, ADR-0033, ADR-0035 and ADR-0036 (sunfmin/control#118).
The watch runs the harness's interactive session in its watch directory,
trusted and with prompts skipped, in a terminal titled “afkconvoy watch”. Its
brief arrives as the first message through the same delivery protocol a
worker uses. A human may see it work and type to it.

On 26 October 2026 a watch continued from Codex at capacity to Claude Code
in print mode. Its transcript showed hundreds of tool calls while its
terminal and output log remained empty for minutes. A human reasonably read
that silence as a stuck watch. The operator must be visible and reachable
in the same way a worker is.

The last line of each brief is the exact `afkconvoy watched --watch <id>
--text-file <report.json>` command. Its report requires five parts: what held
work back, each action, what was verified, each human decision left alone
with its question and answer line, and what is pending. `doctor.host.watch`
and the next brief carry this record. Printed output is terminal evidence,
never a report or proof of recovery. A report is written atomically, once;
submitting the same record again is harmless.

After reporting, the host runner ends only that watch session's harness and
its descendants. The terminal and screen remain until another watch starts
in that directory. The operator's other sessions stay open. A watch reaching
its hour without reporting is ended and explicitly shown as not reported;
a successful harness exit alone is also not a report.

Worker health observes the watch's own terminal. A stopped session without
an outcome is nudged. A usage limit, capacity stop, refusal or failed exit
ends that session before starting the next eligible harness in the same
terminal slot. Its brief carries the same look and the earlier session's
terminal evidence. Continuations share the original hour and directory;
there is still at most one watch on the host. Authority, human bounds,
triggers and episode coverage are unchanged by the terminal surface.

## An unreported watch leaves its work eligible

Amends ADR-0030's once-per-episode rule (sunfmin/control#138). Starting a
watch reserves the work while it runs; its explicit report keeps that
episode covered afterwards. A terminal closed without a report, an expired
watch, a harness exiting without reporting, or exhausted harness choices
must not permanently consume the trigger. The next scheduled look that
still finds the condition starts another watch when a harness is available.
The watch interval bounds these retries; a running watch still prevents a
second one, and a reported human decision is not repeatedly dispatched.

The host runner reconciles coverage against retained watch outcomes before
choosing new work. This repairs failed episodes left covered by an older
runner after an upgrade or restart. The latest outcome for an item wins:
an earlier unsuccessful watch cannot undo a later report. An unreported
research watch also releases its daily research reservation. Earlier failed
attempts and their missing reports remain in the next brief.

Felix's clarification on sunfmin/control#138 keeps workers and watches in
interactive Orca terminals. The defect is missed automatic recovery, not
the choice of chat versus terminal.
