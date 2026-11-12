//! Resource episodes observed by ticks, shared across the host and kept across
//! runner moves. Doctor reads them; only a look starts the host's one watch.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use afkconvoy_core::resources::{Elsewhere, Holding, Reason, Unwatched};
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
    #[serde(default)]
    waiting: Vec<Value>,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct State {
    admission: BTreeMap<PathBuf, Observed>,
    pressure: Holding,
    hot: BTreeMap<PathBuf, Vec<HotNow>>,
    #[serde(default)]
    samples: BTreeMap<PathBuf, BTreeMap<PathBuf, afkconvoy_core::hot::Watch>>,
    /// The other account a reported watch left memory pressure with the
    /// operator for, until the operator answers it or the pressure's largest
    /// holder changes.
    #[serde(default)]
    left: Option<Left>,
}

/// Memory pressure from another account, left with the operator by a watch.
#[derive(Clone, Serialize, Deserialize)]
struct Left {
    account: String,
    uid: u32,
    /// The pressure episode the watch reported, `resource:pressure:<since>`.
    episode: String,
    watch: String,
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
    crate::admission::output("sysctl", &["-n", "kern.memorystatus_vm_pressure_level"])
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
        waiting: Vec<Value>,
    ) -> Result<()> {
        self.change(|state| {
            let observed = state.admission.entry(repo).or_default();
            if observed.waiting.is_empty() {
                // Also clears a host-wide hold saved by an older runner.
                observed.holding.ticked(now, &[]);
            }
            let reasons = if waiting.is_empty() { vec![] } else { admission.reasons() };
            observed.holding.ticked(now, &reasons);
            observed.stages = stages;
            observed.waiting = waiting;
            state.pressure.ticked(
                now,
                if pressure_level().is_some_and(|n| n >= 2) { &[Reason::Memory] } else { &[] },
            );
        })
    }

    pub fn hot(
        &self,
        repo: PathBuf,
        hot: Vec<HotNow>,
        samples: BTreeMap<PathBuf, afkconvoy_core::hot::Watch>,
    ) -> Result<()> {
        self.change(|state| {
            state.samples.insert(repo.clone(), samples);
            state.hot.insert(repo, hot);
        })
    }

    fn change(&self, change: impl FnOnce(&mut State)) -> Result<()> {
        let _held = self.0.lock().unwrap_or_else(|p| p.into_inner());
        // The watch and the asking session change it too, outside this mutex.
        let lock = cache()?.join("watch/resources.lock");
        std::fs::create_dir_all(lock.parent().context("resource record has no directory")?)?;
        let lock =
            std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(lock)?;
        lock.lock()?;
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

/// Sampler state carried through a host runner replacement.
pub fn samples(repo: &std::path::Path) -> Result<BTreeMap<PathBuf, afkconvoy_core::hot::Watch>> {
    Ok(read()?.samples.remove(repo).unwrap_or_default())
}

/// Current conditions, with stable episode identities. Inactive episodes are
/// kept during their five-minute recovery, preserving a watch's coverage.
/// Every checkout's admission comes from the look's shared host reading.
/// `elsewhere` is the other account holding the most memory, as the look's
/// process table says it: pressure while it does, and no fleet worker runs
/// or a watch already left it with the operator, starts no watch.
pub fn json(
    now: u64,
    readings: &crate::admission::Readings,
    elsewhere: Option<&Elsewhere>,
) -> Result<Value> {
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
        let admission = Admission::read_from(repo, readings)?;
        let mut holding = observed.holding.clone();
        let reasons = if observed.waiting.is_empty() { vec![] } else { admission.reasons() };
        holding.ticked(now, &reasons);
        if let Some(episode) = holding.episode() {
            let due = holding.due(now, span);
            conditions.push(
                json!({"id": format!("resource:admission:{}:{episode}", repo.display()),
                "kind": "admission", "repo": repo, "episode": episode,
                "active": due.is_some(), "since": due.map(|due| due.since),
                "reason": due.map(|due| due.reason), "admission": admission.json(),
                "stages": observed.stages, "waiting": observed.waiting}),
            );
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
        // The workers each fleet's sampler watches now, on the listed checkouts.
        let workers: usize =
            state.samples.iter().filter(|(repo, _)| on_list(repo)).map(|(_, w)| w.len()).sum();
        let left = state.left.as_ref().map(|left| left.uid);
        let unwatched =
            afkconvoy_core::resources::unwatched(elsewhere.map(|held| held.uid), workers, left);
        conditions.push(json!({"id": format!("resource:pressure:{episode}"), "kind": "pressure",
            "episode": episode, "active": due.is_some(), "since": due.map(|due| due.since),
            "reason": "memory", "level": pressure, "elsewhere": elsewhere, "workers": workers,
            "unwatched": unwatched,
            "left": state.left.as_ref().map(|left| json!({"account": left.account,
                "uid": left.uid, "episode": left.episode, "watch": left.watch}))}));
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
            let (kind, what) = match kind {
                "hot" => (
                    crate::doctor::Kind::HotWorker,
                    format!(
                        "hot worker is past its limits: {}: {}",
                        resource_text(r, "worker"),
                        resource_text(r, "condition")
                    ),
                ),
                "pressure" => match elsewhere_said(r) {
                    Some(said) => (crate::doctor::Kind::PressureElsewhere, said(seconds)),
                    None => (
                        crate::doctor::Kind::MemoryPressure,
                        format!("the host has been under memory pressure for {seconds}s"),
                    ),
                },
                _ => (
                    crate::doctor::Kind::AdmissionHeld,
                    format!(
                        "admission has held back new workers for {} for {seconds}s",
                        resource_text(r, "reason")
                    ),
                ),
            };
            let mut finding = crate::doctor::finding(
                kind,
                serde_json::json!(["resource", r["id"]]),
                what,
                r.to_string(),
                "afkconvoy doctor --all".into(),
            );
            // A resource's own id already holds its scope.
            if let Some(id) = r["id"].as_str() {
                finding.watch_id = id.to_string();
            }
            finding
        })
        .collect()
}

