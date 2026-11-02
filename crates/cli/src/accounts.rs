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
