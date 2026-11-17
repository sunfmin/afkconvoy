The directory of what the fleet can make again: where, what moves it, what it holds. The helps that speak of a file under "the cache directory" mean this one. What nothing else gives back — the checkout list, the host settings and the host's other state — is in the host's database, in the data directory instead (`afkconvoy help data`).

## Where it is

- `$XDG_CACHE_HOME/afkconvoy`: where XDG_CACHE_HOME is set and not empty
- `$HOME/.cache/afkconvoy`: otherwise — on macOS too: never ~/Library/Caches

It is made when something is first kept in it. With neither variable set, a command that needs it fails and says so.

## What moves it

Setting XDG_CACHE_HOME, and nothing else: there is no flag and no config key for it. To keep everything under a directory of your choosing:

```sh
XDG_CACHE_HOME="$HOME/afkconvoy-cache" afkconvoy check
```

Each command reads the variable for itself, so two commands find the same copies and the same log only under the same value: a `doctor` run without the one a `run` was started with reads another directory, and finds no log there. The host runner keeps the directory of the `run` that started it, whatever its login shell sets, and so does every fleet instance it holds; a `run` under another value asks the host runner of that directory, and starts one there where none answers.

## What it holds

- `candidates/<repo-key>/<candidate-id>/`: a candidate's detached worktree, kept until it stops; its adjacent `.processes` file identifies its gate's descendants. The repo key hashes the canonical common git directory. Keep the cache outside every checkout: `land` refuses a cache inside a checkout so gate tools cannot discover that checkout's configuration above a candidate. `afkconvoy land --help` describes removal and retained evidence.
- `copies/`: the fleet's copy of a team's repo and of each role's, one per commit, read-only: copies/<owner>/<repo>/<commit>. Derived copies name their kind and directory too. Claude Code's role copy is `<commit>-role+claude-code-<directory>` (the directory is empty for a whole repo), with an empty `.git` directory to bound native skill discovery. A worker of a bound repo starts in one; an unbound repo has none.
- `cut-failed/<owner>/<repo>/`: the whole output of each red gate run of a cut, `<version>-<seconds>.log`, named for the version and when the run started: the file the failed cut's issue names, which the next cut, removing the cut's checkout, leaves. The newest {const:CUT_KEPT_RED} of a repo stay. `afkconvoy cut --help`.
- `shared-readings/<host>/<login>/`: a lock for each GitHub reading the host's waiting processes share, held while one of them reads GitHub: a second waiter waits on the first one's read and answers from what it kept, in the host's database. An older release kept them, with the quota observations, under `github-quota/`, which the first process of this release moves aside to `github-quota.migrated` once no host runner of another build holds the host; its observations are not taken in, for each expires within the hour.
- `harness/`: the user directory the fleet keeps for pi, Codex, Kimi Code and Grok, a worker's in place of yours: harness/pi, harness/codex, harness/kimi, harness/grok. A Kimi or Grok worker's role-specific home is under that directory, so concurrent roles do not share hooks or rules.
- `runs/`: the tick log of each repo's fleet instance, one file per repo: what `doctor` reads. It keeps every line of the last {const:LOG_KEPT_HOURS} hours, and never fewer than {const:LOG_KEPT_LEAST} lines nor more than {const:LOG_KEPT_MOST}, the oldest dropped first.
- `host-runner.lock`: held by the host runner as long as it runs, so a host has one; it says its pid, its version and when it started.
- `host-runner.sock`: the host runner's socket, the one socket of the host: `run`, `stop`, `wake`, `panel --open`, `exclusive` and `doctor` speak to the host runner there, each naming its checkout. No fleet instance and no repo has a socket of its own. One nobody listens on — a host runner killed with no goodbye left it — is removed by whoever finds it.
- `host-runner.log`: the host runner's log, moved aside to host-runner.log.1 once past a MiB: what it started, added, refused and stopped, each line stamped in UTC with its zone named: `2026-10-30T13:56:16Z checkout added: …`.
- `stock-watch/`: the stock watch's directory, made and kept when the host names no operator directory. Its rules are the short instruction carried in the binary, refreshed when a stock watch starts.
- `watch/`: the watch's briefs, transcripts and each round's report and delivery record, in either directory, and `evidence/`, the host readings and process or disk evidence. Its state — what survives a host runner restart, what it left with a human, the resource episodes — is in the host's database. `state.lock` is held across a look or a check, and by `watched` while it records a report.
- `asking/`: the asking session's briefs and transcripts (`sessions/`), each question's delivery record (`questions/`) and each report (`reports/`); `steps`, how many steps ran. Its state and its launch record are in the host's database; `state.lock` is held across a step, and by `asked` while it records a report.
- `worker-health.lock`: held by the one fleet instance observing the host's workers, across its whole observation; the book of what it observed is in the host's database.

None of it is fleet state: that is in GitHub and on the repo's remote.

## Beside it: the data directory

What nothing else can give back is not in the cache but in afkconvoy's data directory: `$XDG_DATA_HOME/afkconvoy` where XDG_DATA_HOME is set and not empty, else `$HOME/.local/share/afkconvoy`. It holds `issues.db`, the host's local tracker, and `daybook.db`, the host's daybook (`afkconvoy daybook --help`), each made by its first write.

## See also

- `afkconvoy check --help`: when a copy is cloned, and what is read from it
- `afkconvoy run --help`: the log
- [`afkconvoy help worker-launch`](help:worker-launch): what each harness's user directory holds
- `afkconvoy doctor --help`: what is read back from the log and the copies
