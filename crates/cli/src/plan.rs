//! `afkconvoy plan`: what a tick would act on, read in one look at the
//! fleet, its host and its team, with the startup probe of this host.

use afkconvoy_core::claims::InstanceId;
use serde_json::{Value, json};

use crate::fleet::Fleet;
use crate::git::Repo;
use crate::launch::Launches;
use crate::{admission, blockers, codex_launch, doctor, fleet_report, kit, probe};

/// The plan of `repo` at `now`, with `instance`'s claims its own and the
/// workers' `no_proxy` hosts as the command line names them.
pub(crate) fn plan(
    repo: &Repo,
    now: u64,
    instance: Option<&InstanceId>,
    no_proxy: &[String],
) -> anyhow::Result<Value> {
    let launches = Launches::foreseen(repo).with_no_proxy(no_proxy)?;
    let warnings = match probe::standing(repo, now, &launches) {
        Some(warnings) => warnings,
        None => probe::startup(repo, now, &launches)?,
    };
    probe::announce(&warnings);
    // `plan` is one look: its host reading is the report's.
    let readings = admission::Readings::new();
    let (fleet, open) = Fleet::read(repo, now)?;
    let (mut set, flows, _) = fleet.rebuild(&open, instance)?;
    let report = fleet_report::FleetReport::read(&fleet, &flows, &set, &readings);
    if let Ok(admission) = &report.admission {
        set.held_back = admission.holds();
    }
    let findings = blockers::findings(&set.unworked_blockers);
    let mut plan = serde_json::to_value(set)?;
    plan["findings"] = doctor::answered(doctor::checkout_at(&repo.dir), findings)?;
    if let Some(plan) = plan.as_object_mut() {
        plan.extend(report.plan_json()?);
    }
    plan["need_installs"] = json!(
        probe::installs(&fleet.repo, &fleet.config)
            .into_iter()
            .map(|(kind, installed)| json!({
                "kind": kind,
                "check": installed.check.name,
                "at": installed.at,
                "seconds": installed.seconds,
                "status": installed.status,
                "passed": installed.passed,
                "stderr": installed.stderr,
            }))
            .collect::<Vec<_>>()
    );
    plan["targets"] = fleet.targets(&open.issues)?;
    plan["host_kit"] = kit::show(now);
    plan["selection"] = fleet.selection();
    plan["codex_workers"] = json!(codex_launch::workers(&fleet.repo));
    report.put_launches(&mut plan["frontier"]);
    if let Ok(team) = flows.team() {
        plan["selection"]["roles"] =
            json!(team.roles.iter().map(|role| fleet.role_selection(role)).collect::<Vec<_>>());
    }
    plan["worker_no_proxy"] = launches.proxy_json();
    if let Some(pressure) = crate::resources::from_elsewhere(now, &readings) {
        plan["memory_pressure"] = pressure;
    }
    Ok(plan)
}

/// What `afkconvoy plan --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

Before `run`, to see what the fleet would start on; and beside a running fleet, to see where every claim stands and why an issue is not on the frontier.

## Who runs it

The operator, in a checkout of the repo.

## What it reads and writes

It makes the startup probe — which checks, as a start does, the worker launch command a `run` started now would give every harness the fleet could be asked for (`afkconvoy run --help`) — then the rebuild: the open issues and PRs on GitHub, the claims and heartbeats on the remote, and their landing turns. It changes nothing on GitHub. On the remote it pushes its startup probe and deletes it again, together with what the probe sweeps: a probe an earlier start left behind, and recorded gate runs too old for a landing to believe.

WHEN IT PROBES. Not every time: a startup probe that passed on this host in the last hour stands, and `plan` then makes none — it says so on stderr, repeats that probe's warnings, and pushes nothing. It probes when no probe stands: none passed on this host within the hour — `run` makes one at every start, `tick` and `plan` when they find none — or the config's `base_branch` or `team` changed since, or the remote's URL did, or the note of where the remote keeps the fleet's refs — or no check that found the tracker the checkout says now private stands: one stands for the hour, of that tracker alone. The probe is noted in the repo's git dir (`convoy-probe`), and the check of the tracker beside it (`convoy-tracker`); delete either file to have the next `plan` probe.

