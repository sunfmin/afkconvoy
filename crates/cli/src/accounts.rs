//! Read-only work-account reporting, without opening a fleet or probing it.

use anyhow::Result;
use serde_json::Value;

use crate::{checkout_list, gh::GitHub, git::Repo};

/// Read the tracker's accounts and summarize the requested window. The
/// caller bounds days so converting it to seconds cannot overflow.
pub fn accounts(repo: &Repo, days: u64, all: bool, rows: bool, now: u64) -> Result<Value> {
    let tracker = checkout_list::tracker_of(repo)?;
    let gh = GitHub::new(repo.dir.clone(), tracker);
    let start = (!all).then(|| now.saturating_sub(days.saturating_mul(86400)));
    let issues = gh.accounts()?;
    let watch_rounds =
        afkconvoy_core::accounts::usage_rows(crate::host_watch::rounds()?, start, now);
    let asking_questions =
        afkconvoy_core::accounts::usage_rows(crate::host_asking::usages()?, start, now);
    let patrol_runs = afkconvoy_core::accounts::usage_rows(
        crate::patrol_usage::runs(&gh.repository()?)?,
        start,
        now,
    );
    if rows {
        let (rows, unreadable) = afkconvoy_core::accounts::rows(issues, start, now);
        let mut text = String::new();
        for row in rows {
            text.push_str(&serde_json::to_string(&row)?);
            text.push('\n');
        }
        for (kind, usages) in [
            ("watch_round", &watch_rounds),
            ("asking_question", &asking_questions),
            ("patrol_run", &patrol_runs),
        ] {
            for usage in usages {
                let row = afkconvoy_core::accounts::UsageRow { kind, usage };
                text.push_str(&serde_json::to_string(&row)?);
                text.push('\n');
            }
        }
        if !unreadable.is_empty() {
            eprintln!("{}", serde_json::json!({"unreadable": unreadable}));
        }
        return Ok(Value::String(text));
    }
    Ok(serde_json::to_value(afkconvoy_core::accounts::summarize_with_usage(
        issues,
        watch_rounds,
        asking_questions,
        patrol_runs,
        start,
        now,
    ))?)
}

/// What `afkconvoy accounts --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

Run `afkconvoy accounts` in a checkout to measure how long its issues took, or compare windows before and after a fleet change. Needs git and a signed-in `gh`; no running fleet or orca is needed.

## Who runs it

The operator, or an agent reading completion times for them, in any checkout or worktree of the repo on the host whose tracker choice is to be read.

## What it reads and writes

Reads this host's checkout list to find the tracker, then every open and closed issue's work account through `gh`, paging through issues and comments. Where the tracker is another repo, only this repo's repo label's issues belong. An issue carrying several repo labels belongs to none. With no tracker named on the host, the checkout's own repo is its tracker. It also reads this host's durable watch-round, asking-question and patrol-run usage records. It writes nothing and starts no worker.

The window includes accounts and host usage records whose end is between now minus 7 days and now, inclusive. `--days <n>` changes that duration; `--all` includes every record ending by now. Issue closure and update dates do not select the window. Issues with no account are not samples. Watch rounds, asking questions and patrol runs are selected by their recorded end.

Every readable work account carries one authoritative versioned JSON object in a collapsed details block with a fenced json block. Its human text is rendered from that object and is never parsed back. Old prose is converted by `rewrite-accounts` before this command can read it.

## What it answers

One JSON object: `window` has `start` and `end` in epoch seconds (`start` null with `--all`); `count` counts readable accounts in it; `wall_seconds` has p50, p90, p95 and max of end minus earliest claim start. Percentiles use nearest rank: sort ascending and take rank ceil(p * count / 100), one based; an empty sample answers null for each measure.

`by_harness` maps the first worker's harness to its `count` and `wall_seconds`, with the same percentiles; `unknown` holds issues with no worker. `multiple_workers` counts issues with more than one worker. `gate_seconds` has p50 and p95 of each issue's sum of timed gate rows, zero when no gate ran. Worker rows and gate rows overlap the wall clock: they are never added into it, and no remainder is computed.

`tokens` sums known workers' raw counts as `totals` (input, cache_read, cache_write, output and reasoning); reasoning is part of output, not added to it. `unknown_workers` counts workers with no readable usage. `p50` and `p95` are nearest-rank issue totals over issues with at least one known worker; partial totals still count and unknown workers are reported. `by_harness` gives the same totals, percentiles and unknown count for each harness, grouping each issue's workers of that harness. `top` lists up to five issues by known total, ties by issue number, with number, url and total. No price is applied.

`watch_rounds`, `asking_questions` and `patrol_runs` summarize host work with no issue work account. Each has `count`, known token `totals`, `unknown_rounds`, nearest-rank `p50` and `p95` per known record, `by_harness` with the same measures, and `top`: up to five known records by descending total, ties by id, each with `id`, `conditions` and `total`. A watch round's start is its delivery and end its report, with continuations included and standby excluded. Asking questions merge their delivery segments. Patrol runs have one record per harness. Missing or unreadable session usage is unknown, never zero. No price is applied to these either.

