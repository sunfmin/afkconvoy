# Issue tracker

Issues and specs for this repo live in its **tracker** (`GLOSSARY.md`, `docs/adr/0027`, `docs/adr/0091`): by default its host's **local tracker**, `local/<host>`, a database on the host; or a GitHub repo the checkout names. Whose issues a fleet works is a fact of its host, never pushed, so this file names no tracker: ask the host. `afkconvoy issue` reads and writes either kind, with `gh issue`'s verbs and flags (`afkconvoy issue --help`); `gh issue` reaches a GitHub tracker only.

## Find the tracker

```sh
afkconvoy checkouts | jq -rn --arg root "$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")" \
  'input | if .error then error(.error) else [.checkouts[] | select(.checkout == $root) | .tracker][0] // "none" end'
```

It prints the `tracker` that `afkconvoy checkouts` answers for this checkout, whose entry is its main worktree's: the fleet reads it the same way from any worktree of it. Four cases:

- **It prints `local/<host>`**: the host's local tracker, the default. Every operation is `afkconvoy issue`, run in the checkout on this host — `gh issue` would act on this repo's own GitHub issue of that number. An issue's repo is a field of it, not a label: `afkconvoy issue create` opens one of this repo, and `--repo <owner>/<repo>` one of another repo this host has a checkout of. The `afkconvoy issue` forms are given beside each operation below.
- **It names this repo**, `sunfmin/afkconvoy`: the tracker is this repo itself on GitHub, and no repo label is used. An issue here that carries a repo label is the issue of the repo that label names, not this one's.
- **It names another repo**, `<owner>/<repo>`: the issues are there, among other repos' issues. This repo's carry its **repo label**, `repo:sunfmin/afkconvoy`. An issue there without it is no issue of this repo, and no fleet works it.
- **It prints `none`**: the checkout is on no checkout list of this host. Its fleet would work the host's local tracker, which `afkconvoy issue` reads off the checkout.

Where it fails instead, `afkconvoy checkouts` gave no answer, and the tracker is unknown: open, list and change no issue until it answers.

Below, `<tracker>` is the GitHub repo it names.

## Conventions

