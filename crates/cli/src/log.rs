//! The log of a `run`: one line when it starts and one for each tick it made,
//! in a file under this user's cache directory, for `doctor` to read back.
//!
//! It is a log, not state: nothing reads it to decide what the fleet does
//! (fleet state lives on GitHub and the remote), and a host with none loses
//! only what `doctor` could have said of the ticks before.

use std::io::Write;
use std::path::{Path, PathBuf};

use afkconvoy_core::gate::key;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::copies::cache;
use crate::git::Repo;

/// The lines a log is cut back to once it holds twice as many.
const KEPT: usize = 200;

/// One line of the log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// When it was written, in epoch seconds: for a tick, when it ended.
    pub at: u64,
    /// The fleet instance that wrote it, and its host.
    pub instance: String,
    pub host: String,
    #[serde(flatten)]
    pub what: What,
}

/// What a line says.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum What {
    /// A `run` started, its worker launch command beginning with this word:
    /// the word a login shell is asked about, and never what follows it.
    Start { worker_command: String },
    /// One tick ended.
    Tick(Tick),
}

/// One tick, as its `run` saw it end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tick {
    /// Which tick of its `run` it was, from 1.
    pub count: u64,
    /// The seconds slept after it.
    pub sleep: u64,
    /// What `run` printed of it.
    pub line: String,
    /// The claims its pass could hold; `None` of a tick that failed whole.
    pub bound: Option<usize>,
    /// What `afkconvoy tick` answers for it; `None` of a tick that failed whole.
    pub answer: Option<Value>,
    /// Why it failed whole: it could not even rebuild.
    pub error: Option<String>,
}

/// One step that failed in a tick, as the log has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    /// What the step does (`dispatch`, `settle`, …); `tick` of a tick that
    /// failed whole.
    pub step: String,
    pub issue: Option<u64>,
    pub patrol: Option<String>,
    pub error: String,
}

impl Failed {
    /// Whether `other` is the same step on the same thing, whatever it said.
    pub fn same_step(&self, other: &Failed) -> bool {
        (&self.step, self.issue, &self.patrol) == (&other.step, other.issue, &other.patrol)
    }

    /// The step as a human names it: `dispatch #3`, `patrol triage`, `tick`.
    pub fn named(&self) -> String {
        let issue = self.issue.map(|issue| format!(" #{issue}")).unwrap_or_default();
        let patrol = self.patrol.as_ref().map(|patrol| format!(" {patrol}")).unwrap_or_default();
        format!("{}{issue}{patrol}", self.step)
    }
}

/// `failed` with every failure of one cause told once: the first of each
/// `error`, in the order they failed, with how many failed of it.
pub fn by_cause<T>(failed: &[T], error: impl Fn(&T) -> &str) -> Vec<(&T, usize)> {
    let mut causes: Vec<(&T, usize)> = vec![];
    for failure in failed {
        match causes.iter_mut().find(|(first, _)| error(first) == error(failure)) {
            Some((_, count)) => *count += 1,
            None => causes.push((failure, 1)),
        }
    }
    causes
}

impl Tick {
    /// The steps of this tick that failed.
    pub fn failed(&self) -> Vec<Failed> {
        if let Some(error) = &self.error {
            let step = "tick".to_string();
            return vec![Failed { step, issue: None, patrol: None, error: error.clone() }];
        }
        let errors = self.answer.as_ref().and_then(|answer| answer["errors"].as_array());
        let failed = |row: &Value| Failed {
            step: row["do"].as_str().unwrap_or_default().to_string(),
            issue: row["issue"].as_u64(),
            patrol: row["patrol"].as_str().map(str::to_string),
            error: row["error"].as_str().unwrap_or_default().to_string(),
        };
        errors.into_iter().flatten().map(failed).collect()
    }

    /// Whether this tick started a worker on an issue.
    pub fn dispatched(&self) -> bool {
        let did = self.answer.as_ref().and_then(|answer| answer["did"].as_array());
        let started = |step: &Value| step["do"] == "dispatch" && step["outcome"] == "done";
        did.into_iter().flatten().any(started)
    }

    /// The claims held and the frontier left as it ended.
    pub fn stood(&self) -> Option<(u64, u64)> {
        let answer = self.answer.as_ref()?;
        answer["held"].as_u64().zip(answer["frontier_remaining"].as_u64())
    }
}

/// The log of the `run` of one repo on this host.
pub struct Log {
    pub file: PathBuf,
    kept: usize,
}

impl Log {
    /// The log of the repo `repo` is a checkout of: one file for every
    /// worktree of it, named for its main checkout.
    pub fn of(repo: &Repo) -> Result<Log> {
        let main = repo.main_checkout()?;
        let name = main.file_name().map(|name| name.to_string_lossy().into_owned());
        let named =
            format!("{}-{:016x}.jsonl", name.unwrap_or_default(), key(&main.to_string_lossy()));
        Ok(Log::at(&cache()?.join("runs").join(named), KEPT))
    }

    fn at(file: &Path, kept: usize) -> Log {
        Log { file: file.to_path_buf(), kept }
    }

