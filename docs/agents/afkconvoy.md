# afkconvoy config

What `afkconvoy` reads for this repo: the yaml block below, the cut steps under
"Cutting a version", and nothing else in this file. An unknown key or a wrong
type is an error, not a default.

```yaml
base_branch: main                 # workers cut from it, open their PR against it, land on it
gate_command: afkconvoy exclusive -- ./gate.sh  # what must pass before a PR lands, one at a time on the host
gate_seconds: 3600                # the gate limit: the wait in the queue counts toward it
ready_label: ready-for-agent      # an issue is dispatchable while it carries this
epic_labels: [epic, prd]          # never dispatched
escalate_label: ready-for-human   # swapped in for ready_label when the fleet gives an issue up
```

`base_branch` and `gate_command` have no default. The rest default to the values
shown, and so does the one left out here: `lock_label`, `fleet-lock`.

How many claims are at work at once is no key here: each host that works this
repo says its own, `afkconvoy run <checkout> --concurrency N`, and 3 until it
does (`docs/adr/0025`).

This repo names no `team`, so it is unbound. A bound repo's three keys more —
`team`, `patrols`, `intake_limit` — are in `afkconvoy config --help`.

## Heavy work on the host

One gate fills the host by itself: its Flux, clippy and test builds run one
rustc job per core, and its tests run six at a time, each starting the CLI and
the `git`, `gh` and `orca` it runs. Two gates side by side already reach the
load at which admission holds new workers back, twice the host's cores (#426).
So the gate runs through the host's exclusive queue (ADR-0019), in every
checkout — a worker's `afkconvoy gate` and `gate --train`, the landing train's
gate, and a gate in a temporary checkout that shows a red base branch: one at a
time on the host, across its fleets. The workers keep editing while their gates
wait.

- Run `afkconvoy gate` as it is. Never wrap it in another `afkconvoy exclusive`:
  its own would wait behind it, on itself, until the gate limit.
- A gate run by hand, outside `afkconvoy gate`, is
  `afkconvoy exclusive -- ./gate.sh`.
- A suite or a build of the whole workspace run on its own, outside the gate,
  goes through the queue too: `afkconvoy exclusive -- cargo nextest run
  --workspace`, `afkconvoy exclusive -- cargo clippy --workspace
  --all-targets`, and so `cargo test --workspace`. One test, a build of one
  crate, `cargo fmt`, git and editing do not: they would wait behind every
  gate of the host for no reason.

The wait in the queue counts toward the gate limit, `gate_seconds`, as the run
does, and so toward the time a recorded gate run keeps, by which the gate is
judged slow. This repo's own gates fit in the hour: the most that can queue is
every worker's and the train's, with six workers seven gates of about four and
a half minutes each (the slowest green run on record took 273 seconds, beside
other gates), about 32 minutes. Other fleets' commands in the queue add to the
wait: bizkernel's gate goes through it too, and in this host's tick logs up to
24 October 2026 its 340 runs took 197 seconds at the median and 734 at most,
and a command waited 1022 seconds at most. A gate that reaches the limit while
it waits is red (`timed_out`), and is run again. Where that keeps happening,
`afkconvoy doctor` shows the queue, and the limit is raised here, the one place
it is written, for every host.

Fewer workers at once (`--concurrency`) would also keep gates apart, but would
slow every worker's editing as well, and still let two gates overlap. The queue
bounds only the heavy step.

## Cutting a version

The fleet cuts each version of afkconvoy itself, once it goes idle with landed
work past the newest `v*` tag: it sets the version, lands it through the
landing train, tags it, and publishes and installs it on its host
(`afkconvoy cut --help`, `docs/adr/0028`). These are its steps, and the one
place they are written. Each is run by `sh` in the cut's checkout, with the
version in `$VERSION`: `set` on the base branch, `publish` with the new tag
checked out.

An untagged repo can declare `initial: 0.1.0` beside `set` and `publish` in
its cut block. The next idle tick cuts that exact first version through the
same gate and landing train, even if `set` finds it already set. Without the
declaration it stays untagged; after the first version tag exists, the usual
patch/minor selection applies and the declaration is ignored. `plan` shows
the declaration as `cut.initial` while there is no version tag.

```yaml cut
set: |
  set -ex
  packaging/release.sh set "$VERSION"
publish: |
  set -ex
  packaging/release.sh check "$(git describe --tags --exact-match)"
  afkconvoy exclusive -- packaging/release.sh build
  packaging/release.sh package
  packaging/release.sh publish
  brew update --quiet
  brew upgrade sunfmin/tap/afkconvoy
  test "$("$(brew --prefix)/bin/afkconvoy" --version)" = "afkconvoy $VERSION"
```

- `set -ex` first: `sh` runs on past a line that fails and judges the step by
  its last line, and the cut's log (`convoy-cut.log`) shows each line as it
  runs, so a failed step shows the line it failed at.
- `build` makes both release binaries, one rustc job per core: it waits its
  turn in the host's exclusive queue, as a gate does (above).
- `publish` writes the archives and the formula to `sunfmin/homebrew-tap`: the
  host's `gh` login must be able to write there.
- `brew update` comes first: `brew upgrade` alone fetches the tap at most once
  a day, and would find nothing newer. The last line fails the step unless the
  afkconvoy Homebrew links is the version cut. The host runner moves onto it
  once its fleets are idle (`afkconvoy run --help`).
