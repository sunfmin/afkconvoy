//! The host runner's watch of its host. A quiet doctor look starts no model;
//! actionable items share one bounded, recoverable watch in the operator's
//! directory. The process holds no authority: the look, session and records
//! are read back from orca and the host's cache after every restart.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use afkconvoy_core::harness::Harness;
use afkconvoy_core::host_watch::{self, Item};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::checkout_list::List;
use crate::copies::cache;
use crate::dispatch::Agent;
use crate::git::Repo;
use crate::log::{Entry, What};
use crate::orca::Worktree;
use crate::stop::Stop;

const STATE: &str = "watch/state.json";
const ACTIONS: usize = 80;

/// A watch as the host keeps it, including what a continuation needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Watch {
    pub id: String,
    pub directory: PathBuf,
    pub look: Value,
    pub items: Vec<Item>,
    pub started: u64,
    #[serde(deserialize_with = "crate::kit::harness")]
    pub harness: Harness,
    pub model: Option<String>,
    pub terminal: String,
    #[serde(default)]
    pub earlier: Vec<String>,
    #[serde(default)]
    pub did: Vec<String>,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

/// One session of a watch, before a continuation or the end.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attempt {
    #[serde(deserialize_with = "crate::kit::harness")]
    pub harness: Harness,
    pub model: Option<String>,
    pub terminal: String,
    #[serde(default)]
    pub did: Vec<String>,
}

/// What the last watch did; the original look is not repeated in doctor.
pub fn watch_json(watch: &Watch) -> Value {
    json!({"id": watch.id, "directory": watch.directory, "items": watch.items,
        "started": watch.started, "harness": watch.harness, "model": watch.model,
        "terminal": watch.terminal, "did": watch.did, "attempts": watch.attempts,
        "ended": watch.ended, "outcome": watch.outcome})
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    until: Option<u64>,
    watched: Vec<String>,
    running: Option<Watch>,
    last: Option<Watch>,
    history: Vec<Watch>,
    unavailable: Option<String>,
}

/// The running watch shared with doctor and the panel.
#[derive(Clone, Default)]
pub struct Shared {
    running: Arc<Mutex<Option<Watch>>>,
}

impl Shared {
    fn set(&self, running: Option<Watch>) {
        *self.lock() = running;
    }

    fn lock(&self) -> MutexGuard<'_, Option<Watch>> {
        self.running.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn running(&self) -> Option<Watch> {
        self.lock().clone()
    }
}

