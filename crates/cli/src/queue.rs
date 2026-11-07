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

/// What `afkconvoy queue --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

Run `afkconvoy queue` from any directory on the host, as the host runner's user, to see who occupies its worker lanes and why each request waits. No checkout, login or running fleet is required. It accepts no checkout or host selector: it observes this user's local host runner across all its repos.

## Who runs it

The operator, or their agent, on the host whose exclusive queue they want to read.

## What it reads and writes

It reads the host settings once and asks the host runner once on its local socket, allowing {const:QUEUE_SECONDS}s for its response, including partial lines. Settings absent use the defaults; unreadable settings fail. It reads no GitHub, git, harness, tick log or recorded gate outcomes, makes no probe or full diagnosis, sends no wake and changes no fleet, queue or settings. A stale socket is left in place. A temporary link used to reach a long socket path is removed afterwards. The runner's lock is checked only if its socket is absent or refuses the connection.

## What it answers

Exactly one JSON object, exit 0:

- `observed_at`: epoch seconds captured after the local response. Every duration uses this one time; clock differences clamp negative durations to zero. Settings and the queue are a bounded observation, not an atomic snapshot of both files and process.
- `status`: `available` when the runner answered, including a healthy empty queue; `no_runner` when no runner listens or holds the host. No runner is a successful observation with empty arrays and `host_runner: null`, not evidence of commands running outside the queue.
- `host_runner`: the answering runner's `pid` and `version`, plus the identity fields it supplies: `log` (whole path), `panel` (address or null), and `foreground` where applicable. Null with no runner.
- `paused`: whether the runner has paused new grants for a release move; false with no runner; null where an older runner does not say it, which pauses nothing.
- `worker`: `{capacity, used, free}`. Capacity is the host setting `gate_lanes`, {const:GATE_LANES} by default; used counts running worker requests, free is capacity minus used, clamped to zero. Eligible reservations do not count as used.
- `slow`: `{capacity, used, free}` of the slow lanes, where a round's slow gate runs: the host setting `slow_gate_lanes`, 1 by default (`afkconvoy run --help`).
- `fast`: `{capacity, used, free}` of the fast lane, which only a worker's fast gate takes (`afkconvoy gate --fast`). Capacity is the host setting `fast_lanes`, {const:FAST_LANES} by default; used counts running fast gates. It is independent of worker capacity: no slow gate, turn or other command holds a place in it.
- `repos`: each repo lane with a running or waiting request, sorted by its whole identity, as `{repo, capacity, used, free}`. Each has capacity 1, independent of worker capacity. `repo` is the shared git directory supplied by the ticket, never inferred from a worktree's name.
- `running`, `waiting`: requests in scheduler order within each array: cuts first, then ordinary commands; within each group ascending `asked`, then `number` to break ties. Cut priority never interrupts running commands. There is no priority by lane or repo. A request identifies `number` (runner-local ticket), `pid` (asking process), `command` (one line: the program and the arguments that say what is running; `afkconvoy exclusive -- ./gate.sh` stays as given; a script is named by the program it runs, its `cargo nextest` line or its `afkconvoy exclusive` invocation, and is not shown as the script; no `NAME=value`), `dir` (whole worktree or outside directory), `issue` (number or null; no lookup), `repo` (shared git directory or null for older/outside tickets), `repo_lane` (repo lane identity or null for worker), `is_turn` (whether this is a landing turn), `cut` (whether it has cut priority, false for older tickets), `asked` (request epoch seconds), `ran` (start epoch seconds or null), `gate` (`fast` or `slow` for a gate a worker runs alone, `gate --fast` or `--slow`; null otherwise), `kind` (`fast_gate`, `slow_gate`, `turn` or `command`: a landing turn is a `turn`, whichever gate it runs) and `lane` (`worker`, `fast` or `repo <identity>`).
- Each request adds `elapsed_seconds` (time since `ran`, null while waiting), `waiting_seconds` (time from `asked` to `ran` when running, otherwise to `observed_at`), and `scheduling`: `{number, state, reasons}`. State is `running`, `eligible`, or `waiting`; reasons is empty for running or eligible requests.
- `candidate` is true for independent candidate validation. `validation`, null for ordinary or unreported requests, is the turn's display copy of its remote candidate: {id, predecessor, commit, phase, waiting, cancelled_by, cause}. Phase follows building, validating, green, slow, landed, red, cancelling or cancelled. Waiting distinguishes constructing_worktree, validating, waiting_for_round, slow_gating, waiting_for_predecessor and confirming_process_exit; a waiting queue request without a candidate awaits capacity. Cancellation names its invalidating predecessor and cause. These copies explain current work and authorize no publication; reading queue makes no remote lookup.
- Each reason has `kind`, `requests`, `capacity` and `explanation`. Kind `same_repo` means a running or earlier eligible request of this repo excludes this request in its lane, or across lanes when both are turns. `worker_capacity`, `fast_capacity` or `repo_capacity` means all places in that lane are occupied or reserved, with the limit in `capacity`. `paused` means new grants are paused. Other reasons have `capacity: null`. `requests` names each relevant `{number, state}`: `running` holds a place now, `eligible` reserves one earlier in FIFO order; it is empty for a pause. `explanation` states the constraint in words.

Earlier eligible requests reserve capacity in scheduler order, with cuts before ordinary waiters and FIFO within each group. Blocked requests reserve nothing: with three worker lanes, ordinary requests of a running repo can leave two free while waiting, and a different repo can be eligible. Independently owned candidate worktrees of the same repo may validate together, and so may fast gates of the same repo, never with `same_repo` as a reason; ordinary heavy commands and serial barriers retain same-repo exclusion across worker and repo lanes. Older tickets lacking `repo` use their whole `dir` as the scheduler's fallback identity. `eligible` means allowed by this observation, not guaranteed to start next. Ordering is no fixed future start order or ETA; requests can leave and settings can change.

Errors: unreadable host settings, an inaccessible socket, a runner holding the host but not listening, a closed connection, no response within the bound, a runner error, or an unsupported or unreadable response (including missing queue or runner identity fields). These print `{"error": ...}` with exit 1 and a next action: inspect the host runner log and run the command again; `afkconvoy doctor --all` provides a full diagnosis. An unsupported response needs a host runner of this release. A failed observation never becomes an empty queue.

## What trips you up

`afkconvoy doctor` shows this same observation as `host.exclusive`, and the panel's command lanes are its lanes: the worker lane, the fast lane and each repo lane, with the same running and waiting requests, each named by its kind. It reports current queued commands only. For historical gate outcomes, use `afkconvoy doctor --all --since <time>`; `afkconvoy doctor --help` explains its interval boundaries. Commands that bypass the exclusive queue are absent. Request numbers belong to this runner and may change after it is replaced."#;
