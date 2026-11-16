---
status: accepted
---

# A login account's worker starts in a kept home of that account, and a harness with no windows comes after those with

Amends ADR-0087's "Only a Claude Code account is handed to a worker yet" (sunfmin/control#1231).

ADR-0087 handed only a Claude Code account to a worker: its token alone decides the account. Codex, Grok and Kimi Code workers started in the fleet's kept home, which links the host user's own login file (`Harness::kept().links`), so their accounts took no work.

## A worker on a login account starts in a kept home of that account's

- **Which account first, then the home.** A worker's account is chosen, and named in its worktree's note (`convoy-account`), before its start line is made (`Agent::start_worker` takes the line as a closure). A worker never starts in a home made for another account.
- **The account's own kept home.** A worker on a Codex, Grok or Kimi Code account starts in a directory kept for that account: `<harness>[-host-user]@<account>` under the cache's `harness` (Grok, Kimi Code) or under the worktree's fresh directory (Codex). It holds what the fleet's home holds, but its login (`Harness::kept().links`) links the file in the account's home, `~/.agent-accounts/<name>/<file>` (ADR-0086), not the host user's. The host user's skills and rules are linked as before. An account name holds no `@`, so the name says the harness and the account apart.
- **Its sessions are read there.** Whoever reads a worker's turns, windows and limits (`session::roots`, `worker_homes`) reads every directory kept for its harness, the account ones too (`kept::is_kept_name`). A Grok, Kimi Code or Codex account's windows and limits are kept in the account book as a Claude Code account's are, and it rests and comes back the same way.
- **Its line is the command.** The kept home holds the login, so the line holds no credential and fetches none.

## A quota met on an account continues on another account of its harness

A quota an account met rests that account (ADR-0087). Worker health's continuation leaves its harness in, for that one continuation, when another account of the harness takes a worker now; else it leaves the harness out as before.

## A harness that reports no windows comes after those that do

Where the host kit chooses a worker's harness (no checkout names one), its first harness, which the operator's `switch --harness` puts there, is tried first. Of the rest, in kit order, each that reports its account's usage windows (`Harness::reports_windows`: Claude Code, Codex, Kimi Code) comes before each that reports none (Grok, Pi, DSH). Admission judges a harness with no windows by the host alone: its account's use cannot be seen until a limit is met. The host kit's own order is not rewritten. `Kit::range` and `Kit::first` are its one home.

The kit's first is exempt because its order is the only record of the operator's priority: a strict rule would silently override `switch --harness pi`. So an operator who puts Grok first, by `switch` or by hand, gets Grok first. Pi and DSH, which report no windows either, now come after Codex and Kimi Code by default, where they came before them (consulted: the issue states the rule for every harness with no windows; Grok is its example).
