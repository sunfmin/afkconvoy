A team's roster says what each of its roles' workers may run: which harnesses and models are eligible; the host kit supplies their order. Read this to write one, to see why a worker started on the harness it did, or to see why a role's stages wait.

## The roster

The `roster` of the team file ([`afkconvoy help team`](help:team)) holds an entry per role it names, by the role's short name:

```yaml
roster:
  dev:
    cast: Hard or wide work on fable; the rest on opus.   # optional
    run:                           # one at least; eligibility, not priority
      - { harness: claude-code, model: opus, good_at: most work }
      - { harness: claude-code, model: fable, good_at: hard or wide work }
      - { harness: pi }            # any model the host kit holds for pi
  reviewer:
    run: [{ harness: codex, model: gpt-5.5 }]
```

Each of `run` names a `harness` — claude-code, pi, codex, dsh or kimi — and may name a `model` by any of its names on that harness — its id, as the host kit holds it and `afkconvoy draft` writes it, or another name it answered to, whatever the case, the provider prefix left off where that leaves one match (`afkconvoy kit --help`) — and `good_at`, what it is good at, which the caster is told. `afkconvoy draft` copies good_at from the host kit's sourced, one-time research; its model supplies only the role's cast guidance. No human edits the host kit to supply it. One that names no model allows any model the host kit holds for the harness, or the harness's command as written where the host kit holds none. `cast` says, in the team's words, how an issue is to be cast among them: the caster is told it too. A role the roster names no entry for may run whatever the host kit holds, but a harness it holds no limits for (below). A model an entry names that matches none the host kit holds for its harness is cut from the range: `check`, `plan` and `doctor` name the entry, what the host kit holds and the nearest names, and a tick opens one issue for it in the team's repo, with that repo's ready label, for the team's own fleet to correct — none while one for it is open. The fleet never changes a roster.

A roster is drafted rather than written from nothing: `afkconvoy draft`, in a checkout of the team, writes one into team.md from this host's host kit — every role, every harness and model in the host kit's order, what the host kit's first model says each is good at and how to cast — and commits nothing. A human reads it, edits it and commits it; the fleet never changes a team's roster.

## A role's range

On a host, a role's range is its entry cut to the host kit (`afkconvoy kit --help`), in the host kit's harness and model order: a harness, or a model, the host kit does not hold is left out. An entry the host kit holds nothing of is not followed: the role ranges over the host kit whole, as if the roster named none for it, and `check`, `plan` and `doctor` name the entry. So is a harness the role holds no limits for while it holds limits for another: no worker of the role runs a harness it holds no limits for, and a host kit may hold every harness its host can start. `afkconvoy check` and `afkconvoy plan` name each harness a range leaves out, and why (`no limits for pi`). `afkconvoy check` answers each role's entry and its range on the host it runs on; `afkconvoy show` draws the roster under the workflows.

## An issue's casting

An issue's casting is its `model:` labels, put on by a caster or a human. Every worker chooses the first eligible harness and model in this machine's host kit order; its role's range and limits remain constraints. A matching model label takes priority over model order only within the selected harness. Legacy `harness:` labels are retained but never select a harness. Labels for other harnesses are kept and apply again when their harness is selected.

Automatic casting defaults off, including for an older kit. `afkconvoy kit --cast on` enables it on this host for every checkout; `afkconvoy kit --cast off` disables it. The setting lives only in the kit and survives rebuilding, daily refresh and runner restarts. With it off, no issue waits for a caster, and `afkconvoy cast` sends no prompt. Human model labels are still followed.

With it on, only a stage's selected harness offering multiple eligible models leaves a choice. A harness with zero or one model needs no caster. The caster receives only the models of selected harnesses still needing a model, in kit order, with `good_at` and the roster's `cast`. It adds only model labels, preserving earlier labels; human casting labels are never recast. A switch or fallback can add a different harness's model beside earlier caster labels. Switching back reuses those labels. An issue waits at most ten minutes, then runs its range's first eligible model.

`afkconvoy switch --harness codex` moves the Codex entry first. A known installed harness the kit does not hold is added first with no model, starts workers on its login's default at once, and is listed, probed and researched only after the answer; that later refresh keeps the switch's order. An added harness that cannot start is reported by the switch and `doctor`. `afkconvoy switch --model sonnet` moves that model first within its harness, and its harness first in the kit; any of its names matches. Ambiguous names and names the kit holds no model for are refused with candidates or nearest names. Other entries and contents stay intact. Every checkout follows these orders; no instance or checkout is required. With no target it leaves existing workers alone. `--all` continues workers across the host's checkout list, preserving their scene and reporting failures separately. Every moved worker's answer and tick-log line explain each earlier skipped harness and what would make it eligible. Change priority by switching again; `--clear` is removed. Old repository choice files are ignored.

Rebuild, daily refresh and new-model research preserve established harness and model order. An unavailable or paused candidate is skipped without reordering; a successful recovery probe makes it eligible again. Fault recovery keeps claims and work, reads real output and uses another available harness for bounded diagnosis when structured signals or an explicit harness usage-limit message do not decide the action. Account quota without a reset continues at once; unresolved waits on unchanged evidence are bounded to one hour. Plan, doctor and panel show the host's cast setting, each issue's casting reason, whether model labels were followed, actual running models and why earlier candidates were skipped.

Every tick first reads Orca's agent state for every worker, the train worker included, from one host-wide `orca worktree ps`. `working` is healthy without screen judgment; the existing time bounds still apply. A failed delivery still goes to classification when the runtime's last report says `working`. `done` with `mainAgent.outcome: failure` is stopped at its first observation: classification rules run at once, then bounded diagnosis where no rule decides the fault, without waiting for two screen observations. A turn ending without failure follows the existing finished, verdict and nudge logic. Orca's `agentType` decides the harness for a continuation or pause when the recorded harness disagrees. Doctor and the panel show each observed worker's Orca state, outcome, agent type and `stateStartedAt` (epoch milliseconds), and name a wrong harness record. Termination is confirmed by a fresh runtime read before any replacement starts.
