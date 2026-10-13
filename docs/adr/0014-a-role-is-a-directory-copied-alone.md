---
status: accepted
---

# A role is a directory of a repo, and a worker is given that directory alone

A role is a directory: a whole repo, as it has been (ADR-0008), or a subdirectory of one. A team is the same. So one repo can hold a team and every role it names, and a change to a workflow and to the role that performs it is one commit and one review; a role kept in a repo of its own stays right for a role several teams share.

A team's file names a role as `owner/repo`, as `owner/repo/path`, or as `./path` — a path in the repo the team itself is in, so that a team and its roles in one repo name no repo at all and can be forked or renamed without an edit. A project's config names its team as `owner/repo` or `owner/repo/path`. A `./path` is read from the root of the team's repo, not from the team's directory: two teams in one repo can name the same role.

The fleet's copy of a role holds that role's directory and nothing else of its repo. A worker starts in it (ADR-0009), and Claude Code reads a `CLAUDE.md` in every directory above the one it starts in: were the copy the whole repo, the rules at its root would be every role's rules, by an accident of layout, and each role could read its neighbours. The copy of a team is its directory without the directories of roles, for the same reason. "A role's behaviour is what its repo says and nothing else" becomes "what its directory says".

## Considered

- **The whole repo copied, the worker started in the subdirectory.** Rules above the role would be inherited without anything saying so. Rules several roles share may be wanted one day; they should then be designed and named, not fall out of where directories sit. They since were: a team's rules are the `CLAUDE.md` in the team's directory, which every worker of the team is given with that directory. A role's copy is still the role's directory alone.
- **A role's version as the hash of its directory**, so that a push that leaves a role alone leaves its version alone. The version a stage record carries stays the repo's commit: it is what a human can check out. Whether a role *changed* between two commits is asked of its directory's content, not of the two commits being different.
- **A ref beside the path** (`owner/repo/path@v2`). A role is read at the head of its repo's default branch, as before; a push there is still the only way a change reaches a worker.

## Consequences

- A repo is fetched once per commit, and each role's directory and each team's is taken out of it on its own.
- A role opened by hand in its subdirectory is not quite the role a worker gets: Claude Code also loads what is above it. `check` says so when a directory above a role holds a `CLAUDE.md` or a `.claude/` — except for the team's own rules, which a worker is given as well.
- A role's directory holds no symlink that leaves it: in the worker's copy it would point at nothing. Two roles that need the same hook each hold it.
- The directory of a role and that of a team, or of another role, never contain one another; `check` refuses a team's file that says otherwise. Two names may still be given to one directory.
- One role per repo stops being the layout a new team starts from: `scaffold` writes a team with its roles under `roles/`, and the manual teaches that first.
