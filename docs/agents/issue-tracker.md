# Issue tracker: GitHub

Issues and specs for this repo live as GitHub issues in its **tracker** (`GLOSSARY.md`, `docs/adr/0027`): the repo whose issues the fleet of this checkout works. Whose issues a fleet works is a fact of its host, never pushed, so this file names no tracker: ask the host. Use the `gh` CLI for all operations.

## Find the tracker

```sh
afkconvoy checkouts | jq -r --arg root "$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")" \
  '.checkouts[] | select(.checkout == $root) | .tracker'
```

It prints the `tracker` that `afkconvoy checkouts` answers for this checkout, whose entry is its main worktree's: the fleet reads it the same way from any worktree of it. Two cases, which differ only in the repo label:

- **It names a tracker**, `<owner>/<repo>`: the issues are there, among other repos' issues. This repo's carry its **repo label**, `repo:sunfmin/afkconvoy`. An issue there without it is no issue of this repo, and no fleet works it.
- **It prints `null`, or nothing** (the checkout is on no checkout list of this host): the tracker is this repo itself, and no repo label is used. An issue here that carries a repo label is the issue of the repo that label names, not this one's.

Below, `<tracker>` is the tracker it names, or this repo, as its `origin` remote names it.

## Conventions

Put `--repo <tracker>` on every `gh issue` command, and `repos/<tracker>` in every `gh api` path of an issue: run in the checkout without it, `gh` acts on this repo's own issue of that number.

- **Create an issue**: run `afkconvoy flows` in the checkout, once for all the issues you open. Its `line` (`flows --help`) opens an issue in the tracker, with the repo label where the tracker is another repo, and `flows` creates there the labels that line names. Give it a title and a body file. The ready label on that line hands the issue to the fleet: for an issue not ready to be worked (`needs-triage`, an epic, a wayfinder map), put the label the skill asks for in its place, and keep `--repo` and the repo label.
- **Read an issue**: `gh issue view <number> --repo <tracker> --json number,title,body,labels,comments`.
- **List issues**: `gh issue list --repo <tracker> --label repo:sunfmin/afkconvoy --state open --json number,title,body,labels,comments --jq '[.[] | {number, title, body, labels: [.labels[].name], comments: [.comments[].body]}]'` with appropriate `--label` and `--state` filters. Where the tracker is this repo, leave out `--label repo:sunfmin/afkconvoy`.
- **Make an issue a sub-issue of a parent**: `--parent <parent>` on the create line, or `gh issue edit <parent> --repo <tracker> --add-sub-issue <child>` afterwards (`gh` 2.94+). Older `gh`: `gh api --method POST repos/<tracker>/issues/<parent>/sub_issues -F sub_issue_id=<child-db-id>` (database id, as in **Blocking** below). Without sub-issues, put `Part of #<parent>` at the top of the child body.
- **Comment on an issue**: `gh issue comment <number> --repo <tracker> --body "..."`
- **Apply / remove labels**: `gh issue edit <number> --repo <tracker> --add-label "..."` / `--remove-label "..."`. The tracker refuses a label it lacks: `gh label create <label> --repo <tracker>` first.
- **Close**: `gh issue close <number> --repo <tracker> --comment "..."`
- **Blocking**: GitHub's **native issue dependencies** between the tracker's issues, the canonical, UI-visible representation. Add an edge with `gh api --method POST repos/<tracker>/issues/<child>/dependencies/blocked_by -F issue_id=<blocker-db-id>`, where `<blocker-db-id>` is the blocker's numeric **database id** (`gh api repos/<tracker>/issues/<n> --jq .id`, _not_ the `#number` or `node_id`). Read each blocker's own `state` from the dependency edges (`dependencies/blocked_by`, or GraphQL `blockedBy { nodes { number state } }`): `issue_dependencies_summary.blocked_by` can stay stale after a blocker closes and must not decide blocking. Where dependencies aren't available, fall back to a `Blocked by: #<n>, #<n>` line at the top of the child body. A ticket is unblocked when every blocker is closed.

## Naming an issue

In the tracker, in an issue's body or comment, `#<n>` is the tracker's issue `<n>`. In this repo, in a PR, a commit or a doc, `#<n>` alone is this repo's own issue or PR: name a tracker's issue `<tracker>#<n>`, and close one from a PR with `Closes <tracker>#<n>`.

## Pull requests as a triage surface

**PRs as a request surface: no.** _(Set to `yes` if this repo treats external PRs as feature requests; `/triage` reads this flag.)_

PRs are this repo's, never the tracker's: run `gh pr` in the checkout, with no `--repo`. When set to `yes`, PRs run through the same labels and states as issues, using the `gh pr` equivalents:

- **Read a PR**: `gh pr view <number> --comments` and `gh pr diff <number>` for the diff.
- **List external PRs for triage**: `gh api --paginate 'repos/{owner}/{repo}/pulls?state=open' --jq '.[] | select(.author_association | IN("OWNER","MEMBER","COLLABORATOR") | not) | {number, title, author: .user.login, author_association, labels: [.labels[].name]}'`.
- **Comment / label / close**: `gh pr comment`, `gh pr edit --add-label`/`--remove-label`, `gh pr close`.

GitHub shares one number space across a repo's issues and PRs, so a bare `#42` in this repo may be either: resolve with `gh pr view 42` and fall back to `gh issue view 42`.

## When a skill says "publish to the issue tracker"

Create an issue in the tracker, as in **Create an issue** above.

## When a skill says "fetch the relevant ticket"

Read it as in **Read an issue** above.

## Wayfinding operations

Used by `/wayfinder`. The **map** is a single issue with **child** issues as tickets, all in the tracker.

- **Map**: a single issue labelled `wayfinder:map`, holding the Notes / Decisions-so-far / Fog body: the create line with `--label wayfinder:map` in place of the ready label.
- **Child ticket**: an issue linked to the map as a GitHub sub-issue (see **Make an issue a sub-issue of a parent**). Where sub-issues aren't enabled, add the child to a task list in the map body and put `Part of #<map>` at the top of the child body. Labels: `wayfinder:<type>` (`research`/`prototype`/`grilling`/`task`). Once claimed, the ticket is assigned to the driving dev.
- **Blocking**: as in **Blocking** above.
- **Frontier query**: list the map's open children (`gh issue list --repo <tracker> --label repo:sunfmin/afkconvoy --state open`, scoped to the map's sub-issues / task list), drop any with an open blocker (a blocker's own state is open, including issues in a fallback `Blocked by` line) or an assignee; first in map order wins.
- **Claim**: `gh issue edit <n> --repo <tracker> --add-assignee @me`, the session's first write.
- **Resolve**: `gh issue comment <n> --repo <tracker> --body "<answer>"`, then `gh issue close <n> --repo <tracker>`, then append a context pointer (gist + link) to the map's Decisions-so-far.
