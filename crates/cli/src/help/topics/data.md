The directory of what nothing else gives back: where, what moves it, what it holds. The helps that speak of "the host's database" mean `host.db` in it. What can be made again is in the cache directory instead (`afkconvoy help cache`).

## Where it is

- `$XDG_DATA_HOME/afkconvoy`: where XDG_DATA_HOME is set and not empty
- `$HOME/.local/share/afkconvoy`: otherwise — on macOS too: never ~/Library/Application Support

It is made when something is first kept in it. With neither variable set, a command that needs it fails and says so.

## What moves it

Setting XDG_DATA_HOME, and nothing else: there is no flag and no config key for it. Each command reads the variable for itself, so two commands read the same database only under the same value. The host runner keeps the directory of the `run` that started it, whatever its login shell sets, and so does every fleet instance, worker and gate it starts.

## What it holds

- `host.db`: the host's database (SQLite), read and written by every process of the fleet on the host, each change one transaction, and readable by its user alone (mode 0600). Its schema has a version, and each change of it is a migration: the first process of a newer release to open it migrates it forward once, and an older release that finds it newer refuses, saying so — install the newer one. It holds the host's own state, which no other host reads:
  - the checkout list and, beside it, the host settings (`afkconvoy run --help`)
  - the host kit, and when it was last written and a refresh of it last tried (`afkconvoy kit --help`)
  - the candidate chain of each repo and target branch (`afkconvoy land --help`), and each repo's landing hold (`afkconvoy hold --help`) and fresh looks
  - the harnesses paused, the book of worker health and the harnesses whose workers last took their briefs
  - the account book: what the host last heard of each engine account, its usage windows and the limit it met — never a credential — and the usage of each patrol run
  - the watch's and the asking session's state and launch records, the resource episodes, and the outbox of human decisions
  - each trash directory a discard used
- `backups/<YYYY-MM-DD>/`: the host runner's daily copy of `host.db`, and of `issues.db` where there is one, made by SQLite's online backup while others write. The newest {const:BACKUPS_KEPT} days are kept. A backup that failed is `doctor`'s finding (`the host database`), and is tried again within the hour.

None of it is pushed or sent anywhere: a host's disk and its backups are its only copies.

## From the cache directory

An older release kept these as files in the cache directory: `host.db` itself; `checkouts.json` with `checkouts.lock`; `paused.json`; `worker-health.json`; `accounts/book.json` with `accounts/book.lock`, and `accounts/patrol-runs.json`; `asking/state.json` and `asking/terminal.json`; `watch/state.json`, `watch/terminal.json`, `watch/resources.json` with `watch/resources.lock`, `watch/notify.json` and `watch/delivered.json`; and `trash-dirs`. In each repo's git dir it kept `convoy-landing-hold` and `convoy-fresh-looks/`. In the config directory ($XDG_CONFIG_HOME/afkconvoy, or $HOME/.config/afkconvoy) it kept the host kit, `kit.json`, with `kit.lock` and `kit.refresh`: its modification time is taken in as when it was last written, and `kit.refresh` as when a refresh was last tried; a `kit.json.bak-*` copy is not read. An entry of the checkout list that said no tracker then meant its checkout's own repo, and is taken in naming that repo; one whose repo cannot be read from its remote says none still, and `doctor` names it. The first process of this release that reads one takes it into `host.db`, in the transaction that records it did, and renames the file `<name>.migrated` beside where it was, kept: `host.db` is copied whole, by SQLite's backup. While a host runner of the older release still holds the host — until it moves onto this one between two ticks — the file stays where that runner reads and writes it, and what it writes there is taken in again; once it moved, the file is renamed. Renamed back, the older release reads it again. Taking in a file already taken in changes nothing. `watch/daybook.json`, the queue of daybook entries an older release had not written yet, is not taken into `host.db`: each entry it still holds is written to `daybook.db` once, and the file is renamed the same way.
