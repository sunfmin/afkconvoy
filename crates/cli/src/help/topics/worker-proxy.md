Which hosts a worker reaches past the proxy: a setting of one start on one host, said by `--worker-no-proxy`, which `run`, `tick`, `dispatch`, `restart` and `switch` take to start workers with, and `plan` and `doctor` to show what such a start would do. It is never a key of the repo's config: no proxy setting is read from the config or written to GitHub.

## The flag

`--worker-no-proxy [HARNESS=]HOSTS` says a comma-separated list of hosts workers reach directly. The bare form is for the default harness; repeat it for other harnesses, never twice for one.

```
--worker-no-proxy dsh=api.deepseek.com,internal.example
```

## What a worker is given

The worker's login shell extends its exported no_proxy and NO_PROXY with this list and exports the joined list under both names before it starts the worker. Every other proxy variable stays as it was. Without a list, both spellings stay untouched, except DSH's stock api.deepseek.com addition. Say `--worker-no-proxy dsh=` to turn that stock addition off. These settings apply to every worker, including patrols, consults and the train worker.

## What `plan` and `doctor` show

Each answers `worker_no_proxy` — `doctor` under `host` — listing every harness: {harness, addition, no_proxy, NO_PROXY}. `addition` is the host list added, null when none is: a null addition leaves both inherited values alone. The other fields show the login shell's resulting comma-separated values. They describe a start now, with the flags given to that command and the host user's login shell exports.