    /// Appends `entry` as one line, written whole or not at all, and cuts
    /// the log back to its newest lines once it holds twice what it keeps.
    /// A line already written is never touched again but to be dropped.
    pub fn append(&self, entry: &Entry) -> Result<()> {
        let said = || format!("cannot write the log {}", self.file.display());
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir).with_context(said)?;
        }
        let line = format!("{}\n", serde_json::to_string(entry)?);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.file)
            .with_context(said)?;
        file.write_all(line.as_bytes()).with_context(said)?;
        drop(file);
        let text = std::fs::read_to_string(&self.file).with_context(said)?;
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() >= self.kept * 2 {
            // Whole or not at all: written beside it, then moved over it.
            let newest = lines[lines.len() - self.kept..].join("\n") + "\n";
            let beside = self.file.with_extension("trimming");
            std::fs::write(&beside, newest).with_context(said)?;
            std::fs::rename(&beside, &self.file).with_context(said)?;
        }
        Ok(())
    }

    /// Every line of the log, oldest first; `None` where there is no log. A
    /// line that cannot be read — one a newer afkconvoy wrote — is passed over.
    pub fn read(&self) -> Option<Vec<Entry>> {
        let text = std::fs::read_to_string(&self.file).ok()?;
        Some(text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tick(count: u64) -> Entry {
        let tick = Tick {
            count,
            sleep: 90,
            line: "nothing to do".into(),
            bound: Some(2),
            answer: Some(json!({"did": [], "errors": [], "held": 0, "frontier_remaining": 0})),
            error: None,
        };
        Entry {
            at: 1000 + count,
            instance: "alpha".into(),
            host: "h1".into(),
            what: What::Tick(tick),
        }
    }

    fn counts(log: &Log) -> Vec<u64> {
        let count = |entry: Entry| match entry.what {
            What::Tick(tick) => tick.count,
            What::Start { .. } => 0,
        };
        log.read().unwrap().into_iter().map(count).collect()
    }

    #[test]
    fn a_line_is_read_back_as_it_was_written() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::at(&dir.path().join("runs/widgets.jsonl"), 5);
        assert_eq!(log.read(), None, "no log yet");
        let start = Entry {
            at: 1000,
            instance: "alpha".into(),
            host: "h1".into(),
            what: What::Start { worker_command: "claude".into() },
        };
        log.append(&start).unwrap();
        log.append(&tick(1)).unwrap();
        assert_eq!(log.read().unwrap(), [start, tick(1)]);
        let text = std::fs::read_to_string(&log.file).unwrap();
        assert_eq!(text.lines().count(), 2, "one line each: {text}");
    }

    #[test]
    fn the_log_is_cut_back_to_its_newest_lines_at_twice_what_it_keeps() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::at(&dir.path().join("widgets.jsonl"), 3);
        for count in 1..=5 {
            log.append(&tick(count)).unwrap();
        }
        assert_eq!(counts(&log), [1, 2, 3, 4, 5]);
        log.append(&tick(6)).unwrap();
        assert_eq!(counts(&log), [4, 5, 6]);
        for count in 7..=40 {
            log.append(&tick(count)).unwrap();
            assert!(counts(&log).len() < 6, "never past its bound");
            assert_eq!(counts(&log).last(), Some(&count), "the newest line is always kept");
        }
    }

    #[test]
    fn a_line_that_cannot_be_read_is_passed_over() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::at(&dir.path().join("widgets.jsonl"), 5);
        log.append(&tick(1)).unwrap();
        let mut text = std::fs::read_to_string(&log.file).unwrap();
        text.push_str("{\"at\": 5, \"cut sho\n");
        std::fs::write(&log.file, text).unwrap();
        log.append(&tick(2)).unwrap();
        assert_eq!(counts(&log), [1, 2]);
    }

    #[test]
    fn one_cause_is_told_once_with_the_steps_it_stopped() {
        let causes = by_cause(&["reset", "no", "reset", "reset"], |error| error);
        assert_eq!(causes, [(&"reset", 3), (&"no", 1)]);
    }

    #[test]
    fn a_tick_says_which_of_its_steps_failed() {
        let mut failed = tick(1);
        let What::Tick(tick) = &mut failed.what else { unreachable!() };
        tick.answer = Some(json!({"did": [{"do": "dispatch", "issue": 5, "outcome": "done"}],
            "errors": [{"do": "dispatch", "issue": 3, "error": "orca worktree create failed"},
                {"do": "patrol", "issue": null, "patrol": "triage", "error": "no"}],
            "held": 2, "frontier_remaining": 1}));
        let steps: Vec<String> = tick.failed().iter().map(Failed::named).collect();
        assert_eq!(steps, ["dispatch #3", "patrol triage"]);
        let failed = tick.failed();
        let causes = by_cause(&failed, |failed| &failed.error);
        assert_eq!(causes.len(), 2, "two causes: {causes:?}");
        assert!(tick.dispatched());
        assert_eq!(tick.stood(), Some((2, 1)));
        // a tick that could not rebuild failed whole
        tick.answer = None;
        tick.error = Some("gh pr list failed".into());
        assert_eq!(tick.failed()[0].named(), "tick");
        assert!(!tick.dispatched());
    }
}