/// What doctor says of pressure `resource` that starts no watch: the
/// account it comes from and its memory, and why no watch acts on it.
fn elsewhere_said(resource: &Value) -> Option<impl Fn(u64) -> String> {
    let unwatched: Unwatched = serde_json::from_value(resource["unwatched"].clone()).ok()?;
    let held: Elsewhere = serde_json::from_value(resource["elsewhere"].clone()).ok()?;
    Some(move |seconds| {
        let Elsewhere { account, uid, memory_bytes, processes } = &held;
        let gib = *memory_bytes as f64 / (1u64 << 30) as f64;
        let why = match unwatched {
            Unwatched::NoWorker => "this host's fleets hold no worker".to_string(),
            Unwatched::LeftWithOperator => {
                "a watch already left it with the operator, who has not answered it".to_string()
            }
        };
        format!(
            "the host has been under memory pressure from another account for {seconds}s: \
             the account {account} (uid {uid}) holds {gib:.1} GiB resident in {processes} \
             processes, more than this one; {why}, and a watch keeps away from another \
             account's programs, so none starts"
        )
    })
}

fn resource_text<'a>(resource: &'a Value, key: &str) -> &'a str {
    resource[key].as_str().unwrap_or("unknown")
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

/// A reported watch `watch` on pressure from another account left it with
/// the operator: a later episode of that same account's starts no watch
/// until the operator answers it or the largest holder changes. Its look
/// says each pressure item's largest holder.
pub fn reported(watch: &crate::host_watch::Watch) -> Result<()> {
    let resources = watch.look["host"]["resources"].as_array().cloned().unwrap_or_default();
    let left = watch.items.iter().find_map(|item| {
        let resource = resources.iter().find(|r| r["id"] == item.id && r["kind"] == "pressure")?;
        let held: Elsewhere = serde_json::from_value(resource["elsewhere"].clone()).ok()?;
        Some(Left {
            account: held.account,
            uid: held.uid,
            episode: item.id.clone(),
            watch: watch.id.clone(),
        })
    });
    let Some(left) = left else { return Ok(()) };
    Shared::default().change(|state| state.left = Some(left))
}

/// The operator answered a question a watch left on `condition`: pressure
/// it left with them is theirs no more.
pub fn answered(condition: &str) -> Result<()> {
    if !condition.starts_with("resource:pressure:") {
        return Ok(());
    }
    Shared::default().change(|state| state.left = None)
}

/// A look that finds memory pressure whose largest holder is not the
/// account a watch left it with the operator for — a different account, or
/// this one's fleet — ends that: the next such pressure is new.
pub fn looked(look: &Value) -> Result<()> {
    let Some(pressure) = look["host"]["resources"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|resource| resource["kind"] == "pressure")
    else {
        return Ok(());
    };
    let Some(left) = pressure["left"]["uid"].as_u64() else { return Ok(()) };
    let elsewhere = pressure["elsewhere"]["uid"].as_u64().map(|uid| uid as u32);
    if afkconvoy_core::resources::still_left(left as u32, elsewhere) {
        return Ok(());
    }
    Shared::default().change(|state| state.left = None)
}

/// `plan`'s `memory_pressure`: the host's memory-pressure episode while
/// another account holds the most memory — that account, what it holds, and
/// whether a watch starts on it — or `None` while there is none.
pub fn from_elsewhere(now: u64, readings: &crate::admission::Readings) -> Option<Value> {
    let table = crate::machine::Table::read().ok()?;
    let elsewhere = table.elsewhere()?;
    let resources = json(now, readings, Some(&elsewhere)).ok()?;
    let pressure = resources.as_array()?.iter().find(|r| r["kind"] == "pressure")?.clone();
    Some(json!({"episode": pressure["id"], "active": pressure["active"],
        "since": pressure["since"], "level": pressure["level"],
        "account": elsewhere.account, "uid": elsewhere.uid,
        "memory_bytes": elsewhere.memory_bytes, "processes": elsewhere.processes,
        "workers": pressure["workers"], "unwatched": pressure["unwatched"]}))
}
