Every way the fleet stops for an answer, and the command a human or the host's watch uses.

## Where to look

`afkconvoy doctor` lists every open issue that waits for a human under `waiting_on_human`, with `why` (what kind of wait, then the first sentence of the reason its comment gives), `url` (that comment, or an older failure report itself), since when and `answer`: the line to run for that issue. `afkconvoy plan` gives the same list; the panel shows it as "With a human". Waiting more than an hour is a finding.

An escalated issue carries the escalate label (`{const:ESCALATE_LABEL}` unless renamed) in place of its flow label. Its comment says why; current comments end with the line to answer. `doctor` gives the current line, even where an older comment has none or names the issue before it moved to the tracker. A line `doctor` or `plan` prints is run as printed, apart from what stands between < and >, and already names the checkout. The same commands are below, `<n>` the issue's number.

Each human entry of a recorded watch report also becomes a durable question in the host's **asking session**, a second resident interactive Agent in a Terminal named “afkconvoy asks” beside the watch in the watch's directory. `doctor.host.asking` and the panel show its state, the current question and those waiting. The session starts when the first question is pending and asks one at a time, only when its input is ready. Each delivery names the condition, issue link, watch question and answer line, and how many more wait. Orca's phone push carries the session's last message — the question first, at most 180 characters — and a tap opens this Terminal; no other channel is built for it. A condition that recovered before delivery is not asked; one that recovers afterwards is told as withdrawn. Reworded reports and host runner restarts do not ask a condition again within its episode.

Answer in the asking session: it can clarify, investigate and consult with you, then act with your authority through the same commands below. For a settled question it records your actual words, quoted with their time, on the issue and links that comment in its `afkconvoy asked` report; the session's own words establish no human intent. Only then is the next question delivered. You may put a question aside instead: it stays pending, and the session lists it with the others when you ask. The session stands by without a model between questions and does none of the watch's work: no watch round, no look, no `afkconvoy watched` report. What you tell it there, a host fix or install included, it carries out at once within the watch's bounds, quoting your words and their time on the issue.

With `afkconvoy run --notify-command <COMMAND>` (or AFKCONVOY_NOTIFY_COMMAND), the host runner sends each human decision a watch reports through the operator's shell command, with one JSON object on stdin: `host`, `watch`, `condition`, `question`, `answer`, `url` (the issue's source URL, null for a host condition) and `panel` (the URL actually served, null if unavailable). The question and answer leave the host only through that chosen command. Nothing is sent by default; `--notify-command default` removes it. The same condition is sent once per observed episode, including across runner restarts and upgrades, even when later reports reword or omit it. Human waits use the issue and source identity of `waiting_on_human`; host conditions use the finding's identity. Only observed recovery ends the episode. A report names each entry's `condition` from its round's look, checked against that look (`afkconvoy watched --help`); the words of a question or answer are never its identity. The issue URL can be opened away from the host; the panel URL is local to the host. Failed commands and those past {const:NOTIFY_SECONDS} seconds are tried again at the next look without delaying the watch or fleets; {const:NOTIFY_FAILURES} failures in a row for one entry are a doctor finding. `doctor.host.notify` gives the command and last sending. Only decisions already left to a human in a watch report are sent.

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

A cut that failed at a step, ran out of time or was ended from outside opens an issue of its own, titled `afkconvoy: the cut of v<version> failed`: which step failed, why, what it did before, and the line that goes on from there. Its checkout is kept on the host that cut until the next cut starts. The issue holds no cut back: the next due cut cuts again from the start, on the base tip as it stands, whether or not the issue is open, and a cut that publishes closes it, naming the version that went out. Each later failure while it is open is added to it as a comment. Where the same step fails the same way on three cuts in a row, it takes the ready label and becomes an ordinary ticket for the fleet to fix, and the next cut waits for a landing.

It is opened under the label `convoy-cut-failed`, not the escalate label: the host's watch looks for its cause, as it answers whatever holds a fleet back, and `doctor` says what becomes of it (`its last cut failed`, `afkconvoy help troubleshooting`). Only where what failed it is one of the four kinds every watch leaves with a human does the watch swap that label for the escalate label; then it waits for you, and these are its answers. Cuts are still attempted meanwhile. An issue a release before this label opened carries the escalate label, and waits for you too.

- Fix what failed it: the next due cut cuts again from the start, and closes the issue once it publishes. `afkconvoy cut --help`.
- `gh issue close <n>`: Once you have looked: nothing waits for it either way. A cut that failed at `publish` has pushed its tag: that version stays unpublished, and the next cut is of the version after it, once more lands.

## A fleet lock

An open issue under the lock label is a fleet lock: the fleet works lock issues only, and hands no other issue to a human, until the last one closes. A worker raises one when the fault is one every issue shares; a human may too:

```sh
afkconvoy lock --issue <n>
```

Closing a lock's issue ends it. A lock needs you when nothing can work its issue: it has no flow label or an open blocker. `afkconvoy doctor` gives a critical finding with a `fix` to run. Close a lock raised by mistake. `afkconvoy lock --help`.

## Lasting decisions

A ruling records the decision on its issue; its stage runs again under the existing rework semantics. Incorporate a lasting rule through ordinary work in the owning ADR, team or role rules: amend contradictory wording and preserve the source. A human decision needs an actual human instruction or confirmation and its source; a shared GitHub identity, agent-authored ruling or merged document alone establishes none.

Legacy proposal PRs are retained for content review and migration, with their links and disposition recorded on the PR. The fleet schedules no proposals and provides no human acceptance queue.

## An issue past the end of its workflow

A workflow that ended without a landing leaves its issue open: `gh issue close <n>`.

## What not to do

For an issue with a position, do not move the labels by hand: its stage records say where it is, so one handed back with no ruling on record is escalated again. Rule on it, or send it. An escalation with no position needs a flow label first, as above.
