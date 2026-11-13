---
status: accepted
---

# The daybook crate may be depended on by another program

Amends ADR-0065 (sunfmin/control#812).

The **daybook** — the operator's one place to read what agents did and decided outside any issue, one discussion a day in a tracker's `Daybook` category (ADR-0065) — is written by another program of this host's user too: Guild's daemon records there (sunfmin/guild#68, for sunfmin/control#810). Guild may use afkconvoy only as crates, never as a program (sunfmin/control#811). The daybook was written only from the cli's `daybook` module, so Guild had to keep its own copy of the entry's shape, of the day's discussion and of the rule that the lowest numbered one wins. Two copies of a format two writers share drift apart, and a daybook whose entries disagree in shape cannot be read, indexed or distilled as one.

That knowledge is its own crate, `crates/daybook` (`afkconvoy-daybook`), which the cli depends on and `afkconvoy daybook`, the watch's entries and the weekly distillation go through:

- the entry's one shape: the repo it is of, the kind of session that wrote it, then the headings `Did`, `Why`, `Undo`, `Lesson` and `Links`, each left out when said of nothing (`entry`, `Said`); a lesson as one line under its heading, and read back off an entry (`lesson_of`);
- the day: its title, `YYYY-MM-DD` in the host's local time (`today`, `day_at`, `next_day`, `day_named`);
- the day's discussion: found, or created by the first entry of the day, and where two writers create one at once, the lowest numbered of that title is the day's, every entry goes there, and a creator that finds an older one deletes its own (`Daybook::append`);
- the lesson index at the foot of the day's discussion body, rebuilt whole from the day's entries by each append that writes a lesson and checked to hold it after (`indexed`, `index_of`), and the lessons and entries of a span of days read from the days' bodies alone (`Daybook::lessons`, `Daybook::held`).

GitHub is reached through one seam, `GraphQl`: a GraphQL call with its variables, saying whether it writes. afkconvoy implements it with its own `gh` runner, which reads again on a failure in transit and writes once; the crate's own `Gh` runs `gh api graphql` plainly; a test passes a fake. The caller says the repo whose Discussions hold the daybook, the repo an entry is of, the word for the kind of session that wrote it, the day, and the program a new day's discussion says wrote it. The crate reads no environment variable of afkconvoy's (`AFKCONVOY_WATCH`, `AFKCONVOY_ROLE`, …) nor the host's checkout list: telling the kind of session from the environment, and the tracker from a checkout, stay the cli's.

What it promises another program is the format and the rule, not afkconvoy's flags: an entry written through it reads, indexes and distills the same as one `afkconvoy daybook` wrote, and two writers of the first entry of a day — of either program — leave one discussion holding both. The weekly distillation's file and its check stay the cli's, as the watch's alone.

Another program depends on it as a git dependency pinned to a revision of this repo, the revision it was tested with; it is not published. It depends on nothing of the fleet — not `afkconvoy-core`, nor the cli — and no team, role, stage, issue, claim, patrol, intake, worker or watch type appears in its API. It is effects through GitHub and formatting, and is not checked by Flux.

## Considered options

- **Let the other program run `afkconvoy daybook`.** Refused: Guild may use afkconvoy only as crates (sunfmin/control#811).
- **Let the other program keep its own copy.** Refused: two writers of one daybook would hold two copies of its format and of its one-discussion-a-day rule, and drift.
- **Put it in `afkconvoy-core`.** Refused: the core is the fleet's pure decisions, and every change to the fleet's types would reach the other program (ADR-0058's reason).
- **Publish the crate.** Not needed: a git dependency pinned by revision is reproducible, and nothing outside this host's user depends on it.

## Consequences

- The entry's shape, the day's title, the lowest-numbered rule and the lesson index are spelled only in the daybook crate; the cli maps its environment and checkouts onto it.
- A change to the crate's API, or to the format it writes, is a change to another program's dependency and to the daybook both write: it stays neutral of the fleet, and a format change is one both programs read.
- A test of the crate from outside appends entries to a fake GitHub, indexes a lesson and lists it back, has two writers create the day's discussion at once and finds one discussion holding both, and checks the crate depends on nothing of the fleet, nor names a fleet concept in a public item.