/// A host watch thread. Dropping it waits for its coordinator, never for or
/// kills the operator's session: a later host runner adopts that session.
pub struct HostWatch {
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HostWatch {
    pub fn start(
        list: List,
        stop: Stop,
        shared: Shared,
        say: impl Fn(What) + Send + Sync + 'static,
    ) -> HostWatch {
        let thread = std::thread::spawn(move || {
            let mut coordinator = Coordinator { list, stop, shared, say: Arc::new(say) };
            coordinator.run();
        });
        HostWatch { thread: Some(thread) }
    }
}

impl Drop for HostWatch {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Coordinator {
    list: List,
    stop: Stop,
    shared: Shared,
    say: Arc<dyn Fn(What) + Send + Sync>,
}

impl Coordinator {
    fn run(&mut self) {
        let mut next_look = Instant::now();
        while !self.stop.told() {
            let settings = crate::host_settings::of_host();
            let directory = settings.as_ref().ok().and_then(|settings| settings.watch_dir.clone());
            let Some(directory) = directory.filter(|dir| dir.is_dir()) else {
                self.shared.set(None);
                std::thread::sleep(Duration::from_millis(250));
                next_look = Instant::now();
                continue;
            };
            if let Err(error) = self.monitor(&directory) {
                (self.say)(What::WatchEnded {
                    id: "unknown".into(),
                    outcome: "monitor_failed".into(),
                    said: format!("{error:#}"),
                });
            }
            if Instant::now() >= next_look {
                let every = settings.map_or(crate::host_settings::WATCH_EVERY, |s| s.watch_every());
                next_look = Instant::now() + Duration::from_secs(every);
                if let Err(error) = self.look(&directory) {
                    (self.say)(What::WatchEnded {
                        id: "look".into(),
                        outcome: "look_failed".into(),
                        said: format!("{error:#}"),
                    });
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn read(&self) -> Result<State> {
        let file = cache()?.join(STATE);
        match std::fs::read(&file) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)
                .with_context(|| format!("cannot read watch state {}", file.display()))?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(error) => Err(error.into()),
        }
    }

    fn save(&self, state: &State) -> Result<()> {
        let file = cache()?.join(STATE);
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temporary = file.with_extension(format!("{}.tmp", crate::unique()));
        std::fs::write(&temporary, serde_json::to_vec_pretty(state)?)?;
        std::fs::rename(temporary, &file)?;
        self.shared.set(state.running.clone());
        Ok(())
    }

    fn look(&mut self, directory: &Path) -> Result<()> {
        let mut state = self.read()?;
        let program = std::env::current_exe().context("cannot find this afkconvoy")?;
        let mut args = vec!["doctor".to_string(), "--all".to_string()];
        if let Some(since) = state.until {
            args.extend(["--since".into(), since.to_string()]);
        }
        let output = Command::new(program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("the host's doctor look could not be started")?;
        let look: Value = serde_json::from_slice(&output.stdout)
            .with_context(|| format!("the host's doctor look answered no JSON"))?;
        if look["error"].is_string() && look["until"].is_null() {
            bail!("the host's doctor look failed: {}", look["error"]);
        }
        if let Some(until) = look["until"].as_u64() {
            state.until = Some(until);
        }
        let now = look["until"].as_u64().unwrap_or_else(crate::clock);
        let current = actionable(&look, now);
        state.watched = host_watch::covered(&state.watched, &current);
        let pending = host_watch::pending(&current, &state.watched, state.running.is_some());
        if !pending.is_empty() {
            self.start(&mut state, directory, look, pending)?;
        }
        self.save(&state)
    }

    fn start(
        &self,
        state: &mut State,
        directory: &Path,
        look: Value,
        items: Vec<Item>,
    ) -> Result<()> {
        if state.running.is_some() {
            return Ok(());
        }
        let ids: BTreeSet<&str> = items.iter().map(|item| item.id.as_str()).collect();
        let earlier: Vec<String> = state
            .history
            .iter()
            .filter(|watch| watch.items.iter().any(|item| ids.contains(item.id.as_str())))
            .flat_map(|watch| watch.did.iter().cloned().chain(watch.outcome.clone()))
            .collect();
        let mut failures = vec![];
        for candidate in crate::health::watch_candidates()? {
            let now = crate::clock();
            let watch = Watch {
                id: format!("{now}-{}", crate::unique()),
                directory: directory.to_path_buf(),
                look: look.clone(),
                items: items.clone(),
                started: now,
                harness: candidate.harness,
                model: candidate.model.clone(),
                terminal: String::new(),
                earlier: earlier.clone(),
                did: vec![],
                attempts: vec![],
                ended: None,
                outcome: None,
            };
            match self.start_session(&watch, &[]) {
                Ok(terminal) => {
                    let mut watch = watch;
                    watch.terminal = terminal;
                    state.watched.extend(items.iter().map(|item| item.id.clone()));
                    state.watched.sort();
                    state.watched.dedup();
                    state.unavailable = None;
                    (self.say)(What::WatchStarted {
                        id: watch.id.clone(),
                        directory: watch.directory.display().to_string(),
                        harness: watch.harness.key().to_string(),
                        reasons: items.iter().map(|item| item.reason.clone()).collect(),
                    });
                    state.running = Some(watch);
                    self.save(state)?;
                    return Ok(());
                }
                Err(error) => {
                    let said = format!("{error:#}");
                    failures.push(format!("{}: {said}", candidate.harness.key()));
                    let _ = crate::health::watch_fault(candidate.harness, vec![said], now);
                }
            }
        }
        state.unavailable = Some(if failures.is_empty() {
            "no harness has both a successful worker and a clean health record".into()
        } else {
            failures.join("; ")
        });
        self.save(state)
    }

    fn start_session(&self, watch: &Watch, earlier: &[String]) -> Result<String> {
        let listed = self.list.read()?.checkouts;
        let first = listed.first().context("no checkout for the watch's host kit")?;
        let repo = Repo::at(&first.checkout)?;
        let agent =
            Agent { launches: crate::launch::Launches::new(&repo, true)?, ready_timeout: 120 };
        let command = agent.command(watch.harness)?;
        let kept = crate::kept::keep(watch.harness, false)?;
        let brief = brief(watch, earlier);
        let launch = watch.harness.watch(
            command.as_str(),
            &watch.directory.display().to_string(),
            &kept.display().to_string(),
            watch.model.as_deref(),
            &brief,
        );
        Worktree::at(watch.directory.clone(), String::new()).start_watch(&format!("{launch}; exit"))
    }

    fn monitor(&mut self, directory: &Path) -> Result<()> {
        let mut state = self.read()?;
        let Some(mut watch) = state.running.take() else {
            self.shared.set(None);
            return Ok(());
        };
        let worktree = Worktree::at(directory.to_path_buf(), String::new());
        if watch.terminal.is_empty() {
            watch.terminal = worktree.terminal()?.unwrap_or_default();
        }
        let now = crate::clock();
        let expired = host_watch::expired(watch.started, now);
        let terminal = watch.terminal.clone();
        let output = if terminal.is_empty() {
            vec![]
        } else {
            worktree.screen(&terminal, ACTIONS).unwrap_or_default()
        };
        let mut new = actions(&watch.did, &output);
        let gone = worktree.state(now)? == afkconvoy_core::worker::WorkerState::Gone;
        let structured = worktree.health_report().unwrap_or(Value::Null);
        let fault = crate::health::terminal_fault(watch.harness, &output, &structured);
        if !gone && !expired && fault.is_none() {
            state.running = Some(watch);
            self.shared.set(state.running.clone());
            return Ok(());
        }
        if expired || fault.is_some() {
            let _ = worktree.stop_for_continuation();
        }
        watch.did.extend(new.drain(..));
        if expired {
            self.finish(&mut state, watch, "time_limit", "the watch reached its hour");
            return self.save(&state);
        }
        if let Some(fault) = fault {
            let why = fault.evidence.join("; ");
            let from = watch.harness;
            (self.say)(What::WatchDid {
                id: watch.id.clone(),
                did: format!("harness fault: {why}"),
            });
            crate::health::watch_fault(from, fault.evidence, now)?;
            let attempt = Attempt {
                harness: watch.harness,
                model: watch.model.clone(),
                terminal: watch.terminal.clone(),
                did: watch.did.clone(),
            };
            watch.attempts.push(attempt);
            let earlier = watch.did.clone();
            for candidate in crate::health::watch_candidates()? {
                if watch.attempts.iter().any(|attempt| attempt.harness == candidate.harness) {
                    continue;
                }
                watch.harness = candidate.harness;
                watch.model = candidate.model;
                match self.start_session(&watch, &earlier) {
                    Ok(terminal) => {
                        watch.terminal = terminal;
                        (self.say)(What::WatchContinued {
                            id: watch.id.clone(),
                            from: from.key().into(),
                            to: watch.harness.key().into(),
                            why: why.clone(),
                        });
                        state.running = Some(watch);
                        return self.save(&state);
                    }
                    Err(error) => {
                        let said = format!("{error:#}");
                        let _ = crate::health::watch_fault(candidate.harness, vec![said], now);
                    }
                }
            }
            state.unavailable = Some("no harness can continue the watch".into());
            self.finish(&mut state, watch, "no_harness", &why);
            return self.save(&state);
        }
        self.finish(&mut state, watch, "ended", "its session ended");
        self.save(&state)
    }

    fn finish(&self, state: &mut State, mut watch: Watch, outcome: &str, said: &str) {
        watch.ended = Some(crate::clock());
        watch.outcome = Some(outcome.into());
        for did in &watch.did {
            (self.say)(What::WatchDid { id: watch.id.clone(), did: did.clone() });
        }
        (self.say)(What::WatchEnded {
            id: watch.id.clone(),
            outcome: outcome.into(),
            said: said.into(),
        });
        state.last = Some(watch.clone());
        state.history.push(watch);
        state.history.truncate(20);
        state.running = None;
        self.shared.set(None);
    }
}

/// The new lines of terminal output, treated as the session's own record of
/// what it did. The original prompt is not an action.
fn actions(before: &[String], output: &[String]) -> Vec<String> {
    output
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !line.starts_with("You are an afkconvoy **watch**"))
        .map(|line| line.chars().take(1000).collect::<String>())
        .filter(|line| !before.contains(line))
        .collect()
}

fn brief(watch: &Watch, earlier: &[String]) -> String {
    let reasons = watch.items.iter().map(|item| format!("- {}: {}", item.id, item.reason));
    let previous: Vec<String> = watch.earlier.iter().chain(earlier).cloned().collect();
    let previous = if previous.is_empty() { "None yet.".into() } else { previous.join("\n") };
    format!(
        "You are an afkconvoy **watch**, the operator on this host. Nobody reads this terminal: never wait for an answer.\n\n\
         A doctor look found something that needs acting on. Find what holds the host's work back, fix it under the same bounds as an inspection, and check that the work goes on. Work only on what the fleets of this host started or own; never stop the host runner or the user's own programs; never delete what cannot be rebuilt; never force-push, push a base branch, or merge outside the landing train. This watch, including any continuation, has one hour from its first start.\n\n\
         ## Why this watch started\n{}\n\n## The doctor look\n```json\n{}\n```\n\n## What earlier sessions of this watch did and left\n{}\n",
        reasons.collect::<Vec<_>>().join("\n"),
        serde_json::to_string_pretty(&watch.look).unwrap_or_default(),
        previous,
    )
}

fn push(items: &mut Vec<Item>, seen: &mut BTreeSet<String>, id: String, reason: String) {
    if seen.insert(id.clone()) {
        items.push(Item::new(id, reason));
    }
}

fn findings(items: &mut Vec<Item>, seen: &mut BTreeSet<String>, at: &str, findings: &Value) {
    for finding in findings.as_array().into_iter().flatten() {
        let what = finding["what"].as_str().unwrap_or("a finding");
        push(
            items,
            seen,
            format!("finding:{at}:{what}"),
            format!("{what}; evidence: {}; fix: {}", finding["evidence"], finding["fix"]),
        );
    }
}

/// Everything in a look an operator would act on, with stable episode ids.
fn actionable(look: &Value, now: u64) -> Vec<Item> {
    let mut items = vec![];
    let mut seen = BTreeSet::new();
    findings(&mut items, &mut seen, "host", &look["findings"]);
    for checkout in look["checkouts"].as_array().into_iter().flatten() {
        let at = checkout["checkout"].as_str().unwrap_or("unknown");
        let doctor = &checkout["doctor"];
        findings(&mut items, &mut seen, at, &doctor["findings"]);
        for waiting in doctor["waiting_on_human"].as_array().into_iter().flatten() {
            let issue = waiting["issue"].as_u64().unwrap_or(0);
            push(
                &mut items,
                &mut seen,
                format!("human:{at}:#{issue}"),
                format!(
                    "#{issue} waits on a human: {}; answer: {}",
                    waiting["why"], waiting["answer"]
                ),
            );
        }
        let fleet = &doctor["fleet"];
        if fleet["alive"] == false {
            push(
                &mut items,
                &mut seen,
                format!("fleet:{at}:not-alive"),
                format!("the fleet instance of {at} is not alive"),
            );
        }
        if !fleet["broken"].is_null() {
            let since = fleet["broken"]["since"].as_u64().unwrap_or(0);
            push(
                &mut items,
                &mut seen,
                format!("fleet:{at}:broken:{since}"),
                format!("the fleet instance of {at} is broken: {}", fleet["broken"]["why"]),
            );
        }
        for tick in doctor["watching"]["ticks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|tick| tick["failed"] == true)
        {
            let first = tick["first"].as_u64().unwrap_or(0);
            push(
                &mut items,
                &mut seen,
                format!("tick:{at}:{}:{first}", tick["instance"].as_str().unwrap_or("unknown")),
                format!(
                    "a tick of {} failed: {}; errors: {}",
                    tick["instance"], tick["line"], tick["errors"]
                ),
            );
        }
        for failed in fleet["failed_steps"].as_array().into_iter().flatten() {
            let tick = failed["tick"].as_u64().unwrap_or(0);
            let step = failed["do"].as_str().unwrap_or("tick");
            push(
                &mut items,
                &mut seen,
                format!("tick:{at}:step:{tick}:{step}"),
                format!("`{step}` failed in tick {tick}: {}", failed["error"]),
            );
        }
    }
    let health = &look["host"]["worker_health"];
    for worker in health["workers"].as_array().into_iter().flatten() {
        let next = worker["next_check"].as_u64().unwrap_or(u64::MAX);
        let path = worker["path"].as_str().unwrap_or("unknown");
        if next <= now {
            push(
                &mut items,
                &mut seen,
                format!("health:{path}:{next}"),
                format!(
                    "the health check of {path} was due at {next} and is still waiting: {}",
                    worker["error"]
                ),
            );
        }
    }
    for failed in health["observations_failed"].as_array().into_iter().flatten() {
        let next = failed[2].as_u64().unwrap_or(u64::MAX);
        let path = failed[0].as_str().unwrap_or("unknown");
        if next <= now {
            push(
                &mut items,
                &mut seen,
                format!("health:{path}:failed:{next}"),
                format!(
                    "the health check of {path} failed and its retry is overdue: {}",
                    failed[1]
                ),
            );
        }
    }
    items
}

/// What the last completed watch did, for doctor.
pub fn last() -> Option<Watch> {
    let bytes = std::fs::read(cache().ok()?.join(STATE)).ok()?;
    serde_json::from_slice::<State>(&bytes).ok()?.last
}

/// Why no harness can start or continue a watch, for doctor.
pub fn unavailable() -> Option<String> {
    let bytes = std::fs::read(cache().ok()?.join(STATE)).ok()?;
    serde_json::from_slice::<State>(&bytes).ok()?.unavailable
}

/// A host finding when a watch is needed but cannot start.
pub fn finding() -> Option<crate::doctor::Finding> {
    let why = unavailable()?;
    Some(crate::doctor::finding(
        crate::doctor::Kind::WatchUnavailable,
        "no harness can start this host's watch".into(),
        why,
        "afkconvoy kit".into(),
    ))
}

/// Append a watch record to every checkout's tick log. A failure of one log
/// costs that log's line, never the watch.
pub fn append_to_tick_logs(list: &List, what: What) {
    let now = crate::clock();
    let entry = Entry { at: now, instance: "host-watch".into(), host: crate::this_host(), what };
    if let Ok(listed) = list.read() {
        for checkout in listed.checkouts {
            if let Ok(repo) = Repo::at(&checkout.checkout)
                && let Ok(log) = crate::log::Log::of(&repo)
            {
                let _ = log.append(&entry);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quiet_look_has_nothing_to_act_on() {
        let look = json!({"findings": [], "checkouts": [{"checkout": "/r", "doctor": {
            "findings": [], "waiting_on_human": [], "fleet": {"alive": true, "broken": null,
            "failed_steps": []}, "watching": {"ticks": []}}}], "host": {"worker_health": {}}});
        assert!(actionable(&look, 1000).is_empty());
    }

    #[test]
    fn every_actionable_kind_has_a_stable_identity() {
        let look = json!({"findings": [{"what": "orca is not running", "evidence": "down", "fix": "orca open"}],
            "host": {"worker_health": {"workers": [{"path": "/w", "next_check": 900, "error": "stuck"}]}},
            "checkouts": [{"checkout": "/r", "doctor": {
            "findings": [], "waiting_on_human": [{"issue": 7, "why": "escalated", "answer": "gh issue view 7"}],
            "fleet": {"alive": false, "broken": {"since": 800, "why": "panic"}, "failed_steps": [{"tick": 9, "do": "dispatch", "error": "no"}]},
            "watching": {"ticks": [{"instance": "alpha", "first": 850, "failed": true, "line": "FAILED", "errors": []}]}}}]});
        let ids: Vec<String> = actionable(&look, 1000).into_iter().map(|item| item.id).collect();
        for prefix in [
            "finding:host:",
            "health:/w:900",
            "human:/r:#7",
            "fleet:/r:not-alive",
            "fleet:/r:broken:800",
            "tick:/r:alpha:850",
            "tick:/r:step:9:dispatch",
        ] {
            assert!(ids.iter().any(|id| id.starts_with(prefix)), "{prefix}: {ids:?}");
        }
        assert_eq!(
            actionable(&look, 899).iter().filter(|item| item.id.starts_with("health:")).count(),
            0
        );
    }

    #[test]
    fn terminal_output_adds_only_new_actions() {
        let output =
            vec!["looked at issue 7".into(), "looked at issue 7".into(), "restarted worker".into()];
        assert_eq!(actions(&["looked at issue 7".into()], &output), ["restarted worker"]);
    }
}
