//! `afkconvoy checkouts`: which checkouts this host works and how each
//! stands (docs/adr/0023), for whoever looks after every repo of the host.
//! The checkout list is the one home of which checkouts there are: read from
//! it, from the host runner where one runs, and from each checkout's tick
//! log. Nothing is written, and neither GitHub nor the remote is asked.

use std::path::Path;

use anyhow::Result;
use serde_json::{Value, json};

use crate::checkout_list::{self, List};
use crate::git::Repo;
use crate::host_runner::{self, Holding};
use crate::log::{Log, What};

/// `afkconvoy checkouts` → {"host_runner", "checkouts"}: the host runner's
/// {pid, version, log}, null where none runs; and each checkout on this
/// host's checkout list, in its order, as `standing` says it. The host
/// runner is asked first: a checkout `stop` takes off the list meanwhile is
/// left out, not shown broken.
pub fn checkouts() -> Result<Value> {
    let holding = host_runner::holding()?;
    let listed = List::of_host()?.read()?.checkouts;
    let checkouts: Vec<Value> =
        listed.iter().map(|entry| standing(&entry.checkout, holding.as_ref())).collect();
    Ok(json!({
        "host_runner": holding.map(|holding| holding.about),
        "checkouts": checkouts,
    }))
}

/// How the checkout `root` stands, as `holding` — what the host runner
/// said it holds, `None` where none runs — and its tick log have it →
/// {checkout, repo, instance, state, idle, holding, frontier, ticked,
/// tick_log}.
///
/// Its state is `running` where the host runner holds its fleet instance,
/// `broken` where a host runner runs and holds none — it could not start
/// it, or its fleet instance ended — and `stopped` where none runs. Its
/// instance is the one that runs, null until its start is logged; else the
/// last one its tick log names. That instance's last tick says when it
/// ended, `ticked`, and whether it was idle, the claims it held and the
/// frontier it left as it did: null with no tick, and the three where it
/// failed whole. Its repo and its tick log are null where the checkout is
/// no git checkout any more.
fn standing(root: &Path, holding: Option<&Holding>) -> Value {
    let repo = Repo::at(root).ok();
    let key = repo.as_ref().and_then(|repo| checkout_list::repo_of(repo).ok());
    let tick_log = repo.as_ref().and_then(|repo| Log::of(repo).ok());
    let entries = tick_log.as_ref().and_then(Log::read).unwrap_or_default();
    let (state, running) = match holding.map(|holding| holding.instances.get(root)) {
        None => ("stopped", None),
        Some(Some(instance)) => ("running", Some(instance.clone())),
        Some(None) => ("broken", None),
    };
    let instance = running.unwrap_or_else(|| {
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
        "idle": last.and_then(|(_, tick)| tick.idle()),
        "holding": stood.map(|(held, _)| held),
        "frontier": stood.map(|(_, frontier)| frontier),
        "ticked": last.map(|(at, _)| at),
        "tick_log": tick_log.map(|log| log.file),
    })
}
