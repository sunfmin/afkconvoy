Every way the fleet stops for an answer, and the command a human or the host's watch uses.

## Where to look

`afkconvoy doctor` lists every open issue that waits for a human under `waiting_on_human`, with `why` (what kind of wait, then the first sentence of the reason its comment gives), `url` (that comment, or an older failure report itself), since when and `answer`: the line to run for that issue. `afkconvoy plan` gives the same list; the panel shows it as "With a human". Waiting more than an hour is a finding.

An escalated issue carries the escalate label (`{const:ESCALATE_LABEL}` unless renamed) in place of its flow label. Its comment says why; current comments end with the line to answer. `doctor` gives the current line, even where an older comment has none or names the issue before it moved to the tracker. Run every command below in a checkout of the repo, replacing what stands between < and >.

## An escalation

A stage failed or declared `escalate`. A consult explicitly named by the stage is asked first about the work; an unbound repo, or a stage that names no consult, escalates immediately. There is no stock consult. The host's watch picks the issue up from `waiting_on_human` and answers it with the operator's authority. The comment says what failed or gives the question to answer.

A **fleet fault** escalates at once, with no consult asked and no consult claim taken: a worker start that failed again, a worktree trust gap. The comment and `doctor`'s `waiting_on_human` entry say it is a fleet fault and that the fix is on the host — trusting a worktree, switching a harness, waiting out a usage limit, or mending a build server. Fix it there, then hand the issue back with the line given. Another failed start escalates at once again; nothing new retries. A worker gone silent had its brief, so that hand-off still goes to its named consult first, as do an `escalate`, a tripped breaker, an impossible `blocked`, a refuted `already-satisfied`, a commit a stage may not leave and a changed workflow.

A watch is the operator for every repo on its host's checkout list (`afkconvoy run --help`, WATCH). It may fix the host cause of a fleet fault and hand the issue back with the current line `doctor` gives, checking the next tick dispatches or continues it. A lasting fix in any listed repo goes through its own checkout and landing turn; one that must reach an installed tool is followed through cutting, installation and a check on the new version. The watch may rule, hand an issue back, send it to a stage, close it or change its blocked-by, using the same commands as a human. Its ruling and sending start rework counts over as a human's do; it supplies no consult flag or role and team commits. Both the stock watch and a watch in the operator's directory leave only these four kinds to a human, list each with the question and answer line in their printed record, and act on none of them: spending money or needing an account, a credential or a person's hands; what cannot be undone; taking a secret or private data off the host; changing what a product is for or who it is for. `doctor` lists that record as what the watch did.

For a failure, fix what failed first: `afkconvoy rule` starts the same stage anew, and what failed it once fails it again. In an unbound repo there is no stage to run again: fix what the comment names and put the ready label back, with the line the comment ends with. A PR the escalation kept is not started anew: the next worker takes it up — it reads the issue's comments, merges the base branch, gates and lands in a turn — so say on the issue what is to be done on it.

## An escalation with no position

An issue escalated before it entered any workflow of the team — the repo was unbound then, or it came from another repo — has no stage records: `afkconvoy show --issue <n>` says it has no position. `rule` and `send` refuse it and give the line that puts a flow label on it. Answer it with that line, as `doctor`'s `answer` gives it: with one workflow its flow label is filled in, ready to run; with several workflows, `why` lists each flow label to choose `<flow label>` from. `plan` and the panel give the same answer and choices. The issue starts at the chosen workflow's first stage.

Where its comment gives a line of its own, `doctor`'s is the current one: a comment written before the issue moved to the tracker names its old number and may name no tracker. Use the current issue number and tracker in the line `doctor` gives.

## Answering an escalation

Every command a human or the watch runs to answer one, `<n>` the issue's number. Where the checkout's tracker is another repo (`afkconvoy run --tracker`), name it in every `gh` line — `--repo <owner>/<tracker>`, `repos/<owner>/<tracker>/…` — and give `gh issue create` the repo label too, `--label repo:<owner>/<repo>`: run in the checkout, `gh` acts on the checkout's own repo. The lines an escalation's comment, `plan` and `doctor` give name it already. A consult may run each, and any read-only command, the gate too; never commit, push or change the host.

