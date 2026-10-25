---
status: accepted
---

# A checkout's fleet works the issues of a tracker its host names

A fleet worked the issues of the repo it works in. On a public repo, strangers then write what the fleet reads: anyone can comment on an issue the fleet works, and every stage record, verdict, claim comment and intake record is a comment, read without asking who wrote it. Anyone can write instructions a worker reads. ADR-0012 already said "a repo that takes issues from strangers needs the separate identity first", and nothing enforced it. Separately, two people whose fleets work one repo took each other's issues, because nothing said whose an issue is.

So a fleet reads and writes its issues in its **tracker**: the checkout's own repo, until the checkout says another on its host — `afkconvoy run <checkout> --tracker <owner>/<repo>`. It is kept with the checkout's entry on the checkout list, never pushed, and read as each tick opens the fleet, as the checkout's concurrency is (ADR-0025): whose issues a fleet works is the choice of whoever runs it, and two people may work one repo from two trackers.

## What is where

- Everything about an issue is the tracker's: the issue, its labels, its comments — claim comments, verdicts, records — its blockers, its escalation and its closing. Every issue the fleet or a worker opens, a defect of the base branch among them, is opened in the tracker with the repo label.
- Everything else is the repo's: PRs, branches, worktrees, the gate and the landing train.
- A tracker holds the issues of many repos. The **repo label**, `repo:<owner>/<repo>`, makes one an issue of that repo. A fleet whose tracker is another repo reads only the open issues there that carry its repo's repo label — GitHub filters them, so a rebuild asks GitHub as often however many repos' issues the tracker holds — and its frontier is those that also carry the ready label — in a bound repo, a flow label. An issue that carries two repo labels is dispatched by no fleet: `plan` says why. In a repo that is its own tracker, an issue that carries a repo label is the issue of the repo it names, not its own: a repo that is its own fleet's tracker and another's never works the other's issues.
- `#<n>` alone, in a PR or a commit, is always the repo's own issue. The repo names a tracker's issue `<owner>/<tracker>#<n>`: in a PR's closing line, in the landing train's join commit, and in the grep that settles it from the base branch. A PR that names an issue of another repo, or that GitHub links to one, closes none of the fleet's.
- Every name in the repo or on its host tells a tracker's issues apart from the repo's and from another tracker's: a claim's ref is `claim/<owner>/<tracker>/<n>` (in lower case), a worktree and its branch are named `<owner>-<tracker>-<n>-<slug>`. A fleet takes a claim of another tracker for none of its own.
- In a bound repo the same holds of every stage, split, patrol, consult and proposal, of casting, and of `rule` and `send`. Intake and a fleet lock give a tracker's issue the repo label where it lacks it, before the flow or lock label, and refuse one that carries another repo's: it is that repo's to work, and a lock on it would hold that repo's fleet and not this one. The intake limit, and a patrol's open and landed issues, count the repo's issues alone. `flows` makes the flow labels in the tracker and gives the line that opens an issue there with the repo label; a `stage:`, `harness:` or `model:` label is made there as it is first put on an issue.
- A team's issues stay the team's: a roster issue is opened in the team's repo, and a precedent is proposed as a PR there, whatever the tracker of the fleet that found it.
- A worker's brief names the tracker in every issue command — `gh issue view <n> --repo <owner>/<tracker> --comments`, `gh issue create --repo <owner>/<tracker> … --label repo:<owner>/<repo>` — and in no PR command, which is the repo's. So does every line a human is given to answer an issue — an escalation's comment, the status board, `plan`'s and `doctor`'s answers and fixes — `gh issue close <n> --repo <owner>/<tracker>`: run in the checkout, `gh` acts on the repo's own issue of that number.
- `run --tracker` makes the repo label, and the ready, escalate and lock labels of the repo's config, in the tracker where it lacks them. It refuses to change the tracker of a checkout whose fleet holds a claim on an issue of the tracker it had, or whose landing train carries a PR of one, naming them: their issues would be read in a tracker that does not hold them. `--tracker default`, or the checkout's own repo, says none. The old tracker's issues stay where they are: nothing is moved.
- `checkouts`, `doctor`, `plan` and the panel show the tracker.

## Considered options

- **Keep the issues in the repo, and read only what trusted logins wrote.** Refused: it puts the boundary in a filter at every place the fleet reads — every comment, label, blocker and record — and any place missed is a hole. And no filter keeps a stranger's comment from a worker that runs `gh issue view`. A private tracker puts the boundary in GitHub's access control, which holds whatever reads it.
- **Name the tracker in the repo config.** Refused: it is pushed, so every host would work one tracker, and two people could not keep their issues apart. Whose issues a fleet works is a fact of its host, as its concurrency is.

## Consequences

- Amends ADR-0012's last consequence: the repo may be public, but the tracker may not. A ticket filed beside this one (#463) has the fleet refuse a public tracker.
- What stays exposed in a public repo: the comments on the fleet's PRs there, which no brief sends a worker to read; and the tracker's name and its issue numbers, in the repo's commits and PRs.
- A tracker's issue closes when the landing train that carries its PR lands — by the train of a fleet of that tracker — or when that fleet settles it from the base branch. Two fleets of one repo that work two trackers share the repo's one landing train, and each closes only its own tracker's issues.
- orca links a worker's worktree to its issue by number alone, on its host. A tracker changes only while its fleet holds no claim and its train carries none of its issues, so no worktree of a live claim is taken for another tracker's issue of the same number.
- This covers a bound repo as it does an unbound one, end to end: its stages to the landing train, intake, casting, patrols, splits and fleet lock (#462).
