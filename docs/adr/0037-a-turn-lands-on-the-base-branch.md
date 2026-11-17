---
status: accepted
---

# A turn lands its PR on its target branch

A turn is one command that merges, gates, records and lands. It makes the PR's merge commit on the current target tip, gates that commit, records green, and pushes that same commit to the target branch as a fast-forward. Nothing is joined after the turn: there is no landing train, no train worker and no train lane. A second step would add no verification after a recorded turn, and would need another worker and a separate lane.

A worker used to merge, wait twenty to forty-five minutes for its gate, and then have the combined tree gated again; one red tip made many workers repeat the same failure. Holding merge, gate and record in one command, with the PR open before it asks, removes both. Only the workflow's landing worker takes a turn; a standalone gate before review lands nothing.

How a turn's PR comes together with others' is ADR-0083: a round of candidates, grouped through a pairing tree and slow-gated once. Where the candidate chain and its rounds are kept is ADR-0117: each host's own, in its database. A repo that names a proving branch lands there on its fast gate, and a promotion moves the base branch (ADR-0090); the reason above for no second step does not hold there, because the promotion is that step.

## What ends a turn without landing

Red, a conflict, or a rejected push ends the turn and lands nothing. A conflict with the target tip is merged into the worker's worktree, left for the worker to resolve, commit and ask again. A rejected push requires another turn. Nothing blindly repairs or retries a failed PR; failed work follows the fix-and-commit policy.

## The gate command

`gate_command` names the build-and-test command. `afkconvoy config` and `check` refuse one that calls `afkconvoy gate` or `land`, naming what to set instead. A gate command that wraps its build in `afkconvoy exclusive` inherits the turn's place in the exclusive queue; it does not queue behind itself. Ordinary heavy commands keep same-repo isolation; a light gate takes turns in its repo's own line; fast gates run side by side and take no repo lease (ADR-0088).

## Pinned destinations

An issue turn resolves its task's immutable target record (ADR-0006) before reading its PR or recovering a landing. The PR's actual base must equal that target, before any already-landed shortcut and again before publication; a base changed during validation invalidates the candidate. Each target has its own chain; no candidate or settlement walk uses another target's history. Certification reads the gate declaration from the selected branch's tip but keeps the selected branch's identity. Red markers and repair claims name the affected branch. The configured base alone supplies the landings that trigger a cut.

## Candidates

A candidate is one PR's merge commit on what it lands onto, with a detached worktree of its own. Its source branch and original head stay intact, so a cancelled successor can be rebuilt without carrying a failed predecessor back in. Its states are building, validating, green, landed, red, cancelling and cancelled.

Candidate worktrees live outside every checkout, under the host's cache at `candidates/<repo-key>/<candidate-id>`, keyed by the canonical common git directory: tools that walk up for configuration otherwise read the checkout's files above the candidate's commit. A cache configured inside a checkout is refused before a turn is reserved. A stopped candidate's worktree is removed once its record says landed, red, or cancelled with its processes confirmed gone; one building, validating, green and waiting, or cancelling keeps it, and so does one a live process still has as its working directory. The chain's record keeps the evidence: phase, cause and gate excerpt.

**Publication** is one atomic transaction that advances the target to the candidate whose tree was proved green, requires the target to be exactly the expected predecessor, and verifies each source head unchanged. To local and SSH remotes it is a raw `git receive-pack` command with an explicit old-equals-new check on each source head, which a plain `git push --atomic` would omit. To a GitHub HTTPS origin, the objects go by `git push` to scratch refs and one GraphQL `updateRefs` call checks every old value and applies every update, all or none; after a failed call the refs are read again to tell published, not published, and stale apart. Such a host needs `gh` logged in and `gh auth setup-git`, and no SSH key; no token is handled. An unsupported transport refuses before any update is sent. No aggregate merge is synthesized at publication: the pushed commit is the gated one.

A **red** candidate records its gate evidence and invalidates every candidate built on it before cleanup starts. Their process groups and descendants are stopped, killed and observed exited; green completion cannot restore invalidated work. A cancelled turn answers `cancelled` with `cancelled_by` and `cause`, is charged no gate failure, and asks again from its preserved branch. A base move the chain did not make, or a changed PR head, invalidates the affected suffix.

A valid fast-green candidate whose gate processes are confirmed stopped is adopted by its PR's next turn for the same original head, keeping its identity, commit, tree, result and successors: there is no running gate to duplicate, and a transient fault in a waiting turn should not cancel a round. A candidate still building or gating keeps the invalidation rules.