- `afkconvoy rule --issue <n> --text-file <file>`: The stage runs again under your ruling; both rework counts start over (a consult's starts none over). `afkconvoy rule --help`.
- `afkconvoy send --issue <n> --stage <stage> --text-file <file>`: Put it at another stage, when the approach or workflow changed, or bring it back from a park. `afkconvoy send --help`.
- `gh issue close <n> --comment <why>`: Nothing is left to do on it: the fleet leaves a closed issue alone.
- `gh issue create --title <title> --body-file <file> --label <ready label>`: Open what it found outside this issue — a broken base branch, a host problem, a defect of afkconvoy — for the fleet to work.
- `gh issue edit <n> --body-file <file>`: Keep its body's "## Blocked by" in step with an edge added or removed.
- `gh api repos/{owner}/{repo}/issues/<n>/dependencies/blocked_by -F issue_id=<id>`: Record it as blocked by the issue whose database id is `<id>`.
- `gh api -X DELETE repos/{owner}/{repo}/issues/<n>/dependencies/blocked_by/<id>`: Remove that edge: one whose waiting would never end, say.

## Parked for evidence

A consult's close or park carries a reason and adds no rework. `plan` and `doctor` list parks under `waiting_on_human`. To bring one back in a bound repo, explicitly send it with new evidence or your reason (a plain comment does not bring it back):

```sh
afkconvoy send --issue <n> --stage <stage> --text-file <evidence-file>
```

## A tripped breaker

Rework went past what the workflow allows — in total, or as rejections in a row by one stage — and the issue went to the consult in place of that rework, and escalates for the watch where the stage names no consult or it trips again after the consult ruled. The comment names the breaker and lists every rework with its stage and why. Answer as an escalation: `afkconvoy rule` with what settles it, or `afkconvoy send`. `afkconvoy doctor` says beforehand when an issue is one rework from a breaker.

## A stage `by: human`

A workflow may give a stage to a human. An issue at one waits there, claimed by nobody, under its flow label and with no escalate label: only the lists above show it.

- `afkconvoy verdict pass --issue <n> --stage <stage> --text-file <file>`: Declare the stage's verdict: `pass`, or `reject` where the stage has that edge. The issue goes where the workflow leads from it. `afkconvoy verdict --help`.

## A patrol's failed run

A run of a patrol that failed opens an issue of its own, under the escalate label, which says what failed. The patrol is not run again while that issue is open.

- `gh issue close <n>`: Once you have looked: the patrol runs again when it is next due. `afkconvoy plan` gives each patrol's standing.

## A cut that failed

A cut that failed at a step, ran out of time or was ended from outside opens an issue of its own, under the escalate label, titled `afkconvoy: the cut of v<version> failed`: which step failed, why, what it did before, and the line that goes on from there. Its checkout is kept on the host that cut. No cut starts while that issue is open.

- `cd <its checkout> && afkconvoy cut --from <step>`: Once what failed it is fixed — the line the issue gives. It takes that step and the ones after it, and closes the issue once it took the last. `afkconvoy cut --help`.
- `gh issue close <n>`: Once you have looked and want the fleet to cut again from the start: the next tick that ends idle does, its kept checkout removed first. A PR of the failed cut still open is yours to close. A cut that failed at `publish` has pushed its tag: closed with no line run, that version stays unpublished, and the next cut is of the version after it, once more lands.

## A fleet lock

An open issue under the lock label is a fleet lock: the fleet works lock issues only, and hands no other issue to a human, until the last one closes. A worker raises one when the fault is one every issue shares; a human may too:

```sh
afkconvoy lock --issue <n>
```

Closing a lock's issue ends it. A lock needs you when nothing can work its issue: it has no flow label or an open blocker. `afkconvoy doctor` gives a critical finding with a `fix` to run. Close a lock raised by mistake. `afkconvoy lock --help`.

## A precedent to accept

Where the team names who proposes precedents, each ruling a human gives with `afkconvoy rule` is followed by a proposal: a PR on the team's repo that appends one entry to its precedents — the ruling in a sentence, when it applies, and a link to the ruling. Nothing waits on it: the stage that asked has already run again.

- `gh pr merge <pr> --repo <the team's repo>`: Merging accepts the precedent: every later stage reads it, and follows it in place of declaring `escalate` on the same question.
- `gh pr close <pr> --repo <the team's repo>`: The ruling was about that one issue, and no stage should follow it unasked.

The fleet never merges on the team's repo. `afkconvoy doctor` lists the team's open proposals under `proposals.open`: each PR, the issue and comment of its ruling, how long it has been open, and the merge and close commands. `afkconvoy doctor --all` lists them for every checkout on this host's checkout list. `afkconvoy panel` shows them under "With a human" too. Both read only and make no startup probe. Once the open proposals reach the team's `max_open`, doctor gives a finding and the panel says so: no further proposal starts until a human merges or closes one. `afkconvoy plan` lists the rulings being proposed or still to be proposed under `proposals`, with how each stands; it makes a startup probe where one is due.

## An issue past the end of its workflow

A workflow that ended without a landing leaves its issue open: `gh issue close <n>`.

## What not to do

For an issue with a position, do not move the labels by hand: its stage records say where it is, so one handed back with no ruling on record is escalated again. Rule on it, or send it. An escalation with no position needs a flow label first, as above.
