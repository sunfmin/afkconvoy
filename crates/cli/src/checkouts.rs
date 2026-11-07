//! `afkconvoy checkouts`: which checkouts this host works and how each
//! stands (docs/adr/0023), for whoever looks after every repo of the host.
//! The checkout list is the one home of which checkouts there are: read from
//! it, from the host runner where one runs, and from each checkout's tick
//! log. Nothing is written, and neither GitHub nor the remote is asked.

use anyhow::Result;
use serde_json::{Value, json};

use crate::checkout_list::{self, Entry, List};
use crate::concurrency;
use crate::git::Repo;
use crate::host_runner::{self, Holding};
use crate::log::{Log, What};
use crate::run_log;

/// `afkconvoy checkouts` → {"host_runner", "checkouts"}: the host runner's
/// {pid, version, log}, null where none runs; and each checkout on this
/// host's checkout list, in its order, as `standing` says it. The host
/// runner is asked first: a checkout `stop` takes off the list meanwhile is
/// left out, not shown stopped.
pub fn checkouts() -> Result<Value> {
    let holding = host_runner::holding()?;
    let listed = List::of_host()?.read()?.checkouts;
    let checkouts: Vec<Value> =
        listed.iter().map(|entry| standing(entry, holding.as_ref())).collect();
    Ok(json!({
        "host_runner": holding.map(|holding| holding.about),
        "checkouts": checkouts,
    }))
}

/// How the checkout `listed` names stands, as `holding` — what the host
/// runner said it holds, `None` where none runs — and its tick log have it →
/// {checkout, repo, instance, state, broken, idle, holding, frontier,
/// concurrency, harness, ticked, tick_log}.
///
/// Its state is `broken` where its tick log says its fleet instance is, of
/// the host its `run` named (`run_log::broken`); else
/// `running` where the host runner holds its fleet instance; else
/// `stopped`. Its instance is the one that runs, null until its start is
/// logged; else the last one its tick log names. That instance's last tick
/// says when it ended, `ticked`, and whether it was idle, the claims it held
/// and the frontier it left as it did: null with no tick, and the three
/// where it failed whole. Its concurrency is the one its fleet instance goes
/// by (`concurrency::json_of`), read from its config and its entry; its
/// tracker the one its entry says, null where it says none. Its repo
/// and its tick log are null where the checkout is no git checkout any more,
/// and its concurrency {error}.
fn standing(listed: &Entry, holding: Option<&Holding>) -> Value {
    let root = &listed.checkout;
    let repo = Repo::at(root).ok();
    let key = repo.as_ref().and_then(|repo| checkout_list::repo_of(repo).ok());
    let tick_log = repo.as_ref().and_then(|repo| Log::of(repo).ok());
    let entries = tick_log.as_ref().and_then(Log::read).unwrap_or_default();
    let host = listed.named.host.clone().unwrap_or_else(crate::this_host);
    let broken = run_log::broken(&entries, &host, holding.is_some());
    // What the host runner holds of it: its instance, `None` until logged.
    let held = holding.and_then(|holding| holding.instances.get(root));
    let state = match (&broken, held) {
        (Some(_), _) => "broken",
        (None, Some(_)) => "running",
        (None, None) => "stopped",
    };
    let instance = held.cloned().flatten().or_else(|| {
        let of_run = entries.iter().rev().find(|entry| entry.of_run());
        of_run.map(|entry| entry.instance.clone())
    });
    let last = instance.as_ref().and_then(|instance| {
        entries.iter().rev().find_map(|entry| match &entry.what {
            What::Tick(tick) if &entry.instance == instance => Some((entry.at, tick)),
            _ => None,
        })
    });
    let stood = last.and_then(|(_, tick)| tick.stood());
    json!({
        "checkout": root,
        "repo": key,
        "instance": instance,
        "state": state,
        "broken": broken,
        "idle": last.and_then(|(_, tick)| tick.idle()),
        "holding": stood.map(|(held, _)| held),
        "frontier": stood.map(|(_, frontier)| frontier),
        "concurrency": match &repo {
            Some(repo) => concurrency::json_of(repo),
            None => json!({"error": format!("{} is no git checkout", root.display())}),
        },
        "harness": match &repo {
            Some(repo) => crate::named_harness::json_of(repo),
            None => json!({"error": format!("{} is no git checkout", root.display())}),
        },
        "tracker": listed.tracker,
        "ticked": last.map(|(at, _)| at),
        "tick_log": tick_log.map(|log| log.file),
    })
}

/// What `afkconvoy checkouts --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

To see every repo of this host at once: which checkouts its host runner works, whether each one's fleet instance runs, and whether it has work. A session that looks after every repo of the host asks this — and `afkconvoy doctor --all` for what is wrong with each — instead of keeping a list of its own: the checkout list is the one home of which checkouts there are.

