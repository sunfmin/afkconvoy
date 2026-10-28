A team is a directory — a whole repo, or a subdirectory of one — that names a set of roles and holds the workflows and patrols they work by. Read this to write a team, bind a repo to one, or understand a fault from `afkconvoy check`.

## What its directory holds

- `team.md`: the team file
- `CLAUDE.md`: the team rules: what every worker of the team works under
- `workflows/<name>.md`: one file per workflow: [`afkconvoy help workflows`](help:workflows)
- `patrols/<name>.md`: one file per patrol: [`afkconvoy help patrols`](help:patrols)

Only each definition file's first `` ```yaml `` block is read; the rest explains it.

## The team file

```yaml
escalate_label: {const:ESCALATE_LABEL}    # default
roles:
  <short name>: <its directory>    # owner/repo, owner/repo/path or ./path
  <short name>: { dir: <its directory>, host_user: false }   # its own host_user
roster:                            # optional: `afkconvoy help roster`
  <short name>: { cast: <ignored>, run: [{ harness: pi, model: <model>,
    good_at: <what it does best> }] }   # cast is accepted and ignored; model and good_at optional
host_user: true                    # default
dsh_profile: fleet                # fleet (default) or host-user; DSH only
split: true                        # default; false omits splitting
```

`escalate_label` is the label an issue handed to a human carries in place of its flow label. `roles` gives each role the short name a `by` or a `to` says. `host_user`, true or false, says whether a worker is given the host user's own skills and rules beside its role's ([`afkconvoy help roles`](help:roles)); a role named by `dir` may set its own. `dsh_profile`: `fleet` (default) or `host-user`; independent of `host_user`. A role's `{ dir, dsh_profile }` says its own. Details: [`afkconvoy help worker-launch`](help:worker-launch). `split: false` omits splitting from briefs; true or unsaid offers it, with no guard.

## How it names a role

A role is a directory too, and the team file names it in one of three ways:

- `owner/repo`: a whole repo
- `owner/repo/path`: a subdirectory of a repo: the first two segments are the repo, the rest the path from its root
- `./path`: a subdirectory of the repo the team itself is in

A `./path` is read from the root of the team's repo, not from the team's directory: a team and its roles in one repo name no repo at all, and two teams of one repo name the same role alike. Every role is read at the head of its repo's default branch, and nothing names a branch or a tag. A role's directory is not the team's and does not contain it, is not inside the team's `workflows/` or `patrols/`, and neither contains another role's nor is inside one. It holds:

- `CLAUDE.md`: its rules: what a worker in it always works under
- `.claude/skills/<skill>/SKILL.md`: one directory per skill it can be asked for
- `.claude/settings.json`: its limits for claude-code: hooks, and the permissions it denies
- `.pi/extensions/`: its limits for pi: extensions
- `.codex/hooks.json`: its limits for codex: hooks
- `.dsh/cordis.patch.yml`: its limits for dsh: a DSH overlay
- `.kimi-code/limits.toml`: its limits for kimi: Kimi hooks, and the permissions it denies
- `.grok/config.toml`: its limits for grok: Grok hooks, and the permissions it denies

Only a skill's file has keys `check` reads: [`afkconvoy help skill-file`](help:skill-file). A `do` names an entry of `.claude/skills/` in the role. Rules and skills are written once; limits per harness, in its own form ([`afkconvoy help worker-launch`](help:worker-launch) says how each loads a role).

A worker is given its role's directory alone, and nothing else of its repo: a `CLAUDE.md` or a `.claude/` in a directory above the role is loaded for a session opened there by hand, and a worker never sees it — and a symlink that leaves the role's directory would point at nothing. The team's directory is given to every worker beside its role's, without the directories of its roles and without a `.claude/` at its top: a worker's skills and hooks are its role's.

## The team rules

The `CLAUDE.md` in the team's directory, and optional. Every worker of the team is given it, whatever its role and whatever it was started for — a stage, a consult, a patrol, a landing — and no key turns it off. A role two teams name works under the rules of the team whose stage it is in.

They are one of three kinds of text a worker reads, beside the repo's own rules and its role's rules; the host user's rules give way to all three. Whose:

- **its role's**: the CLAUDE.md of the role's directory, under every harness
- **the team rules**: the CLAUDE.md of the team's directory, under every harness
- **the repo's own**: of the repo an issue is of: under claude-code its CLAUDE.md; under pi, codex, kimi and grok its AGENTS.md, or CLAUDE.md where it has none

Claude Code and pi load the three; the fleet names the other three to Codex, Kimi Code and Grok. What each is for, and where a sentence belongs: [`afkconvoy help roles`](help:roles).

## How a repo is bound to it

By its config, on its base branch (`afkconvoy config --help` shows the file whole):

- `team`: owner/repo or owner/repo/path: naming it is what binds the repo
- `patrols`: which of the team's patrols run here, by name; none by default
- `intake_limit`: the open issues taken in by workers, at most; {const:INTAKE_LIMIT} by default

The team is read at the head of its repo's default branch: pushing is the only way an edit reaches a worker. An issue of a bound repo is the fleet's to work only while it carries a flow label, which `afkconvoy flows` creates: [`afkconvoy help workflows`](help:workflows).

## What `check` holds it to

`afkconvoy check` loads a team and answers every fault at once, each with its file and key; `afkconvoy check --help` lists them all. Those a role's directory can hold:

- A role's directory that holds an `AGENTS.md`, an `AGENTS.MD` or an `AGENTS.override.md` is a fault: pi, Codex and Kimi Code load one in place of `CLAUDE.md`; the fleet adds the link Kimi needs to its own role copy.
- A role that holds limits for one harness and none for another is right for the first alone: a roster entry that lets it run the other is a fault. One with no limits is right for every harness; two that refuse alike are the role's to see to.
- A `.claude/` in the team's directory is no fault and is loaded by no worker: a note.

The legacy `precedents` key is retired and refused with migration instructions. Migrate effective content and source links from the old document into the owning ADR, team or role rules before removing the key. Rulings remain on their issues; no separate document is loaded as an authority.

## Where to read on

- [`afkconvoy help roles`](help:roles): what goes where in a role, and what makes one good
- [`afkconvoy help roster`](help:roster): what each role's workers may run, and an issue's model label
- [`afkconvoy help workflows`](help:workflows): the stages an issue passes through, and every key of them
- [`afkconvoy help patrols`](help:patrols): a role run on a schedule, intake and the fleet lock
- [`afkconvoy help glossary`](help:glossary): each term used here, with its one definition