Their NDJSON rows and durable records have `id`, `harness`, `model`, `start`, `end`, `seconds`, `tokens` and `conditions`. Times are epoch seconds; `seconds` is end minus start. `tokens` has the same model map and null-on-unknown meaning as a work account worker's. Watch conditions are the delivered condition ids; asking conditions identify the question's condition; patrol conditions are `patrol:<name>`.

`slowest` lists up to five issues in descending wall clock (ties by issue number), each with `number`, `url` and `wall_seconds`. `unreadable` lists `number` and `reason` for accounts with unknown or invalid starts, ends, worker rows or gate seconds, duplicate accounts, or comments that could not be read. They are excluded from all measures. An unreadable end or failed comment read cannot be placed in a window and is reported in every window; an account with a readable end outside the window is excluded.

A failed issue listing refuses the command with {"error": …} and a non-zero exit. Check `gh auth status`, correct the reported GitHub or checkout-list fault, then run it again.

With `--rows`, stdout is NDJSON instead: one compact JSON object per account in the window, ordered by issue number, with the schema below plus `url`, the issue's URL. Host usage rows follow, ordered by id within each kind: `watch_round`, then `asking_question`, then `patrol_run`, with `kind` beside the usage fields above. An empty window prints no lines. Unknown measurements remain null in these rows even when they exclude the account from the summary. Accounts that cannot be decoded, including unknown schema versions, duplicate accounts and failed comment reads, are omitted and listed under `unreadable` in one JSON object on stderr (only when nonempty). Read stderr as well as stdout. This mode still exits 0 for unreadable accounts and writes nothing.

## Work account schema

`schema_version` is {const:ACCOUNT_SCHEMA_VERSION}. A field's meaning stays fixed within a version; a breaking change raises the version. Unknown values are JSON null, never a substitute zero or an invented default. Times (`start`, `end`, `at`) are epoch seconds. Duration and size field names carry their units.

- `issue`: the tracker issue number. `repo`: the code repo, owner/name. `tracker`: the tracker, owner/name. `afkconvoy_version`: the version of the binary that wrote the account.
- `host`: {name, cpu, logical_cpus, memory_bytes}, as reported by the writing host; cpu is its chip name, else its machine model identifier.
- `start`: earliest recorded claim since, retained across later ends. `end`: the end being accounted for. `wall_seconds`: end minus start, null if unknown or reversed.
- `outcome`: landed, closed (nothing more to do), escalated, ended (the workflow ended), or human-closed (closed by a human while claimed). An unknown outcome is null.
- `workers`: a list of {harness, model, role, stage, claim, start, end, seconds, verdict, tokens}. `tokens` is null when usage is unknown; otherwise it maps model names to {input, cache_read, cache_write, output, reasoning}, raw counts with reasoning null when not stated. An empty model name means unknown. Cached input is excluded from input, and reasoning is included in output. A model not explicitly named is null. Role and stage are null where there is none or neither was recorded. Claim is the claim's identity. A bound worker ends at its stage record or failure, else the account end; an unbound worker ends at the account end. Seconds are end minus start. Verdict is its stage record's verdict, null without one. Landing workers are not rows; continuations on the same harness are not new rows. A worker on a harness that keeps a goal — Codex — also has `goal`, absent for any other: {set, objective, goal_turns, goal_turns_after_verdict, status, tokens_used, token_budget, session}, read from its session record and its home's goal store while its worktree holds them. `set` says whether its session was set a goal, and `objective` is what `/goal` was typed with: the brief pointer. `goal_turns` counts the turns with `turn_trigger: goal`, its setting's first turn among them; `goal_turns_after_verdict` those begun after its verdict stood — its stage record, else its last verdict comment — null without one. `status` is the store's final `thread_goals.status`, or `cleared` where the goal was set and its row is gone, as `/goal clear` leaves it. `tokens_used` and `token_budget` are the store's, uncached input and output; a cleared goal's are null. `session` is the session record's path on the writing host.
- `gates`: a list of {tree, command, lane, `result`, at, seconds, host}. Tree is the tested git tree, command is the exact command, lane is fast, slow or whole when known, `result` is green or red, at is the gate record's epoch timestamp, seconds times the command after its queue turn, and host is the host that ran it. Prior red runs survive later edits after their remote refs disappear. For converted records, a lane stays null unless its original marker names one.

A converted account retains its original evidence in `legacy_evidence`. Worker durations, models and host hardware stated by the old rows are recovered; facts not stated remain null. The overlapping worker and gate durations do not partition wall_seconds.

## What trips you up

The sample is work accounts, including an account on an issue still open after escalation. It does not measure every closed issue. Unknown gate seconds make the account unreadable; no gate row means zero. Read `unreadable` with the count before comparing two windows."#;