The rebuild asks GitHub twice, side by side with one listing of the remote, however many issues are open: one query for the open issues with their comments (comments beyond the first hundred or blockers beyond the first ten are read in full through their paginated endpoints), one for the open PRs. In a tracker another repo than the checkout's, GitHub answers only the open issues that carry this repo's repo label, so it asks as often however many repos' issues the tracker holds. It asks once more — for the issues that carry the label `slow-gate` — only while the recorded gate runs of the base branch's landings say the gate is slow.

{const:REMOTE_READS}

## What it answers

{"frontier", "excluded", "waiting_on_human", "mine", "peer_live", "stale", "stale_closed", "free_slots", "now", "locks", "concurrency", "harness", "tracker", "admission", "host_kit", "worker_no_proxy", "need_installs"} — "held_back" while admission holds; and in a bound repo "intake", "left_out", "unmatched" and "nothing_safe" where they name some, "patrols" where it names some; and "slow_gate" while the gate is slow; and "cut" in a repo that turns cutting on, or while a cut is going or on the base branch; and "memory_pressure" while the host's memory pressure stands and another account holds the most memory

`targets` lists open tracker tasks with issue, tracker_issue, delivery (null when unattached), target_branch, configured_base and pinned. Invalid destinations report error per task without falling back to the configured base.

`deliveries` lists each Jira delivery with its final PR or pending reason, whether recorded delivery evidence certifies the current delivery head, final-target head and what each attached task came to, and any settlement. Reading writes nothing.

