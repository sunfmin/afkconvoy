//! The harnesses paused on this host (docs/adr/0020): a harness the fleet
//! finds refusing every prompt (`harness_down`) is left out of the host kit,
//! and so of every range, until a probe finds it working. Every hour each one
//! is probed; one that answers is back. This is the host's running state, in
//! its cache, shared by every fleet on it: the host kit's file is not changed.

use std::path::PathBuf;

use afkconvoy_core::casting::Paused;
use afkconvoy_core::harness::Harness;
use anyhow::Result;

use crate::launch::Answer;

/// Why a worker is not started: the harness it would run is paused, or every
/// harness of the host kit is. What waits for it is held back, not failed.
#[derive(Debug)]
pub struct Held(pub String);

impl std::fmt::Display for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Held {}

impl Held {
    /// A worker that would run `pause`'s harness.
    pub fn of(pause: &Paused, now: u64) -> Held {
        Held(format!("held while its harness is paused: {}", said(pause, now)))
    }

    /// A worker of a host kit every harness of which is `paused`.
    pub fn every(paused: &[Paused], now: u64) -> Held {
        let each: Vec<String> = paused.iter().map(|pause| said(pause, now)).collect();
        Held(format!("held while every harness of the host kit is paused: {}", each.join("; ")))
    }
}

/// A pause as a human is told it: `` `pi` is paused on this host since …: why;
/// it is probed every hour ``.
pub fn said(pause: &Paused, now: u64) -> String {
    format!(
        "`{}` is paused on this host {}: {}; it is probed every hour, and is back once it answers",
        pause.harness.key(),
        crate::tick::waiting_since(pause.since, now),
        pause.why
    )
}

/// Where this host keeps its pauses: in its cache, outside every checkout.
#[derive(Debug, Clone)]
pub struct Pauses {
    file: PathBuf,
}

impl Pauses {
    /// This host's, whichever repo a command is about.
    pub fn of_host() -> Result<Pauses> {
        Ok(Pauses { file: crate::copies::cache()?.join("paused.json") })
    }

    /// Kept in `file`.
    pub fn at(file: PathBuf) -> Pauses {
        Pauses { file }
    }

    /// The harnesses paused, in the order they were paused.
    pub fn read(&self) -> Result<Vec<Paused>> {
        match std::fs::read(&self.file) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(e) => Err(e.into()),
        }
    }

    fn write(&self, paused: &[Paused]) -> Result<()> {
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = self.file.with_extension(format!("{}.tmp", crate::unique()));
        std::fs::write(&temporary, serde_json::to_vec_pretty(paused)?)?;
        Ok(std::fs::rename(temporary, &self.file)?)
    }

    /// The pause of `harness`, where it is paused.
    pub fn of(&self, harness: Harness) -> Result<Option<Paused>> {
        Ok(self.read()?.into_iter().find(|pause| pause.harness == harness))
    }

    /// Pauses `harness` `now`, for `why` → whether it was not paused
    /// already: one already paused keeps its time and its reason.
    pub fn pause(&self, harness: Harness, now: u64, why: &str) -> Result<bool> {
        let mut paused = self.read()?;
        if paused.iter().any(|pause| pause.harness == harness) {
            return Ok(false);
        }
        paused.push(Paused { harness, since: now, why: why.to_string(), probed: None });
        self.write(&paused)?;
        Ok(true)
    }

    /// Puts `harness` back in the host kit → whether it was paused.
    pub fn resume(&self, harness: Harness) -> Result<bool> {
        let mut paused = self.read()?;
        let before = paused.len();
        paused.retain(|pause| pause.harness != harness);
        if paused.len() == before {
            return Ok(false);
        }
        self.write(&paused)?;
        Ok(true)
    }

    /// Probes each harness paused that is due `now` (`Paused::due`), with
    /// `probe` given its worker launch command and first model in `kit`, the
    /// host kit's file → each probed. One that answers is back in the host
    /// kit; one `kit` no longer holds is forgotten.
    pub fn probe_due(
        &self,
        now: u64,
        kit: &crate::kit::Kit,
        probe: impl Fn(Harness, &str, Option<&str>) -> Answer,
    ) -> Result<Vec<Probed>> {
        let due: Vec<Paused> = self.read()?.into_iter().filter(|pause| pause.due(now)).collect();
        let mut probed = vec![];
        for pause in due {
            let held = kit.harnesses.iter().find(|held| held.harness == pause.harness);
            let answer = held.map(|held| {
                probe(
                    held.harness,
                    &held.command,
                    held.placed().next().map(|seen| seen.model.as_str()),
                )
            });
            let resumed = matches!(answer, None | Some(Answer::Answered(..)));
            let unasked = match answer {
                Some(Answer::Unasked(why)) => Some(why),
                _ => None,
            };
            if resumed {
                self.resume(pause.harness)?;
            } else {
                let mut paused = self.read()?;
                for kept in paused.iter_mut().filter(|kept| kept.harness == pause.harness) {
                    kept.probed = Some(now);
                }
                self.write(&paused)?;
            }
            probed.push(Probed { harness: pause.harness, resumed, unasked });
        }
        Ok(probed)
    }
}

