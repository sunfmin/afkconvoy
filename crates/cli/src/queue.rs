//! One local, read-only observation of the host's exclusive queue. The core
//! supplies occupancy and scheduling; neither a checkout nor a fleet rebuild
//! is involved.

use std::time::Duration;

use afkconvoy_core::exclusive::{Lanes, Snapshot, Ticket, snapshot};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::runner_socket::{self, Reached, Request};

/// The total time allowed for a runner's response, including partial lines.
pub const WITHIN: Duration = Duration::from_secs(10);

/// The queue and the runner's identity deliberately have no defaults: an
/// unreadable response must never become an empty queue. An older runner
/// that omits the pause leaves it unknown.
#[derive(Deserialize)]
struct Answer {
    exclusive: Vec<Ticket>,
    exclusive_paused: Option<bool>,
    host_runner: Identity,
}

#[derive(Deserialize)]
struct Identity {
    pid: u32,
    version: String,
    #[serde(flatten)]
    rest: serde_json::Map<String, Value>,
}

pub fn queue(fixed_now: Option<u64>) -> Result<Value> {
    observe(fixed_now).context(
        "queue observation failed; inspect the host-runner.log in the cache directory \
         (`afkconvoy help cache`) and retry `afkconvoy queue`; use `afkconvoy doctor --all` \
         for a full diagnosis; an unsupported response needs a host runner of this release",
    )
}

fn observe(fixed_now: Option<u64>) -> Result<Value> {
    let settings = crate::host_settings::of_host()?;
    let answer = match runner_socket::observe(&Request::Shared, WITHIN)? {
        Reached::Answer(answer) => {
            if let Some(error) = answer.get("error") {
                bail!("the host runner says: {error}");
            }
            Some(
                serde_json::from_value::<Answer>(answer)
                    .context("the host runner's queue response cannot be read")?,
            )
        }
        Reached::Nobody if !crate::host_runner::runs() => None,
        Reached::Nobody => bail!("a host runner holds the host but its socket is unavailable"),
        Reached::Closed => bail!("the host runner closed the connection before answering"),
        Reached::Unanswered => bail!("the host runner did not answer within {}s", WITHIN.as_secs()),
    };
    let now = fixed_now.unwrap_or_else(crate::clock);
    let (runner, said) = match answer {
        Some(answer) => {
            let mut runner = answer.host_runner.rest;
            runner.insert("pid".into(), json!(answer.host_runner.pid));
            runner.insert("version".into(), json!(answer.host_runner.version));
            (Value::Object(runner), Some((answer.exclusive, answer.exclusive_paused)))
        }
        None => (Value::Null, None),
    };
    let mut answer = observation(said, settings.lanes(), now);
    answer["host_runner"] = runner;
    Ok(answer)
}

/// What a host runner says of its exclusive queue: its tickets and whether
/// it paused new grants, the pause `None` where an older one does not say.
/// `None` whole where no host runner listens.
pub type Said = Option<(Vec<Ticket>, Option<bool>)>;

/// The one observation of the exclusive queue that `queue`, doctor and the
/// panel show, of what the host runner `said`.
pub fn snapshot_of(said: &Said, lanes: Lanes, now: u64) -> Snapshot {
    // An older runner that does not say its pause has none to apply.
    let (tickets, paused) = match said {
        Some((tickets, paused)) => (tickets.as_slice(), paused.unwrap_or(false)),
        None => (&[][..], false),
    };
    snapshot(tickets, lanes, paused, now)
}

/// `snapshot_of` as `queue` and doctor answer it. A pause the runner did not
/// say stays null.
pub(crate) fn observation(said: Said, lanes: Lanes, now: u64) -> Value {
    let (status, paused) = match &said {
        Some((_, paused)) => ("available", *paused),
        None => ("no_runner", Some(false)),
    };
    let mut answer = observation_json(&snapshot_of(&said, lanes, now));
    answer["paused"] = json!(paused);
    answer["observed_at"] = json!(now);
    answer["status"] = json!(status);
    answer
}

/// The common display of optional ticket metadata. Keep the private ticket
/// protocol unchanged for older runners.
fn observation_json(snapshot: &Snapshot) -> Value {
    let mut answer = json!(snapshot);
    // Old ticket metadata may omit these keys on the wire. An operator
    // observation says explicitly that the identity or lane is unknown.
    for group in ["running", "waiting"] {
        for request in answer[group].as_array_mut().into_iter().flatten() {
            if let Some(request) = request.as_object_mut() {
                request.entry("repo").or_insert(Value::Null);
                request.entry("repo_lane").or_insert(Value::Null);
                request.entry("is_turn").or_insert(json!(false));
                request.entry("cut").or_insert(json!(false));
                request.entry("candidate").or_insert(json!(false));
                request.entry("slow").or_insert(json!(false));
                request.entry("validation").or_insert(Value::Null);
                request.entry("gate").or_insert(Value::Null);
            }
        }
    }
    answer
}