- `host_kit`: {path, refreshed, harnesses}: this host's kit file; when it was last written, in epoch seconds — by `kit`, by `run`, or by a tick's daily refresh (`afkconvoy kit --help`); each harness in order of preference as {harness, command, models, default_model, research_pending}, each model {model, names, good_at, settled, sources, researched_at}. `default_model` is true when the harness has no placed model, so a worker runs its login's default with no model flag; `research_pending` is true while it holds a model awaiting research. good_at is what research says a model is good at, or a research marker; settled is a reason research found it unsuitable. The research states are defined in `afkconvoy kit --help`. sources holds dated URLs and researched_at the research time, absent where none succeeded. With an unreadable kit, {path, error}. `afkconvoy kit` writes it again.
- `frontier`: issues due to dispatch, with their stage and cast {harness, model, why}. A checkout that names none takes the host kit's first eligible harness within role eligibility. One that names a harness starts on that harness, and the host kit orders that harness's models. Model labels only match within the harness in force. `selection` lists kit order, eligible candidates, skipped candidates and health evidence.
- `need_installs`: each install this host last ran for a gate or cut need the config declares, as {kind, check, at, seconds, status, passed, stderr}: `gate-needs` or `cut-needs`, the check's name, when it ran in epoch seconds, how long it took, the install's status, whether the check then passed, and the user-only file of its last 20 stderr lines — never its output (`afkconvoy config --help`). Empty where none ran.
- `codex_workers`: started Codex sessions with their actual model, effort and sources, as `doctor` answers them. A frontier issue whose harness is Codex also has `codex`: its next-launch model and source from a followed model label (`model_label`), the current user preference (`user_selection`), or Codex's native default (`codex_default`), plus the effort read now from your own `~/.codex/config.toml` (`codex_default` with no setting). Existing sessions keep their launch evidence or their own saved `/model` choice. [`afkconvoy help worker-launch`](help:worker-launch).
- `excluded`: every other open issue, with why: {number, title, reason}. An escalated issue says so — "escalated: waits for a ruling" — and not that it lacks the label its escalation took off. A name that matches nothing never excludes an issue: it runs the user default, and the label is said once. A bound repo's issue at a stage whose role has no harness on this host it holds limits for waits, "<role> has no harness on this host it holds limits for: ", naming what the role holds limits for and what the host kit holds, and is dispatched once the host kit holds one. No issue is held for a caster.
- `left_out`: in a bound repo whose team and host kit can be read: each role, in the team's order, with what its range on this host leaves out: {role, left_out: [{harness, why}]} — a harness its roster entry and the host kit allow it and it holds no limits for, while it holds limits for another, why `no limits for pi` (`afkconvoy help roster`)
- `unmatched`: in a bound repo, where some role's roster entry matches nothing in the host kit: {role, why} — the role ranges over the host kit whole, less what it holds no limits for
- `nothing_safe`: in a bound repo, where some role has no harness on this host it holds limits for: {role, why} — its stages wait, and every other stage goes on
- `unmatched_models`: each roster model matching nothing held, {role, harness, model, holds, said}, with the nearest names or the two matches
- `casting`: each open issue, {number, needed, why, stages, running}. why is "its model label" where every stage followed one, "the user's default" where none did, or both where the stages differ. needed is always false. stages lists {stage, harness, model, followed, why, skipped}; followed says whether the one `model:` label was followed, model is that id or null for the user default, and skipped is empty. running is {harness, model} recorded at the existing worker's launch — model null for the user default — or null. Doctor answers the same under issues.casting, and the panel shows it under Model selection. There is no caster.
- `waiting_on_human`: every open issue nobody holds that waits for a human, lowest number first: {issue, title, why, url, since, answer} — `why`: what kind of wait, then the reason the issue’s comment gives, in its first sentence; `url`: that comment (the report itself for an older failed patrol or cut with no reason comment), or null where no comment explains the wait; since when in epoch seconds where GitHub said, and the line that answers it, run as printed apart from what stands between < and >, naming the checkout as that command takes it so the line runs from any directory. An escalation with no position gets the team's only flow label filled in; with several workflows, `why` lists the flow labels to choose `<flow label>` from. They are: an escalated issue, one whose records call for a human, one at a stage that is a human's, one whose workflow ended and that is still open, and the report of a patrol's failed run
- `escalating`: in a bound repo, when there are any: the unclaimed issues a tick hands to a human, with why: {number, title, reason}
- `mine`: the claims of `--instance`: {number, title, status, pr, declared, idle} — `declared` is true while a verdict declared under the claim waits to be acted on: the claim takes no slot meanwhile. `idle` is true of a landing claim whose worker is not changing the tree — gone or idle, or its turn queued, waiting on a predecessor or last answering a wait: it takes no slot either, and is held until its PR lands. In a bound repo with `due`, the stage a worker is to be started on once the claim's stage has declared, where a role performs one. status is one of those below
- `peer_live`: other instances' claims within the lease — never touched
- `stale`: other instances' claims past the lease, on open issues — a tick takes them
- `stale_closed`: the same on closed issues — a tick deletes them (each of the three: {number, instance, sha})
- `free_slots`: how many more claims the instance may take
- `concurrency`: {in_force, said_by}: how many claims the fleet of this checkout holds at work at once on this host — what `free_slots` counts from, beside the claims a fleet lock leaves alone and those whose worker declared — and who says it: `checkout`, where `afkconvoy run --concurrency` said it of this checkout, which keeps it on the checkout list; `default`, where it said none ({const:CONCURRENCY}). The repo config says none: a `concurrency` it names is read no more
- `harness`: {in_force, said_by}: the harness workers of this checkout start on, and who says it. `checkout`, where `afkconvoy run --harness` named one, which keeps it on the checkout list, and `in_force` is that harness even when it cannot start. `host_kit`, where it named none, and `in_force` is the host kit's first eligible harness, or null where none can start. A role that holds no limits for the harness in force is excluded with the checkout, the harness and that it holds no limits. Where it cannot start, `plan` and `doctor` say `account fault`, or `model fault` once no placed model is left, or that its launch cannot start, and no other harness is started in its place
- `tracker`: the repo the fleet of this checkout reads and writes its issues in, `<owner>/<repo>`, where `afkconvoy run --tracker` said one of it, which keeps it on the checkout list; null where it said none, and the checkout's own repo is its tracker. With one, the frontier, `excluded` and every other issue here are the open issues there that carry this repo's repo label, `repo:<owner>/<repo>`; one that carries the repo label of another repo too is excluded, "repo labels of more than one repo"
- `now`: the time the plan was made at, in epoch seconds
- `locks`: the fleet locks standing: the open issues that carry the lock label, lowest number first — none: []. While there is one, the frontier holds lock issues only, and every issue a tick would otherwise dispatch or hand to a human is under `excluded` with the reason "a fleet lock stands (#n)"
- `admission`: whether a tick would start a new worker on this host now (`afkconvoy run --help`, ADMISSION): {admits, held_back_by, shares, thresholds, reading}. held_back_by is each threshold the host is past, as a tick's line says it: none, and it admits. shares are the percentages in force — load_percent_of_cores, memory_free_percent, disk_free_percent — and said_by_host the host settings that said one, by key (admit_load, admit_memory, admit_disk, admit_inspect_after); thresholds what they come to on this host — load, memory_free_percent, disk_free_bytes; reading what the host reads now — cores, load, memory_free_percent, disk_free_bytes, disk_size_bytes, each null where it could not be read
- `worker_no_proxy`: every harness, with {harness, addition, no_proxy, NO_PROXY}: the host list added and the resulting login shell values, with this command's --worker-no-proxy flags. A null addition leaves the inherited spellings alone
- `held_back`: while admission holds: each threshold the host is past, as data: {by: "load", load, limit} in hundredths, {by: "memory", free, floor} in percent, {by: "disk", free, floor} in bytes. The frontier is still what a tick would dispatch once it admits
- `memory_pressure`: while a memory-pressure episode stands and another user account on the machine holds the most resident memory, more than this one, as the process table says it: {episode, active, since, level, account, uid, memory_bytes, processes, workers, unwatched} — the episode as `doctor`'s `host.resources` says it, that account and what its processes hold, the workers this host's fleets run, and why it starts no watch (`no_worker`, `left_with_operator`), null where one starts. Admission still holds new workers back under it
- `intake`: in a bound repo: {"open", "limit"} — the open issues taken in by workers, counted from their intake records, against the repo's `intake_limit`. With `open` at `limit`, `afkconvoy intake` refuses. An issue a human labelled is not among them
- `findings`: open blockers no fleet works, each {severity, what, evidence, fix, see}: the waiting issue, the blocker's repo and number, why no checkout serving the tracker works it, and the transfer and label lines. A consult or declared hand-off proceeds if these are its only blockers; build work waits
- `patrols`: in a bound repo that names patrols: each, in the order the config names them: {name, standing, every, last_run, open, max_open} — `every` in seconds, `last_run` when its last recorded run started (null: it never ran), `open` the open issues it opened. A patrol whose file says `max_runs_a_day` has it too, and `runs_in_day`: how many of its runs started in the last 24 hours. A patrol with `max_landed_a_day` reports it and `landed_in_day`: the issues it opened that closed as completed in the last rolling 24 hours. One a human asked to run now (`afkconvoy patrol-now --help`) has `asked`: when the ask was made, until a tick starts the run. standing says what a tick does about it:

  - `due`: its interval has passed, or a human asked for it: a tick runs it
  - `not_due`: its interval has not passed since `last_run`
  - `spent`: `runs_in_day` has reached `max_runs_a_day`: it is held until `until` — when the oldest of those runs is 24 hours old
  - `landed`: `landed_in_day` reached `max_landed_a_day`: held until `until`, when enough completions are 24 hours old to fall below the bound. A human's ask respects this bound too
  - `locked`: a fleet lock stands: no patrol runs
  - `full`: `open` has reached `max_open`
  - `failed`: its last run failed, and the issue that reports it — `failure`, in this case only — is still open
  - `running`: a run of `--instance` is still going
  - `peer_running`: a live peer's run is still going
  - `stale_run`: the run is claimed by a peer presumed dead: a tick deletes that claim

