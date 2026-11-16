//! One local, read-only observation of the host's exclusive queue. The core
//! supplies occupancy and scheduling; neither a checkout nor a fleet rebuild
//! is involved.

use std::time::Duration;

use afkconvoy_core::exclusive::{Halt, Low, Room, Snapshot, Ticket, snapshot};
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
    /// What it last saw of the disk; `None` from an older runner, which
    /// holds nothing for it.
    #[serde(default)]
    disk: Option<crate::disk::Seen>,
    /// What it starts commands by (docs/adr/0087); `None` from an older
    /// runner, which counted places.
    #[serde(default)]
    exclusive_room: Option<Room>,
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
            (
                Value::Object(runner),
                Some((
                    answer.exclusive,
                    answer.exclusive_paused,
                    answer.disk.and_then(|seen| seen.low()),
                    answer.exclusive_room,
                )),
            )
        }
        None => (Value::Null, None),
    };
    let mut answer = observation(said, settings.busy_share(), now);
    answer["host_runner"] = runner;
    Ok(answer)
}

/// What a host runner says of its exclusive queue: its tickets and whether
/// it paused new grants, the pause `None` where an older one does not say;
/// the disk under its floor, where it grants nothing new for it (`disk`);
/// and what it starts commands by, `None` where an older one does not say.
/// `None` whole where no host runner listens.
pub type Said = Option<(Vec<Ticket>, Option<bool>, Option<Low>, Option<Room>)>;

/// What `shared`, a host runner's answer, says of its exclusive queue.
pub fn said_of(shared: &crate::host_runner::SharedNow) -> Said {
    Some((
        shared.exclusive.clone(),
        shared.exclusive_paused,
        shared.disk.as_ref().and_then(|seen| seen.low()),
        shared.exclusive_room,
    ))
}

/// The one observation of the exclusive queue that `queue`, doctor and the
/// panel show, of what the host runner `said`, the busy share `share` where
/// it says no room of its own: no reading.
pub fn snapshot_of(said: &Said, share: u64, now: u64) -> Snapshot {
    // An older runner that does not say its pause has none to apply.
    let (tickets, halt, room) = match said {
        Some((tickets, paused, disk, room)) => (
            tickets.as_slice(),
            Halt { paused: paused.unwrap_or(false), disk: *disk },
            room.unwrap_or(Room::unread(share)),
        ),
        None => (&[][..], Halt::default(), Room::unread(share)),
    };
    snapshot(tickets, room, halt, now)
}

/// `snapshot_of` as `queue` and doctor answer it. A pause the runner did not
/// say stays null.
pub(crate) fn observation(said: Said, share: u64, now: u64) -> Value {
    let (status, paused) = match &said {
        Some((_, paused, _, _)) => ("available", *paused),
        None => ("no_runner", Some(false)),
    };
    let mut answer = observation_json(&snapshot_of(&said, share, now));
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
                request.entry("retained").or_insert(Value::Null);
            }
        }
    }
    answer
}

/// How long a lane may stay retained for an unconfirmed cleanup before
/// doctor says so: the runner tries its cleanup again each minute, so one
/// held this long keeps failing.
pub const RETAINED_SECONDS: u64 = 600;

/// Doctor's finding of each request of `observation` — `observation`'s
/// answer — whose lane has stayed retained `RETAINED_SECONDS` or longer at
/// `now`, naming its ticket and its cleanup's last error.
pub(crate) fn retained_findings(observation: &Value, now: u64) -> Vec<crate::doctor::Finding> {
    use crate::doctor::Kind;
    let mut found = vec![];
    for request in observation["running"].as_array().into_iter().flatten() {
        let Some(since) = request["retained"]["since"].as_u64() else { continue };
        let minutes = now.saturating_sub(since) / 60;
        if now.saturating_sub(since) < RETAINED_SECONDS {
            continue;
        }
        let error = request["retained"]["error"].as_str().unwrap_or("");
        found.push(crate::doctor::finding(
            Kind::LaneRetained,
            json!(["lane-retained", request["number"], request["pid"]]),
            format!(
                "request {} ({}, pid {}) has had {} for {minutes} min, keeping the {} lane \
                 occupied",
                request["number"],
                request["command"].as_str().unwrap_or(""),
                request["pid"],
                Kind::LaneRetained.symptom(),
                request["lane"].as_str().unwrap_or("worker"),
            ),
            format!("its cleanup's last error: {error}"),
            "afkconvoy queue".into(),
        ));
    }
    found
}