Process ownership is exact. A gate shell waits on a pipe until its process group is recorded, so a parent dying before registration starts no build. Kernel-held worker places are inherited only by the owned command and survive runner and owner death while descendants remain. Process identities include host, PID and precise kernel start time, and an inherited scope descriptor identifies owned descendants after reparenting; a process group is signalled only while its leader's identity and scope still match. A failed observation does not prove death. A green run whose record the host's database does not take is reported as `not_recorded`; the candidate is not green.

The core tests exhaust the reachable interleavings of three candidates through green and red completion, invalidation, exit confirmation, external movement and publication; Flux checks the publication authorization invariants. This is bounded exploration, not a proof for arbitrary graphs.

## Barriers

Watches, fleet-lock repairs and base certification are serial barriers. A barrier reserves a durable pending place outside the exclusive queue, blocks new assignment while the chain drains, and lands alone through a candidate of its own; it cannot starve behind a refilled window. Unconfirmed cancellation also holds the window; otherwise it has no bound of its own.

A **cut** takes no turn: it commits its version on a branch of its own, gates that commit, and tags it; the base branch never receives it (ADR-0028). Issue turns and watches land as merge commits. A commit with one parent whose message says `By a cut on <host>` is still read as a cut's landing by an older release; one with one parent and no such line is not a landing.

## A red or unrecorded base tip

A base tip with no record is gated alone in a separate checkout, by its own gate declaration. Its record is this host's: a host keeps its recorded gate runs in its own database (below), so a tip another host landed, and certified there, is gated alone again here before this host's next landing. That run is wasted, and accepted (sunfmin/control#1326). Ordinary turns wait, outside the exclusive queue, while it is red. The fleet opens a dispatchable fleet-lock repair issue for that exact tip, under an exclusive claim, with the fleet's repair and ready labels and no human stage; the guard reserves the repair label to the fleet. Only that repair may take a turn on the red tip, and it must still gate and record its complete merge commit. A repair's turn never waits for the green tip it exists to restore.

A base-tip run red only on TIMEOUTs the gate says it cannot classify because the host is overloaded (`gate::judge`) is recorded neither green nor red: the turn answers `base_uncertified`, the next turn gates the tip again, and no repair opens. A FAIL, a classified TIMEOUT, or a run that reached the gate limit is red. Every base-tip run that is not green keeps its whole output in `convoy-base-tip-gate-<at>-<unique>.log` in the checkout's git dir, which the turn's log line names.

## A green run of a tree is not run again

Felix decided it always on (sunfmin/control#693). A gate record names its tree, its command, and the kind of machine it ran on (operating system and architecture, such as `macos-aarch64`). Asked to run the same command on the same committed tree on the same kind of machine, the gate crate's runner answers with the green run on record, says it is reused, and runs nothing. A red run is never on record, so never reused. Another kind of machine, a worktree that is not exactly its commit, or a record that states no machine, runs again. A candidate whose tree is green on record is not gated again, and its evidence names the run it stands on under `reused`. The rule lives in the gate and landing crates (ADR-0058), so Guild has it too. A worker's own `afkconvoy gate --fast` records nothing, so it does not answer for a candidate.

## Where a gate record lives

Each host keeps its own recorded gate runs in its own database, `host.db` (ADR-0117), one row per repo, tree and command, holding the record's one encoding: tree, command, when, seconds, host, kind of machine and each test's times. A red run of an issue's tree is a row of its own until the work account cites it. Each host lands its own, and runs wasted between hosts are fine (Felix, sunfmin/control#1321): so a green run pushes nothing and waits on no remote, and reuse, base-tip certification and the gate gone slow (ADR-0047) read this host's records alone. Nothing reads a gate ref of the remote; a start's probe deletes every one an older release left, and `doctor` says while any is left. A record is believed for a day; a write sweeps the repo's older ones. The gate crate's `Records` is where a record is kept: afkconvoy's are the host database's, and its `Git` keeps them as refs of a remote for a program that has no database (ADR-0058).

## Older lines

A landing-train line an older release left is migration input only: a green tip can fast-forward the base with its record; otherwise each PR takes a turn, through resumable recovery PRs that keep the line's history. No new line is written, and the old one is removed only once its work is accounted for.

## Considered options

- **Keep the landing train**, a separate join after a recorded turn. Refused: it added no verification, and needed its own worker and lane.
- **Make reuse a repo's choice, off by default.** Refused: Felix wanted it always on.
- **Key reuse by host name.** Refused: two hosts of one kind pass the same tests, and that sameness is what reuse saves.
