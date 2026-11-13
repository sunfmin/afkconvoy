A first start settles a new checkout's tracker and a new host's host settings with the user before its fleets start.

## Tell the user

Explain these briefly, in the user's words. A tracker is the private repo the fleet reads and writes its issues in. Anyone who can comment on its issues writes what workers read, so a public repo cannot be its own tracker. Two people working one repo need their own trackers; those choices stay on their hosts and are never pushed. `afkconvoy checkouts` shows the trackers other checkouts on this host already use.

{const:FIRST_START_SETTINGS}

## Set up without asking

Do what requires no user decision: give Orca this checkout with `orca repo add --path "$PWD"`, read `afkconvoy config`, and run `afkconvoy check` in a bound repo. Read `afkconvoy plan` once the tracker is settled: a plan uses the tracker the checkout already says, or this repo until a first `run` records the choice, and refuses a public one. Do not start a fleet just to get a plan before the user answers. A plan may push and delete its startup probe; `afkconvoy doctor` reads without writing.

Leave the host kit to `run`, which writes one where none exists, without asking. Leave the tracker labels to `run --tracker`, and the flow labels to `afkconvoy flows`, which makes them. Do not ask the user to perform those steps or pick their contents. Install nothing and log nobody in: a missing command or login names the line the user must run.

## Ask the user

Ask every applicable question below in one message, recommending an answer for each. Settle the tracker for a checkout not on `afkconvoy checkouts`; settle the operator's watch directory on a host with no host settings yet. Existing choices stand unless the user asks to change them. Create a new private repo only on the user's yes to its owner and name. Run the lines only after the user answers. Replace placeholders with their answers and combine the selected flags in one `afkconvoy run <checkout>`; separate `run` lines may still lack another required answer.

{const:FIRST_START_QUESTIONS}

A first `run` without `--tracker` for a checkout off the list refuses before writing anything. Its JSON has `error`, `asks` (each unanswered setting, question, default, recommendation as `recommend`, and every option with its `line`) and `see`, the command that prints this topic. A flag's environment variable counts as saying it. The watch directory keeps its default if no flag names it. A checkout already on the list and host settings already kept run as before.
