A first start settles a new host's host settings with the user before its fleets start.

## Tell the user

Explain these briefly, in the user's words. A checkout's fleet reads and writes its issues in its host's local tracker, `local/<host>`: a database on this host that only this host's user writes, which `afkconvoy issue` reads and writes. No repo is asked for: a checkout works the local tracker unless `afkconvoy run --tracker <owner>/<repo>` names a private GitHub repo to hold its issues instead — its own among them. Anyone who can comment on a public repo's issues writes what workers read, so a public one is refused.

{const:FIRST_START_SETTINGS}

## Set up without asking

Do what requires no user decision: give Orca this checkout with `orca repo add --path "$PWD"`, read `afkconvoy config`, and run `afkconvoy check` in a bound repo. Read `afkconvoy plan` once the host settings are settled. Do not start a fleet just to get a plan before the user answers. A plan may push and delete its startup probe; `afkconvoy doctor` reads without writing.

Leave the host kit to `run`, which writes one where none exists, without asking. Leave the flow labels to `afkconvoy flows`, which makes them where the tracker needs them. Do not ask the user to perform those steps or pick their contents. Install nothing and log nobody in: a missing command or login names the line the user must run.

## Ask the user

Ask every applicable question below in one message, recommending an answer for each: settle the operator's watch directory on a host with no host settings yet. Existing choices stand unless the user asks to change them. Run the lines only after the user answers. Replace placeholders with their answers and combine the selected flags in one `afkconvoy run <checkout>`.

{const:FIRST_START_QUESTIONS}

A first `run` of a checkout off the list, on a host with no host settings and no `--watch-dir`, refuses before writing anything. Its JSON has `error`, `asks` (each unanswered setting, question, default, recommendation as `recommend`, and every option with its `line`) and `see`, the command that prints this topic. A flag's environment variable counts as saying it. A checkout already on the list and host settings already kept run as before.