## Who runs it

The operator, or a session of theirs that looks after every repo of the host: anywhere on this host, since it is about no one checkout.

## What it reads and writes

The checkout list: checkouts.json in the cache directory (`afkconvoy help cache` says where it is). The host runner, asked on its socket which checkouts it holds a fleet instance of: where none runs, the list alone answers. Each checkout's tick log, for whether its fleet instance is broken and for its last tick, and its entry, for its concurrency, its tracker and its harness. It writes nothing, asks neither GitHub nor the remote, and makes no startup probe: it is safe beside a live fleet, as often as you like.

## What it answers

```json
{"host_runner": {"pid": 4242, "version": "0.6.0", "log": "<cache>/host-runner.log",
                 "panel": "http://127.0.0.1:7070/"},
 "checkouts": [{"checkout": "/Users/me/Developments/hkj", "repo": "github.com/me/hkj",
  "instance": "me-h1-1a2b3c4d", "state": "running", "broken": null,
  "ticked": 1767225600, "idle": false, "holding": 2, "frontier": 1,
  "concurrency": {"in_force": 6, "said_by": "checkout"},
  "harness": {"in_force": "kimi", "said_by": "checkout"}, "tracker": "me/issues",
  "tick_log": "<cache>/runs/hkj-0123456789abcdef.jsonl"}]}
```

- `host_runner`: the one running — its pid, its version, its log and where it serves the panel — or null where none runs. `<cache>` stands for the cache directory, whole in the answer.
- `checkouts`: each checkout on the list, in its order; [] with none:

  - `checkout`: its root. `repo`: the repo its remote `origin` names, as `run` answers it; null where it is no git checkout any more
  - `state`: `broken` — its fleet instance is broken (`afkconvoy run --help`), as its tick log says, whether or not a host runner runs to start it again; else `running` — the host runner holds its fleet instance; else `stopped` — no host runner runs, or it holds none of it. `afkconvoy run` in the checkout starts a broken or stopped one at once
  - `broken`: as `afkconvoy doctor` answers it under `fleet`: {why, since, next, instance} — why, as it last broke; since when, and when the host runner starts it again, in epoch seconds, null where no host runner runs; and the fleet instance that broke, "" where none had started. null where it is not broken
  - `instance`: its fleet instance id: the one running, null until its start is logged; else the last one its tick log names, null where it names none
  - `ticked`: when that fleet instance's last tick ended, in epoch seconds; null with none
  - `idle`, `holding`, `frontier`: as that tick ended: whether the fleet instance was idle — it held nothing, nothing of its was on the base branch, and its frontier was empty — the claims it held, and the frontier issues it left undispatched (`afkconvoy tick --help`). null with no tick, or one that failed whole; `idle` null too of a tick logged by an older release
  - `concurrency`: {in_force, said_by}: how many claims its fleet instance holds at work at once on this host, and who says it, as `afkconvoy plan` answers it: `checkout` (`afkconvoy run --concurrency`) or `default`. {error} where its config cannot be read, or it is no git checkout any more
  - `harness`: {in_force, said_by}: the harness its workers start on, and who says it, as `afkconvoy plan` answers it: `checkout` (`afkconvoy run --harness`) or `host_kit`. {error} where the host kit cannot be read, or it is no git checkout any more
  - `tracker`: the repo its fleet reads and writes its issues in, `<owner>/<repo>`, as its entry says it (`afkconvoy run --tracker`); null where it says none, and the checkout's own repo is its tracker
  - `tick_log`: the file its fleet instance logs its ticks in, as `run` answers it; null where it is no git checkout any more

It exits 0 whatever the checkouts' states. Errors, as {"error": …} with a non-zero exit: the checkout list cannot be read; the host runner cannot say which checkouts it holds — it is of an older afkconvoy, which `afkconvoy run` in any checkout moves onto this one; it ended before it answered; or it answered nothing within 10 seconds.

## What trips you up

- `idle`, `holding` and `frontier` are of the last tick, not of now: those of a stopped or broken checkout are as its fleet instance last ticked, which `ticked` dates. `afkconvoy doctor`, in the checkout, reads GitHub as it is now.
- A checkout `run` is adding is on no list until its first tick ends, and is not answered until then; one `stop` took off is not answered.
- A broken checkout the host runner starts again is `running` once its tick log keeps that start: it is back once the start's first tick ends, and broken again if it ends before. `afkconvoy doctor` says the same of it.
- `host_runner.panel` is where the host runner serves the panel: the address `afkconvoy panel --open` answers, with no need to ask it. Where no host runner runs it is null, as `host_runner` is whole; where the host runner serves none, it alone is null, and `panel --open` says why. `afkconvoy panel --help` says what the page shows."#;
