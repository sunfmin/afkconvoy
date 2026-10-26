//! Resource episodes observed by ticks, shared across the host and kept across
//! runner moves. Doctor reads them; only a look starts the host's one watch.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use afkconvoy_core::resources::{Holding, Reason};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::admission::Admission;
use crate::copies::cache;
use crate::hot::HotNow;

const FILE: &str = "watch/resources.json";

#[derive(Default, Clone, Serialize, Deserialize)]
struct Observed {
    holding: Holding,
    stages: Vec<(u64, String)>,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct State {
    admission: BTreeMap<PathBuf, Observed>,
    pressure: Holding,
    hot: BTreeMap<PathBuf, Vec<HotNow>>,
}

fn read() -> Result<State> {
    let file = cache()?.join(FILE);
    match std::fs::read(&file) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("cannot read resource episodes in {}", file.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(error) => Err(error.into()),
    }
}

/// The host's kernel pressure signal, independent of admission's memory share.
pub fn pressure_level() -> Option<u64> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    crate::host_sleep::output("sysctl", &["-n", "kern.memorystatus_vm_pressure_level"])
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Every fleet instance observes the same host episodes. The cache is the
/// durable record; the mutex serializes its writes within the host runner.
#[derive(Clone, Default)]
pub struct Shared(Arc<Mutex<()>>);

impl Shared {
    pub fn ticked(
        &self,
        now: u64,
        admission: &Admission,
        repo: PathBuf,
        stages: Vec<(u64, String)>,
    ) -> Result<()> {
        self.change(|state| {
            let reasons = admission.reasons();
            let shared = state
                .admission
                .values()
                .filter(|observed| {
                    observed.holding.due(now, 0).is_some_and(|due| reasons.contains(&due.reason))
                })
                .min_by_key(|observed| observed.holding.episode())
                .map(|observed| observed.holding.clone())
                .unwrap_or_default();
            let observed = state
                .admission
                .entry(repo)
                .or_insert_with(|| Observed { holding: shared, stages: vec![] });
            observed.holding.ticked(now, &admission.reasons());
            observed.stages = stages;
            state.pressure.ticked(
                now,
                if pressure_level().is_some_and(|n| n >= 2) { &[Reason::Memory] } else { &[] },
            );
        })
    }

    pub fn hot(&self, repo: PathBuf, hot: Vec<HotNow>) -> Result<()> {
        self.change(|state| {
            state.hot.insert(repo, hot);
        })
    }

