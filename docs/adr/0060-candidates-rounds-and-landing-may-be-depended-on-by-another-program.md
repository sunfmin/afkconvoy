---
status: accepted
---

# Candidates, rounds and landing may be depended on by another program, with the forge behind an interface

Amends ADR-0024, ADR-0048 and ADR-0059 (sunfmin/control#696).

How afkconvoy lands — a candidate as an immutable merge of a change's original head onto its predecessor, a candidate chain that cancels a failed or invalid suffix, a round that slow-gates once at the chain's tip and is not split when red (ADR-0048), an ordered fast-forward after the predecessor landed, all on one record the remote holds and changes only by compare-and-swap — is wanted by another program of this host's user (Guild, sunfmin/control#693), whose repos live on Forgejo and whose changes are its own tasks. Landing is already plain git: a turn merges, gates, records and pushes a fast-forward, and never asks the forge to merge. What tied it to GitHub and to the fleet is around it, and that is what is put behind an interface. The rules have one home.

That home is its own crate, `crates/landing` (`afkconvoy-landing`), beside the gate crate it builds on (ADR-0059). `afkconvoy-core` depends on and re-exports it, and the cli uses it:

- `chain`: candidates, the chain and every transition of it — assignment, the gate's evidence deciding a candidate, cancellation of the suffix, a round's tip and its red, ripeness, the landed prefix, an outside move of the target, and the proof `Publication` that publication cannot be authorized with any observation missing (Flux).
- `lease`: the ages a chain's owners are judged live or dead by, which the fleet's claims and instances are read by too.
- `turn`: what every runner of a turn follows: the ref the chain of a target is recorded on, the gate a candidate runs itself and the slow gate a round at it runs, what a turn on the chain does next, the publication authorized from what was just observed, the order of fast-forwards it checks, and the refs one atomic publication updates.
- `forge`: `Forge`, what a turn needs of the forge, which the program that lands implements: the change as the forge has it now (its number, its source branch, its target, its title), what the program notes of its candidate, the message of the commit that lands it, whether it may land now, and what follows once it landed.
- `land`: a plain-git runner (`Lander`) for a program with no runner of its own: it assigns its changes' candidates, gates each with the gate crate's plain runner, runs a round's slow gate once at its tip, and lands what is ripe as one atomic push, telling the forge each change that landed.

afkconvoy's forge is GitHub's, in the cli (`forge::Pulls`): a pull request is the change, the issue it closes is noted beside its candidate, the issue's open blockers and pinned target are what may keep it off, and closing that issue with its work account is what follows once it landed. The fleet lock stays the turn's: a lock is a barrier of the whole window, not a refusal of one change. afkconvoy's own runner of a turn stays in the cli, with what only a fleet host has: lanes of the exclusive queue, a gate's process group owned, frozen and confirmed gone across turns and hosts, a fast-green candidate adopted by a later turn of its PR, and a base tip certified alone before a candidate is assigned on it. It decides nothing of its own: every rule it follows is the crate's.

Its API stays free of fleet concepts: no team, role, stage, issue, claim, patrol, intake, tracker, GitHub or pull-request type appears in it, and it does not depend on `afkconvoy-core`. What a program notes beside a candidate is passed in neutrally, as a ticket's note is in the gate crate: a candidate carries a `note` of its program's type, which no rule of the chain reads and whose fields are the candidate's own on the record (afkconvoy's `Note`: the issue the PR completes). A change is recorded by its number on its forge, under the key `pr` the record has always had, so a record written before reads the same.

The crate keeps its Flux refinements (ADR-0007). `./verify.sh` checks it as the core's dependency, and the gate holds it, as the core, the harness crate and the gate crate, to no function taken on trust.

Another program depends on it as a git dependency pinned to a revision of this repo; it is not published.

## Considered options

- **Let the other program depend on afkconvoy-core.** Refused: a candidate named its issue, a turn checked a fleet lock, a claim and a pinned target, and settled the issue it closed; every change to the fleet's types would reach the other program.
- **Copy the chain into the other program.** Refused: the rules would have two homes, and drift.
- **Move afkconvoy's runner of a turn into the crate too.** Not now: its lanes, process groups, freezing and adoption are a fleet host's, tied to the exclusive queue's holder and the host's identity. The crate holds every rule the runner follows and a plain runner beside it, as the gate crate does with `run::gate` (ADR-0059).
- **Ask the forge to merge.** Refused, as before: a forge's merge is not the exact commit the gate passed, and the chain's record is what orders the landings.

## Consequences

- A rule of candidates, chains, rounds or landing is spelled only in the landing crate: afkconvoy-core re-exports it, and the cli calls it.
- A change to the crate's API is a change to another program's dependency: it stays neutral, and a fleet fact it needs is noted beside it or asked of the forge, not named in the crate.
- A test of the crate from outside, with only the crate, the gate crate and a fake forge, lands two changes on a local bare repo in one round — both fast-green, one slow gate at the tip, the target fast-forwarded through both candidates in order, the forge told each landed — and lands nothing from a red round, which cancels the chain; it checks that the crate depends on nothing of the fleet, nor names a fleet concept in a public item.
- The plain runner's gates run to their end in its own process, so a candidate is stopped once its gate returns; it reads each source head unchanged just before the push that lands it, where afkconvoy's runner checks them within that push. It certifies no base tip alone; a program that needs that gates its target first.
- afkconvoy's behaviour does not change: the same rounds, the same red-round handling, the same records.
