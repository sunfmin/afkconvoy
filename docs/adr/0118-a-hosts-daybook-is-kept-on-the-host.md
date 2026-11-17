---
status: accepted
---

# A host's daybook is kept on the host

Amends ADR-0065 and ADR-0071. Decided by Felix, control session 2026-11-17.

The daybook was the Discussions of the host's tracker: one discussion a day, one comment an entry. With a host's issues in its own local tracker (ADR-0091) a host has no GitHub tracker to hold its Discussions, and every entry was a GitHub write that spent quota, could fail in transit, and needed a queue to be written again (`watch/daybook.json`) and a rule for two writers opening the same day's discussion at once.

So afkconvoy keeps a host's daybook on the host: `daybook.db`, one SQLite database in the host's data directory, beside `issues.db` and `host.db`. An entry is a row: its day, when it was written, and its text in the shape ADR-0071 gives it. A day is the entries of that date; there is no discussion to find or open, so the lowest-numbered rule and the lesson index in a discussion's body go — a day's lessons are read from its entries.

## What stays

- The entry's one shape, the day's title in the host's local time, and `lesson_of`: an entry written by either program reads, indexes and distills the same.
- `afkconvoy daybook` writes an entry, and the watch's entries and the weekly distillation go through the same crate.
- It is not fleet state: nothing reads it to decide what the fleet does.

## What changes

- The daybook crate keeps both stores behind one interface: the host's database, and GitHub Discussions as it was. afkconvoy writes the host's. Guild, which depends on the crate (ADR-0071), keeps whichever its own design chose — its spec wants its repair records readable on GitHub while its Hub is down — and moves only when it says so.
- The database is the crate's own file and its schema the crate's alone: a second program of the host's user writes the same file, as two writers once wrote one discussion.
- A human reads it with `afkconvoy daybook --read`, by day or span. GitHub's pages no longer show a host's new entries; the Discussions written before this stay where they are and are not moved.
- An entry is written at once or the write fails locally; the watch's queue of entries waiting for GitHub has nothing left to wait for.

## Considered options

- **Keep the daybook on GitHub, in a repo every host names.** Refused by Felix: a host's records are the host's, as its issues are.
- **Put the entries in `host.db`.** Refused: the daybook crate is another program's dependency, and two schema owners of one file would fight over its version.
- **Move Guild's daybook too.** Not this decision: Guild's choice is Guild's.

## Consequences

- The daybook of a host is read on that host. Several hosts' days are read host by host.
- The weekly distillation reads this host's entries alone. Lessons recorded on GitHub before the move are not distilled again.
- The host's daily backup (of its data directory) covers the daybook.