- `cut`: in a repo that turns cutting on (`afkconvoy cut --help`): {standing, newest_tag, initial, past, oldest_landing_age_seconds, busy_in_seconds, failure, failed, waiting} — `newest_tag` the newest version tag on the remote (none: it has none), `initial` the declared first version, present only without a version tag, `past` how many commits the base branch holds past its newest tag (0 without one), `oldest_landing_age_seconds` how long its oldest landing has waited, `busy_in_seconds` how long until a busy cut may start (zero at the bound; both ages null without a landing past a tag), `failure` the open issue that says the last cut failed — it holds no cut back, and a cut that publishes closes it — `failed`, while it is open, what that issue says, of its latest failure, and how the cut starts again — {issue, version, step, why, host, checkout, kept, goes_on, branch, restart}: the host and checkout it kept (checkout null: gone), `kept` here, elsewhere (on that host) or gone, `goes_on` its `--from` line, `branch` the branch it pushed {name, commit, taken_up} — taken up by the next cut while it stands on the base tip, else deleted as that cut starts — and `restart` all of it in words with its lines, `waiting` the open issues that wait on the next cut (`afkconvoy verdict --help`), off the frontier until a cut has published and installed its version. standing says what a tick does about it:

  - `due`: the base branch holds commits past `newest_tag`: the next idle tick starts a cut, or the next busy tick once `busy_in_seconds` is zero, whatever claims or frontier remain. An untagged remote with `initial` declared starts on an idle tick only
  - `tagged`: the base branch holds nothing past `newest_tag`: nothing to cut
  - `untagged`: the remote has no version tag and the repo declares no `initial` version: nothing is cut
  - `failed`: the last {const:CUT_REPEATS} cuts failed at the same step the same way, and nothing has landed past the base the last was cut from: `failure` is a ticket under the ready label, and the next cut waits for a landing. An open `failure` alone holds no cut back: the cut stands `due` beside it
  - `locked`: a fleet lock stands: no cut starts
  - `running`: a cut `--instance` started is still going
  - `peer_running`: a live peer's cut is still going
  - `stale_run`: it is claimed by a peer presumed dead: a tick deletes that claim
  - `off`: the repo turns cutting off, and a cut started before it did is on the base branch
