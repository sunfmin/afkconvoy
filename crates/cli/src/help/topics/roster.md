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

Each of `run` names a `harness` — claude-code, pi, codex or dsh — and may name a `model` by any of its names on that harness — its id, as the host kit holds it and `afkconvoy draft` writes it, or another name it answered to, whatever the case, the provider prefix left off where that leaves one match (`afkconvoy kit --help`) — and `good_at`, what it is good at, which the caster is told. `afkconvoy draft` copies good_at from the host kit's sourced, one-time research; its model supplies only the role's cast guidance. No human edits the host kit to supply it. One that names no model allows any model the host kit holds for the harness, or the harness's command as written where the host kit holds none. `cast` says, in the team's words, how an issue is to be cast among them: the caster is told it too. A role the roster names no entry for may run whatever the host kit holds, but a harness it holds no limits for (below). A model an entry names that matches none the host kit holds for its harness is cut from the range: `check`, `plan` and `doctor` name the entry, what the host kit holds and the nearest names, and a tick opens one issue for it in the team's repo, with that repo's ready label, for the team's own fleet to correct — none while one for it is open. The fleet never changes a roster.

A roster is drafted rather than written from nothing: `afkconvoy draft`, in a checkout of the team, writes one into team.md from this host's host kit — every role, every harness and model in the host kit's order, what the host kit's first model says each is good at and how to cast — and commits nothing. A human reads it, edits it and commits it; the fleet never changes a team's roster.

## A role's range

On a host, a role's range is its entry cut to the host kit (`afkconvoy kit --help`), in the host kit's order: a harness, or a model, the host kit does not hold is left out. An entry the host kit holds nothing of is not followed: the role ranges over the host kit whole, as if the roster named none for it, and `check`, `plan` and `doctor` name the entry. So is a harness the role holds no limits for while it holds limits for another: no worker of the role runs a harness it holds no limits for, and a host kit may hold every harness its host can start. `afkconvoy check` and `afkconvoy plan` name each harness a range leaves out, and why (`no limits for pi`). `afkconvoy check` answers each role's entry and its range on the host it runs on; `afkconvoy show` draws the roster under the workflows.

## An issue's casting

An issue's casting is its `harness:` and `model:` labels, put on by a caster or a human. Every worker chooses the first eligible harness in this machine's host kit order; its role's range and limits remain eligibility constraints. Roster order and ticket labels cannot outrank that order. A matching casting may choose a model within the selected harness, and never carries a model across harnesses. Stock consults, patrols, landing workers, casters and inspections follow the same kit priority.

`afkconvoy switch --harness codex` moves the existing Codex entry first in this host's kit. Every checkout follows it; no instance or checkout is required. With no target it leaves existing workers alone. `--all` continues workers across the host's checkout list, preserving their scene and reporting failures separately. Change priority by switching again; `--clear` is removed. Old repository choice files are ignored.

An unavailable or paused candidate is skipped without reordering the kit. A successful recovery probe makes it eligible again for future starts. Fault recovery keeps claims and work, reads real output and uses another available harness for bounded diagnosis when structured signals do not decide the action. `plan` and `doctor` show kit order, eligible candidates and health evidence; the panel shows recovery actions and actual successors.
