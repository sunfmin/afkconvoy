A team's roster says what each of its roles' workers may run: which harnesses, on which models, in what order. Read this to write one, to see why a worker started on the harness it did, or to see why a role's stages wait.

## The roster

The `roster` of the team file ([`afkconvoy help team`](help:team)) holds an entry per role it names, by the role's short name:

```yaml
roster:
  dev:
    cast: Hard or wide work on fable; the rest on opus.   # optional
    run:                           # one at least, in order of preference
      - { harness: claude-code, model: opus, good_at: most work }
      - { harness: claude-code, model: fable, good_at: hard or wide work }
      - { harness: pi }            # any model the host kit holds for pi
  reviewer:
    run: [{ harness: codex, model: gpt-5.5 }]
```

Each of `run` names a `harness` — claude-code, pi, codex or dsh — and may name a `model` by any of its names on that harness — its id, as the host kit holds it and `afkconvoy draft` writes it, or another name it answered to, whatever the case, the provider prefix left off where that leaves one match (`afkconvoy kit --help`) — and `good_at`, what it is good at, which the caster is told. One that names no model allows any model the host kit holds for the harness, or the harness's command as written where the host kit holds none. `cast` says, in the team's words, how an issue is to be cast among them: the caster is told it too. A role the roster names no entry for may run whatever the host kit holds, but a harness it holds no limits for (below). A model an entry names that matches none the host kit holds for its harness is cut from the range: `check`, `plan` and `doctor` name the entry, what the host kit holds and the nearest names, and a tick opens one issue for it in the team's repo, with that repo's ready label, for the team's own fleet to correct — none while one for it is open. The fleet never changes a roster.

A roster is drafted rather than written from nothing: `afkconvoy draft`, in a checkout of the team, writes one into team.md from this host's host kit — every role, every harness and model in the host kit's order, what the host kit's first model says each is good at and how to cast — and commits nothing. A human reads it, edits it and commits it; the fleet never changes a team's roster.

## A role's range

On a host, a role's range is its entry cut to the host kit (`afkconvoy kit --help`), in the roster's order: a harness, or a model, the host kit does not hold is left out. An entry the host kit holds nothing of is not followed: the role ranges over the host kit whole, as if the roster named none for it, and `check`, `plan` and `doctor` name the entry. So is a harness the role holds no limits for while it holds limits for another: no worker of the role runs a harness it holds no limits for, and a host kit may hold every harness its host can start. `afkconvoy check` and `afkconvoy plan` name each harness a range leaves out, and why (`no limits for pi`). `afkconvoy check` answers each role's entry and its range on the host it runs on; `afkconvoy show` draws the roster under the workflows.

## An issue's casting

An issue's casting is its `harness:` and `model:` labels, which the caster (`afkconvoy cast --help`) or a human puts on — the model by any of its names, a caster’s by its id; the caster is given each stage's range, and casts only an issue some range leaves a choice in. A worker for an issue — its stage's, its consult's, its landing's — runs the casting when its role's range holds it, the range's first when it does not or the issue has none, and the range's one where it holds one harness and one model, whatever the casting. `afkconvoy plan` says which, and why. A name that matches nothing never stops work: a casting no range of the issue's workflow's stages holds runs each stage's range's first, `plan` says `unmatched` and names the label, and the issue is told once, in a comment, what ran instead and what the host kit holds. The label stays, and is followed again once the host kit holds it. A patrol's worker, which has no issue, runs its role's range's first. A harness `afkconvoy switch` chose on a host runs every worker there, whatever the roster and the casting say; one the host kit does not hold is not followed, and each worker runs its range's first, `unmatched`.

## What is refused, and what only waits

- A stage, a patrol and a config name no harness and no model: `check` refuses one that does, with the line to delete and the roster entry to write in its place.
- In the roster, `check` refuses an entry for a name that is no role, a `run` that is missing or empty, a harness that is none, an allowance written twice, and a harness the role holds no limits for while it holds limits for another.
- Nothing about a range refuses a start. A role whose range on a host is empty — it holds limits for no harness the host kit holds, paused harnesses left out — has no worker started without its limits: its stages wait, `plan` says why on each issue at one, and `doctor` and the panel name the role, while every other stage goes on. They go on by themselves once the host kit holds a harness it holds limits for: add one with `afkconvoy kit`, or write the role's limits for one the host kit holds.

## Where to read on

- [`afkconvoy help team`](help:team): the team file the roster is a key of
- `afkconvoy draft --help`: a roster drafted from the host kit
- `afkconvoy kit --help`: what this host can start, and on which models
- `afkconvoy cast --help`: the caster, and the labels it puts on
- [`afkconvoy help glossary`](help:glossary): each term used here, with its one definition