- `slow_gate`: while the gate is slow: {gate_seconds, gate_budget_seconds?, host, runs, issue}. On `host`, the last {const:SLOW_GATE_LANDINGS} landings on the base branch that have a recorded run of the config's `gate_command` with a time all exceeded the fixed `gate_budget_seconds` when declared, or took more than {const:SLOW_GATE_PERCENT}% of the termination gate limit, `gate_seconds`, otherwise. runs are those {const:SLOW_GATE_LANDINGS}, newest first: {commit, seconds}. issue is the open issue that says so — the lowest open one carrying the label `slow-gate` — or null: a tick opens one (`afkconvoy tick --help`)

A claim's `status`, under `mine`, is one of:

{table:claim_status}

The probe's warnings go to stderr. Errors: a command the fleet needs is missing or logged out, or orca is not running or does not know this checkout (the error names each, with the line to run; `claude` is among them when no custom provider is set, as `run` would then start its workers with it); the repo fails the startup probe (the remote has no base branch, its protection refuses the fleet's push, its tracker is public — the error names it, with the line that says a private one — or the remote takes none of the fleet's refs); the config cannot be read; GitHub or the remote cannot be read.

## What trips you up

- Without `--instance` nobody's claims are its own: `mine` is empty, a running fleet's claims show under `peer_live`, and `free_slots` is the whole concurrency. To see a running fleet as it sees itself, pass its instance id — `takeover --list` shows every id the remote remembers.
- A lock that stands is not thereby worked. A lock issue is on the frontier only when it would be without the lock: with the ready label in an unbound repo, a flow label in a bound one, no epic label, no open blocker, no claim. `excluded` says which of these keeps it off — and the lock stands until the issue is closed.
- `frontier` is what the rebuild read, and `plan` reads nothing more. An issue that gains an open blocker after it is still listed; a tick reads each issue's blockers once more just before it claims it, and skips the issue then.
- A patrol is due or it is not: one that missed ten intervals runs once. And a run still going takes one slot of the concurrency, like a claim on an issue.
- With a lock standing, `free_slots` counts as free the slot of every claim of `--instance` the lock leaves alone: a lock issue does not wait for them.
- `free_slots` counts as free the slot of every claim of `--instance` that is `declared`: `mine` may hold more claims than the concurrency, and the frontier does not wait out the grace period of a worker that already declared.
- Fleet state is unchanged. When it probes it still pushes to the remote, and deletes there: it needs push access, and it is not a read-only command. To read without writing anything, run `afkconvoy doctor`.
- A gate gone slow is read off the recorded gate runs the remote still has: a record is swept a day after its run, a landing with no record or with a record that states no time is passed over, and {const:SLOW_GATE_LANDINGS} landings spread over hosts none of which has {const:SLOW_GATE_LANDINGS} say nothing. After an issue for a slow gate closed, only the landings made since count.
- A probe that stands is trusted, not checked: a command uninstalled or logged out since, or a rule the remote gained, shows as the error of whatever runs into it — or, for the ref layout, not until a probe runs. A noted ref layout moves when two probes that run, one after the other, each see it rejected and the other taken; one rejection leaves the note, and a probe that takes the noted layout clears that streak. A checkout that has noted no layout still settles on hidden refs, or on branches where the remote rejects those. `run` probes at every start.

## Worker proxy bypass

`--worker-no-proxy [HARNESS=]HOSTS` shows what a start with those flags would give each harness, under `worker_no_proxy`: [`afkconvoy help worker-proxy`](help:worker-proxy)."#;
