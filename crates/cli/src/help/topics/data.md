The directory of what nothing else gives back: where, what moves it, what it holds. The helps that speak of "the host's database" mean `host.db` in it. What can be made again is in the cache directory instead (`afkconvoy help cache`).

## Where it is

- `$XDG_DATA_HOME/afkconvoy`: where XDG_DATA_HOME is set and not empty
- `$HOME/.local/share/afkconvoy`: otherwise — on macOS too: never ~/Library/Application Support

It is made when something is first kept in it. With neither variable set, a command that needs it fails and says so.

## What moves it

Setting XDG_DATA_HOME, and nothing else: there is no flag and no config key for it. Each command reads the variable for itself, so two commands read the same database only under the same value. The host runner keeps the directory of the `run` that started it, whatever its login shell sets, and so does every fleet instance, worker and gate it starts.

## What it holds

- `host.db`: the host's database (SQLite), read and written by every process of the fleet on the host, each change one transaction. It holds the host's own state, which no other host reads:
  - the checkout list and, beside it, the host settings (`afkconvoy run --help`)
  - the candidate chain of each repo and target branch (`afkconvoy land --help`)

None of it is pushed or sent anywhere: a host's disk is its only copy.

## From the cache directory

An older release kept these as files in the cache directory: `host.db` itself, and `checkouts.json` with `checkouts.lock`. The first process of this release that reads one takes it into `host.db`, in the transaction that records it did, and renames the file `<name>.migrated` beside where it was, kept: `host.db` is copied whole, by SQLite's backup. While a host runner of the older release still holds the host — until it moves onto this one between two ticks — the file stays where that runner reads and writes it, and what it writes there is taken in again; once it moved, the file is renamed. Renamed back, the older release reads it again. Taking in a file already taken in changes nothing.
