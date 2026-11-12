# afkconvoy config

What `afkconvoy` reads for this repo: the yaml block below, its gate budget,
and the cut steps under "Cutting a version". An unknown key or a wrong
type is an error, not a default.

```yaml
base_branch: main                 # workers cut from it, open their PR against it, land on it
gate_command: afkconvoy exclusive -- ./gate.sh  # what must pass before a PR lands, one at a time on the host
gate_seconds: 3600                # the gate limit, starting after its turn is granted
ready_label: ready-for-agent      # an issue is dispatchable while it carries this
epic_labels: [epic, prd]          # never dispatched
escalate_label: ready-for-human   # swapped in for ready_label when the fleet gives an issue up
```

`base_branch` and `gate_command` have no default. The rest default to the values
shown, and so does the one left out here: `lock_label`, `fleet-lock`.

The warning budget is separate from termination. This block lets an older
installed afkconvoy still land and cut the version that introduces it; older
versions skip it and keep their existing warning rule until upgraded.

```yaml gate-budget
gate_budget_seconds: 600
```

This fixed ten-minute budget is based on the 14-core, 48-GiB Apple Silicon
host's complete gate, including builds and checks, with the observations and
cache/load limits in [the gate baseline](../gate-baseline.md). Five consecutive
recorded landings on one host must each exceed it before the existing slow-gate
finding opens one issue. Queue waits count toward neither budget nor limit.
The one-hour gate limit still bounds termination, leaving cold builds and host
competition room to finish. A warning alone fails no validation. Change the
budget only with a deliberate baseline review; never raise it per run.

The gate is two gates, run one after the other by `./gate.sh` (ADR-0047):
the fast gate, which a worker passes before it opens or updates its PR, and
the slow gate, which runs the tests `.config/slow-tests` marks slow. A landing
turn runs the fast gate on each candidate and the slow gate once per round, at
the candidate chain's tip (ADR-0048). Older versions skip this block, and land
on `gate_command`, both; this repo's fleet is upgraded by hand before it
lands with rounds.

```yaml gates
fast:
  command: afkconvoy exclusive -- ./gate.sh fast
  seconds: 600                    # its limit: room for a cold build
  budget_seconds: 180             # 137s measured on Offices-MacBook-Pro.local
slow:
  command: afkconvoy exclusive -- ./gate.sh slow
  seconds: 3000                   # with the fast gate's, gate_seconds
  budget_seconds: 420             # with the fast gate's, gate_budget_seconds
```

The two limits add up to `gate_seconds` and the two budgets to
`gate_budget_seconds`: the split raises neither. The fast gate's budget is
its measured 137 seconds with about a third more for load; the slow gate's is
what is left of the whole gate's, which its measured 1,157 seconds on the
8-core host already exceeded. Its worlds running the real git and not
Apple's shim, twelve tests at a time, it took 665 seconds there
(sunfmin/control#234): still past it. The measurements are in
[the gate baseline](../gate-baseline.md).

How many claims are at work at once is no key here: each host that works this
repo says its own, `afkconvoy run <checkout> --concurrency N`, and 3 until it
does (`docs/adr/0025`).

This repo names no `team`, so it is unbound. A bound repo's three keys more —
`team`, `patrols`, `intake_limit` — are in `afkconvoy config --help`.

The gate checks the core crate with Flux, and `./verify.sh` checks it with
Homebrew's `sunfmin/tap/flux-toolchain`, and refuses to while it is not
installed or is older than the newest the tap has, the tap read fresh for
the check (sunfmin/control#735, ADR-0067). The tap builds
it every day, with the `cargo-flux`, `fixpoint` and `z3` it runs, so nothing
here pins or builds any of them. So a host without it is refused at the
startup probe, and its work goes to a host that has it (ADR-0053). The check
is `verify.sh`'s own, run without checking anything, and how to install it
is what the check says when it fails: `./verify.sh install`, which is `brew
install sunfmin/tap/flux-toolchain`, or `brew upgrade` of it where it is
installed and older, and does nothing where it is the newest. It is the
need's `install`, so the host runs it where the check fails, then probes
again. Its limit covers a bottle's download, with room for a slow network:
nothing is built. The Rust nightly the tap's Flux runs on is added by its
first run, the gate's.

The gate's tests run `node` too: the dsh harness's scripts are JavaScript,
and its tests run them with the `node` on the PATH, as a worker's dsh does.
So a host whose runner has no `node` it can run is refused at the same probe,
and its gate work goes to a host that has one (sunfmin/control#506). The
check runs `node` itself, with the PATH the runner gives the gate, so a shim
with no node behind it fails as well. No version is pinned: the tests ask for
none but what runs a `.mjs` file, and a pin through a version manager would
put that manager on every host's PATH and a download on each.

```yaml gate-needs
checks:
  - name: flux-toolchain
    command: ./verify.sh --toolchain
    seconds: 60
    install: ./verify.sh install
    install_seconds: 1800           # a download: nothing is built
  - name: node
    command: |
      node --version >/dev/null 2>&1 || {
        echo "the gate runs node, and the host runner cannot run one on its PATH: $PATH" >&2
        echo "put a node on that PATH: install one under \$HOME (~/homebrew's brew install node, or mise use -g node), or link an existing node into ~/.local/bin, then restart the host runner" >&2
        exit 1
      }
    seconds: 30
```

## Heavy work on the host

One gate fills the host by itself: its Flux, clippy and test builds run one
rustc job per core, and its tests run as many at a time as
`.config/nextest.toml` says, each starting the CLI and the `git`, `gh` and
`orca` it runs. Two gates side by side already reach the
load at which admission holds new workers back, twice the host's cores (#426).
So the gate runs through the host's exclusive queue (ADR-0019), in every
checkout — a worker's standalone gate or landing turn, and a gate in a
temporary checkout that certifies the base branch: by default one command
at a time across its fleets. The host's `gate_lanes` setting may allow more. The workers keep editing while their gates
wait. A worker's fast gate, `afkconvoy gate --fast`, waits on none of these: it
takes the host's fast lane, beside the other fast gates up to the host's
`fast_lanes` setting, whatever their repo (ADR-0049).

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

The gate limit, `gate_seconds`, starts after the queue grants its turn. A
nested `exclusive` in the gate command inherits that turn, so it waits for
no second lane. The recorded gate duration measures the gate command itself.
A cut's overall limit still includes its queue waits; its commands go ahead
of waiting workers so their gates cannot consume that limit (ADR-0028).

Fewer workers at once (`--concurrency`) would also keep gates apart, but would
slow every worker's editing as well, and still let two gates overlap. The queue
bounds only the heavy step.

## Cutting a version

The fleet cuts each version of afkconvoy itself, once it goes idle with landed
work past the newest `v*` tag, or the oldest landing has waited five hours
while busy: it sets the version, lands one commit on the base branch through the
landing turn — the commit that set the version stays on the cut's branch — tags
that commit, and publishes and installs it on its host
(`afkconvoy cut --help`, `docs/adr/0028`). These are its steps, and the one
place they are written. Each is run by `sh` in the cut's checkout, with the
version in `$VERSION`: `set` on the base branch, `publish` with the new tag
checked out.

An untagged repo can declare `initial: 0.1.0` beside `set` and `publish` in
its cut block. The next idle tick cuts that exact first version through the
same gate and landing turn, even if `set` finds it already set. Without the
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

The bound is in a second cut block: afkconvoy versions that know cut steps
but not this key read only the first one and skip this addition.

```yaml cut
busy_after_seconds: 18000
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
  between ticks once no exclusive command runs (`afkconvoy run --help`).