/// What `afkconvoy queue --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

Run `afkconvoy queue` from any directory on the host, as the host runner's user, to see what runs in its exclusive queue, how busy the host reads, and why each request waits. No checkout, login or running fleet is required. It accepts no checkout or host selector: it observes this user's local host runner across all its repos.

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
- `disk`: `{free, floor}`, in bytes, while the disk the host builds on is under its floor and the runner grants no new request for it; null otherwise, with no runner, or from an older runner. The floor is the measured one admission holds to (`afkconvoy run --help`, DISK); running requests keep their lanes.
- `room`: what a queued command starts by, in place of a count of places (docs/adr/0087): `{reading, share, counted, spent}`. `reading` is the runner's reading of the host, `{busy, by, since, taken, memory}`: `busy` the share of its cores' time busy in percent, `by` `ticks` (the kernel's CPU ticks) or `load` (the load average over its cores, where the ticks cannot be read), from `since` to `taken` in epoch seconds — the last minute, or less for a runner that has read for less — and `memory`, where the host's memory free is under admission's floor, `{free, floor}` in percent; `reading` is null where the host cannot be read, where no runner listens, or for an older runner. `share` is the busy share a command starts under, the host setting `busy_share` ({const:BUSY_SHARE}% by default, `afkconvoy run --help`). `counted` is the percent of the host the queue's commands started since the reading began add, each as its kind of command took in its recent runs, until the reading includes it. `spent` is true where a command already started on this reading: the next starts on the next reading.
- `repos`: each repo lane with a running or waiting request, sorted by its whole identity, as `{repo, capacity, used, free}`. Each has capacity 1: a light gate keeps its repo's own line, whatever the room. `repo` is the shared git directory supplied by the ticket, never inferred from a worktree's name.
- `running`, `waiting`: requests in scheduler order within each array: cuts first, then ordinary commands; within each group ascending `asked`, then `number` to break ties. Cut priority never interrupts running commands. There is no priority by lane or repo. A request identifies `number` (runner-local ticket), `pid` (asking process), `command` (one line: the program and the arguments that say what is running; `afkconvoy exclusive -- ./gate.sh` stays as given; a script is named by the program it runs, its `cargo nextest` line or its `afkconvoy exclusive` invocation, and is not shown as the script; no `NAME=value`), `dir` (whole worktree or outside directory), `issue` (number or null; no lookup), `repo` (shared git directory or null for older/outside tickets), `repo_lane` (repo lane identity or null for worker), `is_turn` (whether this is a landing turn), `cut` (whether it has cut priority, false for older tickets), `asked` (request epoch seconds), `ran` (start epoch seconds or null), `gate` (`fast` or `slow` for a gate a worker runs alone, `gate --fast` or `--slow`; null otherwise), `kind` (`fast_gate`, `slow_gate`, `turn` or `command`: a landing turn is a `turn`, whichever gate it runs) and `lane` (`worker`, `fast` or `repo <identity>`).
- Each request adds `elapsed_seconds` (time since `ran`, null while waiting), `waiting_seconds` (time from `asked` to `ran` when running, otherwise to `observed_at`), and `scheduling`: `{number, state, reasons}`. State is `running`, `retained`, `eligible`, or `waiting`; reasons is empty for running, retained or eligible requests.
- `retained`, null otherwise, is `{since, error}` of a request in `running` whose command ended and whose asker left, but whose cleanup the runner could not confirm (a candidate's, say, on a full disk): its state is `retained`, not `running`, its pid is gone, and it keeps its repo's exclusion, though the host's room does not count it running, until the runner confirms the cleanup. `since` is when its cleanup first failed, `error` what the last try said. The runner tries again each minute, and frees the place once it passes; `afkconvoy doctor` warns of one retained {const:RETAINED_SECONDS}s or longer.
- `candidate` is true for independent candidate validation. `validation`, null for ordinary or unreported requests, is the turn's display copy of its remote candidate: {id, predecessor, commit, phase, waiting, cancelled_by, cause}. Phase follows building, validating, green, slow, landed, red, cancelling or cancelled. Waiting distinguishes constructing_worktree, validating, waiting_for_round, slow_gating, waiting_for_predecessor and confirming_process_exit; a waiting queue request without a candidate awaits room. Cancellation names its invalidating predecessor and cause. These copies explain current work and authorize no publication; reading queue makes no remote lookup.
- Each reason has `kind`, `requests`, `capacity`, `busy`, `share` and `explanation`. Kind `same_repo` means a running or earlier eligible request of this repo excludes this request in its lane, or across lanes when both are turns. `host_busy` means the host's cores count `busy` percent — the reading and `counted` — at or past `share`, or that they cannot be read while a command of the queue runs; `next_reading` means a command already started on this reading, and this one waits for the next, which counts that one; `host_memory` means the host's memory free is under admission's floor. While nothing of the queue runs, the first request that may start starts whatever the reading says. `repo_capacity` means a light gate of the repo already holds or reserves its line, with `capacity` 1. `paused` means new grants are paused. `disk` means the disk the host builds on is under its floor: no request is granted until space returns, whatever the room, and `explanation` says the bytes free and the floor. `busy` and `share` are said for the host's room alone, `capacity` for a repo's line alone; they are left out or null otherwise. `requests` names each relevant `{number, state}`: `running` holds what it needs now, `eligible` reserves it earlier in FIFO order; it is empty for a pause or the disk. `explanation` states the constraint in words.

Earlier eligible requests reserve their start in scheduler order, with cuts before ordinary waiters and FIFO within each group. Blocked requests reserve nothing: ordinary requests of a running repo can wait while a different repo is eligible. Independently owned candidate worktrees of the same repo may validate together, and so may fast gates of the same repo, never with `same_repo` as a reason; ordinary heavy commands and serial barriers retain same-repo exclusion across lanes. Older tickets lacking `repo` use their whole `dir` as the scheduler's fallback identity. `eligible` means allowed by this observation, not guaranteed to start next — a reservation, with no fixed future start order or ETA; requests can leave, readings and settings can change.

Errors: unreadable host settings, an inaccessible socket, a runner holding the host but not listening, a closed connection, no response within the bound, a runner error, or an unsupported or unreadable response (including missing queue or runner identity fields). These print `{"error": ...}` with exit 1 and a next action: inspect the host runner log and run the command again; `afkconvoy doctor --all` provides a full diagnosis. An unsupported response needs a host runner of this release. A failed observation never becomes an empty queue.

## What trips you up

`afkconvoy doctor` shows this same observation as `host.exclusive`, and the panel's command lanes are its lanes: the worker lane, the fast lane and each repo lane, with the same running and waiting requests, each named by its kind, under the host's reading and the busy share. It reports current queued commands only. For historical gate outcomes, use `afkconvoy doctor --all --since <time>`; `afkconvoy doctor --help` explains its interval boundaries. Commands that bypass the exclusive queue are absent. Request numbers belong to this runner and may change after it is replaced."#;

#[cfg(test)]
mod tests {
    use super::*;

    /// Doctor names a lane retained past the bound, with its cleanup's
    /// error, and is silent of one retained a shorter time.
    #[test]
    fn doctor_warns_of_a_lane_retained_past_its_bound_with_its_error() {
        let request = |number: u64, since: u64| {
            json!({"number": number, "pid": 40284, "command": "afkconvoy land", "lane": "worker",
                   "retained": {"since": since, "error": "git commit-tree: No space left on device"}})
        };
        let observation = json!({"running": [
            request(61, 1_000),
            request(62, 1_000 + RETAINED_SECONDS - 1),
            {"number": 63, "pid": 1, "command": "x", "lane": "worker", "retained": null},
        ]});
        let found = retained_findings(&observation, 1_000 + RETAINED_SECONDS);
        assert_eq!(found.len(), 1, "{:?}", found.iter().map(|f| &f.what).collect::<Vec<_>>());
        let finding = &found[0];
        assert_eq!(finding.kind, crate::doctor::Kind::LaneRetained);
        assert_eq!(finding.severity, crate::doctor::Severity::Warning);
        assert!(
            finding.what.starts_with("request 61 (afkconvoy land, pid 40284)"),
            "{}",
            finding.what
        );
        assert!(finding.evidence.contains("No space left on device"), "{}", finding.evidence);
    }
}