/// What one probe of a paused harness came to.
pub struct Probed {
    pub harness: Harness,
    /// It is no longer paused.
    pub resumed: bool,
    /// Why it could not be asked, when no terminal could be made for its
    /// probe.
    pub unasked: Option<String>,
}

/// Probes the harnesses paused on this host that are due `now`, each
/// through its worker launch command in the login shell a worker gets
/// (`launch::probe`), and appends a line to the tick log of `repo` for each:
/// back in the host kit, or still paused. A `tick` of `instance` on `host`
/// does it before anything else.
pub fn probe(repo: &crate::git::Repo, instance: &str, host: &str, now: u64) -> Result<()> {
    let pauses = Pauses::of_host()?;
    if !pauses.read()?.iter().any(|pause| pause.due(now)) {
        return Ok(());
    }
    let kit = crate::kit::load()?;
    for probed in pauses.probe_due(now, &kit, crate::launch::probe)? {
        let outcome = match (probed.resumed, &probed.unasked) {
            (true, _) => "it answered its probe: back in the host kit".to_string(),
            (false, Some(why)) => format!(
                "its probe could not be asked ({why}): still paused, and probed again in an hour"
            ),
            (false, None) => {
                "it did not answer its probe: still paused, and probed again in an hour".to_string()
            }
        };
        let entry = crate::log::Entry {
            at: now,
            instance: instance.to_string(),
            host: host.to_string(),
            what: crate::log::What::Command {
                command: "probe_paused".into(),
                on: Some(probed.harness.key().into()),
                by: "fleet".into(),
                outcome,
                failed: false,
            },
        };
        crate::log::Log::of(repo)?.append(&entry)?;
    }
    Ok(())
}

/// What `doctor` finds of the harnesses paused on this host `now`: one
/// finding each, with since when and why.
pub fn findings(now: u64) -> Vec<crate::doctor::Finding> {
    read()
        .iter()
        .map(|pause| {
            crate::doctor::finding(
                crate::doctor::Kind::HarnessPaused,
                format!("the harness `{}` is paused on this host", pause.harness.key()),
                format!("{}; no worker is started on it until then", said(pause, now)),
                "afkconvoy kit".to_string(),
            )
        })
        .collect()
}

/// This host's paused harnesses; none where they cannot be read.
pub fn read() -> Vec<Paused> {
    Pauses::of_host().and_then(|pauses| pauses.read()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pause_keeps_its_first_reason_and_a_due_probe_that_answers_ends_it() {
        let dir = tempfile::tempdir().unwrap();
        let pauses = Pauses::at(dir.path().join("cache/paused.json"));
        let kit: crate::kit::Kit = serde_json::from_str(
            r#"{"harnesses":[{"harness":"pi","command":"pi","models":[{"model":"a/b"}]},
                {"harness":"codex","command":"codex"}]}"#,
        )
        .unwrap();
        assert!(pauses.pause(Harness::Pi, 100, "refused").unwrap());
        assert!(!pauses.pause(Harness::Pi, 200, "another").unwrap());
        assert!(pauses.pause(Harness::Codex, 100, "refused too").unwrap());
        assert_eq!(pauses.of(Harness::Pi).unwrap().unwrap().why, "refused");
        let silent = |_: Harness, _: &str, _: Option<&str>| Answer::Silent;
        assert!(pauses.probe_due(3699, &kit, silent).unwrap().is_empty(), "within the hour");
        let asked = std::cell::RefCell::new(vec![]);
        let pi_answers = |harness: Harness, command: &str, model: Option<&str>| {
            asked.borrow_mut().push((harness, command.to_string(), model.map(str::to_string)));
            match harness {
                Harness::Pi => Answer::Answered(1.0, None),
                _ => Answer::Unasked("orca terminal create failed".into()),
            }
        };
        let probed = pauses.probe_due(3700, &kit, pi_answers).unwrap();
        assert_eq!(probed.iter().filter(|probed| probed.resumed).count(), 1);
        let unasked = probed.iter().find(|probed| probed.harness == Harness::Codex).unwrap();
        assert_eq!(unasked.unasked.as_deref(), Some("orca terminal create failed"), "says why");
        assert_eq!(
            *asked.borrow(),
            [
                (Harness::Pi, "pi".to_string(), Some("a/b".to_string())),
                (Harness::Codex, "codex".to_string(), None)
            ]
        );
        assert_eq!(pauses.of(Harness::Pi).unwrap(), None);
        assert_eq!(pauses.of(Harness::Codex).unwrap().unwrap().probed, Some(3700));
        assert!(pauses.probe_due(7299, &kit, silent).unwrap().is_empty(), "after its last probe");
        assert_eq!(pauses.probe_due(7300, &kit, silent).unwrap().len(), 1);
        assert!(pauses.resume(Harness::Codex).unwrap());
        assert!(!pauses.resume(Harness::Codex).unwrap());
        assert!(pauses.read().unwrap().is_empty());
    }
}