    fn change(&self, change: impl FnOnce(&mut State)) -> Result<()> {
        let _held = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let mut state = read()?;
        change(&mut state);
        let file = cache()?.join(FILE);
        std::fs::create_dir_all(file.parent().context("resource record has no directory")?)?;
        let temporary = file.with_extension(format!("{}.tmp", crate::unique()));
        std::fs::write(&temporary, serde_json::to_vec(&state)?)?;
        std::fs::rename(temporary, file)?;
        Ok(())
    }
}

/// Current conditions, with stable episode identities. Inactive episodes are
/// kept during their five-minute recovery, preserving a watch's coverage.
pub fn json(now: u64) -> Result<Value> {
    let state = read()?;
    let span = crate::host_settings::of_host()?
        .admit_inspect_after
        .unwrap_or(afkconvoy_core::resources::SPAN);
    let mut conditions = vec![];
    let listed = crate::checkout_list::List::of_host()?.read()?.checkouts;
    let on_list = |repo: &PathBuf| listed.iter().any(|entry| &entry.checkout == repo);
    for (repo, observed) in &state.admission {
        if !on_list(repo) {
            continue;
        }
        let admission = Admission::read(repo)?;
        let mut holding = observed.holding.clone();
        holding.ticked(now, &admission.reasons());
        if let Some(episode) = holding.episode() {
            let due = holding.due(now, span);
            conditions.push(json!({"id": format!("resource:admission:{episode}"),
                "kind": "admission", "repo": repo, "episode": episode,
                "active": due.is_some(), "since": due.map(|due| due.since),
                "reason": due.map(|due| due.reason), "admission": admission.json(),
                "stages": observed.stages}));
        }
    }
    let pressure = pressure_level();
    let mut holding = state.pressure.clone();
    holding.ticked(
        now,
        if pressure.is_some_and(|level| level >= 2) { &[Reason::Memory] } else { &[] },
    );
    if let Some(episode) = holding.episode() {
        let due = holding.due(now, span);
        conditions.push(json!({"id": format!("resource:pressure:{episode}"), "kind": "pressure",
            "episode": episode, "active": due.is_some(), "since": due.map(|due| due.since),
            "reason": "memory", "level": pressure}));
    }
    for (repo, workers) in state.hot {
        if !on_list(&repo) {
            continue;
        }
        for hot in workers {
            let worktree = hot.worktree();
            conditions.push(
                json!({"id": format!("resource:hot:{}:{}", hot.path.display(), hot.episode.since),
                "kind": "hot", "repo": repo, "active": true, "since": hot.episode.since,
                "worker": hot.worker, "issue": hot.issue, "worktree": hot.path,
                "terminal": worktree.terminal().ok().flatten(), "condition": hot.why,
                "held": crate::hot::held_said(&hot.episode),
                "evidence": crate::evidence::of(&worktree, &hot.worker, hot.episode.since)}),
            );
        }
    }
    Ok(json!(conditions))
}

/// Resource findings carry their episode identity so a changing reading does
/// not start another watch. Doctor names the condition and its duration.
pub fn findings(resources: &Value, now: u64) -> Vec<crate::doctor::Finding> {
    resources
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["active"] == true)
        .map(|r| {
            let kind = r["kind"].as_str().unwrap_or("");
            let seconds = now.saturating_sub(r["since"].as_u64().unwrap_or(now));
            let what = match kind {
                "hot" => {
                    format!("hot worker {} is past its limits: {}", r["worker"], r["condition"])
                }
                "pressure" => format!("the host has been under memory pressure for {seconds}s"),
                _ => format!(
                    "admission has held back new workers for {} for {seconds}s",
                    r["reason"]
                ),
            };
            crate::doctor::Finding {
                severity: crate::doctor::Severity::Warning,
                what,
                evidence: r.to_string(),
                fix: "afkconvoy doctor --all".into(),
                see: crate::topic::See::Troubleshooting,
                of_host: true,
                watch_id: r["id"].as_str().map(str::to_string),
            }
        })
        .collect()
}

/// Take the former host evidence only for a condition a watch will act on.
/// The complete reading and the evidence path travel in its look and brief.
pub fn evidence(look: &mut Value, items: &[afkconvoy_core::host_watch::Item]) -> Result<()> {
    let Some(resources) = look["host"]["resources"].as_array_mut() else { return Ok(()) };
    for resource in resources {
        if !items.iter().any(|item| resource["id"] == item.id) || resource["kind"] == "hot" {
            continue;
        }
        let repo = resource["repo"].as_str().map(PathBuf::from).or_else(|| {
            crate::checkout_list::List::of_host()
                .ok()?
                .read()
                .ok()?
                .checkouts
                .first()
                .map(|entry| entry.checkout.clone())
        });
        let Some(repo) = repo else { continue };
        let admission = Admission::read(&repo)?;
        let reason = resource["reason"].as_str().and_then(Reason::read).unwrap_or(Reason::Memory);
        let stages: Vec<(u64, String)> =
            serde_json::from_value(resource["stages"].clone()).unwrap_or_default();
        let own = crate::host_evidence::Own { repo: &repo, stages: &stages };
        let reading = resource.to_string();
        resource["evidence"] =
            json!(crate::host_evidence::take(&admission, reason, &reading, &own)?);
    }
    Ok(())
}
