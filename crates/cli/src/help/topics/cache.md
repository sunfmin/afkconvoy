The one directory of its own the fleet keeps on a host: where it is, what moves it, and what it holds. The helps that speak of a file under "the cache directory" mean this one.

## Where it is

- `$XDG_CACHE_HOME/afkconvoy`: where XDG_CACHE_HOME is set and not empty
- `$HOME/.cache/afkconvoy`: otherwise — on macOS too: never ~/Library/Caches

It is made when something is first kept in it. With neither variable set, a command that needs it fails and says so.

## What moves it

Setting XDG_CACHE_HOME, and nothing else: there is no flag and no config key for it. To keep everything under a directory of your choosing:

```sh
XDG_CACHE_HOME="$PWD/.cache" afkconvoy check
```

Each command reads the variable for itself, so two commands find the same copies and the same log only under the same value: a `doctor` run without the one a `run` was started with reads another directory, and finds no log there. The host runner keeps the directory of the `run` that started it, whatever its login shell sets, and so does every fleet instance it holds; a `run` under another value asks the host runner of that directory, and starts one there where none answers.

## What it holds

- `copies/`: the fleet's copy of a team's repo and of each role's, one per commit, read-only: copies/<owner>/<repo>/<commit>. A worker of a bound repo starts in one; an unbound repo has none.
- `harness/`: the user directory the fleet keeps for pi and for Codex, a worker's in place of yours: harness/pi, harness/codex.
- `runs/`: the tick log of each repo's fleet instance, one file per repo: what `doctor` reads. It keeps every line of the last {const:LOG_KEPT_HOURS} hours, and never fewer than {const:LOG_KEPT_LEAST} lines nor more than {const:LOG_KEPT_MOST}, the oldest dropped first.
- `checkouts.json`: the checkout list, the checkouts the host runner works, each with the `--instance`, `--host` and `--worker-no-proxy` its `run` named; `checkouts.lock`, held while it is changed.
- `host-runner.lock`: held by the host runner as long as it runs, so a host has one; it says its pid, its version and when it started.
- `host-runner.sock`: the host runner's socket, the one socket of the host: `run`, `stop`, `wake`, `panel --open` and `doctor` speak to the host runner there, each naming its checkout. No fleet instance and no repo has a socket of its own. One nobody listens on — a host runner killed with no goodbye left it — is removed by whoever finds it.
- `host-runner.log`: the host runner's log, moved aside to host-runner.log.1 once past a MiB: what it started, added, refused and stopped, each line stamped in UTC.
- `host-inspection/`: what the last inspection of the host was given (evidence.md) and what it found (last.json), whichever fleet of the host started it: what `doctor` and the panel show beside admission's reading.

None of it is fleet state: that is in GitHub and on the repo's remote.

## See also

- `afkconvoy check --help`: when a copy is cloned, and what is read from it
- `afkconvoy run --help`: the log, and what each harness's user directory holds
- `afkconvoy doctor --help`: what is read back from the log and the copies