Of a local tracker, each `gh issue <verb> <n> --repo <tracker>` below is `afkconvoy issue <verb> <n>` with the same flags and no `--repo`; blocking is `afkconvoy issue block <n> --by <m>` and `afkconvoy issue unblock <n> --by <m>`, by number; reading is `afkconvoy issue view <n> --comments` (its `--json` takes `number,title,body,labels,comments,state` as gh's does, and `blockedBy`) and `afkconvoy issue list --label <l> --state open`. A label needs no making there, and there are no sub-issues: put `Part of local/<host>#<parent>` at the top of the child body. A worker's guard refuses `gh issue` and `gh api …/issues…` where its fleet's tracker is local.

Of a GitHub tracker, every operation on an issue goes to the tracker: put `--repo <tracker>` on each `gh issue` command below, and `repos/<tracker>` in each `gh api` path of an issue. Run in the checkout without it, `gh` acts on this repo's own issue of that number.

A GitHub tracker refuses a label it lacks, on a new issue or an old one: where `gh label list --repo <tracker>` does not have a label a skill asks for (`needs-triage`, `epic`, `wayfinder:map`, …), make it first with `gh label create <label> --repo <tracker>`.

- **Create an issue**: run `afkconvoy flows` in the checkout, once for all the issues you open. Its `line` (`flows --help`) opens an issue in the tracker, with the repo label where the tracker is another repo. The tracker is the one this host's checkout list says; in a checkout the list does not hold (another account, a fresh clone) `flows` refuses until `--tracker <owner>/<repo>` names it. Give it a title and a body file. The ready label on that line hands the issue to the fleet: for an issue not ready to be worked (`needs-triage`, an epic, a wayfinder map), put the label the skill asks for in its place, and keep `--repo` and the repo label.
- **Read an issue**: `gh issue view <number> --repo <tracker> --json number,title,body,labels,comments`.
- **List issues**: `gh issue list --repo <tracker> --label repo:sunfmin/afkconvoy --state open --json number,title,body,labels,comments --jq '[.[] | {number, title, body, labels: [.labels[].name], comments: [.comments[].body]}]'` with appropriate `--label` and `--state` filters. Where the tracker is this repo, leave out `--label repo:sunfmin/afkconvoy`.
- **Make an issue a sub-issue of a parent**: `--parent <parent>` on the create line, or `gh issue edit <parent> --repo <tracker> --add-sub-issue <child>` afterwards (`gh` 2.94+). Older `gh`: `gh api --method POST repos/<tracker>/issues/<parent>/sub_issues -F sub_issue_id=<child-db-id>` (database id, as in **Blocking** below). Without sub-issues, put `Part of #<parent>` at the top of the child body.
- **Comment on an issue**: `gh issue comment <number> --repo <tracker> --body "..."`
- **Apply / remove labels**: `gh issue edit <number> --repo <tracker> --add-label "..."` / `--remove-label "..."`. Changing a published ticket keeps its ready label: see **Publishing and changing a ticket**.
- **Close**: `gh issue close <number> --repo <tracker> --comment "..."`
- **Blocking**: GitHub's **native issue dependencies** between the tracker's issues, the canonical, UI-visible representation. Add an edge with `gh api --method POST repos/<tracker>/issues/<child>/dependencies/blocked_by -F issue_id=<blocker-db-id>`, where `<blocker-db-id>` is the blocker's numeric **database id** (`gh api repos/<tracker>/issues/<n> --jq .id`, _not_ the `#number` or `node_id`). Read each blocker's own `state` from the dependency edges (`dependencies/blocked_by`, or GraphQL `blockedBy { nodes { number state } }`): `issue_dependencies_summary.blocked_by` can stay stale after a blocker closes and must not decide blocking. Where dependencies aren't available, fall back to a `Blocked by: #<n>, #<n>` line at the top of the child body. A ticket is unblocked when every blocker is closed.

## Publishing and changing a ticket

- **The ready label stays.** A ticket published with the ready label (or another workflow's flow label) keeps it. Whether it can start is said by its blockers alone: native dependencies as in **Blocking**, mirrored in a `## Blocked by` section of the body. Being blocked, wanting the fleet to do something else first, or wanting a human to do it is no reason to take the label off: a blocker says it. To hold a ticket back from the fleet, make it blocked by the ticket that must come first.
- **A started ticket's body is not edited.** A ticket has started once the fleet has left any of its signs: a `convoy:branch` comment naming the worker's branch, a `convoy:worker` comment naming its claim, or a PR that closes it. Before then, edit the body. After, open a new ticket instead, blocked by the original where it uses the original's result: a worker at work read the body when it started and never sees a later edit.
- **Real blockers only.** Walk the ticket's acceptance criteria and make it blocked by every ticket a criterion truly needs. A ticket that is only nice to have is no blocker: the body names it and says "use it if present, don't wait for it". Before a migration deletes an old implementation, every capability of the old one has a ticket that blocks the deletion.

## Naming an issue

In the tracker, in an issue's body or comment, `#<n>` is the tracker's issue `<n>`. In this repo, in a PR, a commit or a doc, `#<n>` alone is this repo's own issue or PR: name a tracker's issue `<tracker>#<n>`, and close one from a PR with `Closes <tracker>#<n>` — of a local tracker, `Closes local/<host>#<n>`, which the landing turn of that host closes.

## Pull requests as a triage surface

**PRs as a request surface: no.** _(Set to `yes` if this repo treats external PRs as feature requests; `/triage` reads this flag.)_

PRs are this repo's, never the tracker's: run `gh pr` in the checkout, with no `--repo`. When set to `yes`, PRs run through the same labels and states as issues, using the `gh pr` equivalents:

- **Read a PR**: `gh pr view <number> --comments` and `gh pr diff <number>` for the diff.
- **List external PRs for triage**: `gh api --paginate 'repos/{owner}/{repo}/pulls?state=open' --jq '.[] | select(.author_association | IN("OWNER","MEMBER","COLLABORATOR") | not) | {number, title, author: .user.login, author_association, labels: [.labels[].name]}'`.
- **Comment / label / close**: `gh pr comment`, `gh pr edit --add-label`/`--remove-label`, `gh pr close`.

GitHub shares one number space across a repo's issues and PRs, so a bare `#42` in this repo may be either, and both are this repo's own: resolve with `gh pr view 42` and fall back to `gh issue view 42`, both with no `--repo`.

## When a skill says "publish to the issue tracker"

Create an issue in the tracker, as in **Create an issue** above.

## When a skill says "fetch the relevant ticket"

Read it as in **Read an issue** above.

## Wayfinding operations

Used by `/wayfinder`. The **map** is a single issue with **child** issues as tickets, all in the tracker.

- **Map**: a single issue labelled `wayfinder:map`, holding the Notes / Decisions-so-far / Fog body: the create line with `--label wayfinder:map` in place of the ready label.
- **Child ticket**: an issue linked to the map as a GitHub sub-issue (see **Make an issue a sub-issue of a parent**). Where sub-issues aren't enabled, add the child to a task list in the map body and put `Part of #<map>` at the top of the child body. Labels: `wayfinder:<type>` (`research`/`prototype`/`grilling`/`task`). Once claimed, the ticket is assigned to the driving dev.
- **Blocking**: as in **Blocking** above.
- **Frontier query**: list the map's open children (as in **List issues**, scoped to the map's sub-issues / task list), drop any with an open blocker (a blocker's own state is open, including issues in a fallback `Blocked by` line) or an assignee; first in map order wins.
- **Claim**: `gh issue edit <n> --repo <tracker> --add-assignee @me`, the session's first write.
- **Resolve**: `gh issue comment <n> --repo <tracker> --body "<answer>"`, then `gh issue close <n> --repo <tracker>`, then append a context pointer (gist + link) to the map's Decisions-so-far.
