//! An operator's landing hold (docs/adr/0115): one note in the repo's git
//! dir, which every worktree of the repo on this host shares, so a worker's
//! own `land` and `gate` read the same hold the fleet's tick does. While it
//! stands no turn of a held issue starts and no worker's gate runs; claims,
//! worktrees and branches are kept, and lifting it is all that resumes them.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::git::Repo;

/// The note's name in the repo's git dir.
pub const NOTE: &str = "convoy-landing-hold";

/// A landing hold standing on this host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    /// When it was raised, in seconds since the epoch.
    pub at: u64,
    /// The one issue whose turn it lets through — the PR it is raised
    /// for to land alone — if any. A watch's `land --watch` is always let through.
    pub issue: Option<u64>,
    /// Why, as whoever raised it said.
    pub reason: Option<String>,
}

impl Hold {
    /// Whether it keeps the work of `issue` off: every issue but the one it
    /// lets through.
    pub fn keeps_off(&self, issue: u64) -> bool {
        self.issue != Some(issue)
    }

    /// The line that names it, as every refusal says it.
    pub fn line(&self) -> String {
        let lets = self.issue.map(|n| format!(", letting #{n} through")).unwrap_or_default();
        let why = self.reason.as_deref().map(|why| format!(": {why}")).unwrap_or_default();
        format!(
            "an operator's landing hold stands on this host since {}{lets}{why} — `afkconvoy hold \
             --lift` lifts it",
            self.at
        )
    }
}

/// The hold standing on `repo` on this host, if any. A note that cannot be
/// read as one is a hold all the same, letting nothing through: a hold is
/// lifted only by removing it.
pub fn standing(repo: &Repo) -> Option<Hold> {
    let file = repo.note(NOTE).ok()?;
    let text = std::fs::read_to_string(&file).ok()?;
    Some(serde_json::from_str(&text).unwrap_or(Hold {
        at: 0,
        issue: None,
        reason: Some("its note is unreadable".into()),
    }))
}

/// The hold standing that keeps `issue` off, if any.
pub fn keeping_off(repo: &Repo, issue: u64) -> Option<Hold> {
    standing(repo).filter(|hold| hold.keeps_off(issue))
}

/// The hold as a JSON value: null where none stands.
pub fn json(repo: &Repo) -> Value {
    json!(standing(repo))
}

/// `afkconvoy hold`: raise the hold (`lift` false), replacing one standing,
/// or lift it.
///
///   {"hold": {"at", "issue", "reason"} | null, "lifted": bool}
pub fn hold(
    repo: &Repo,
    issue: Option<u64>,
    reason: Option<String>,
    lift: bool,
    now: u64,
) -> Result<Value> {
    let file = repo.note(NOTE)?;
    if lift {
        let lifted = match std::fs::remove_file(&file) {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(error).with_context(|| format!("cannot remove {}", file.display()));
            }
        };
        return Ok(json!({"hold": null, "lifted": lifted}));
    }
    let hold = Hold { at: now, issue, reason };
    // Written whole, then moved into place: a reader never sees half a note.
    let part = file.with_extension(format!("{}.part", std::process::id()));
    std::fs::write(&part, serde_json::to_string(&hold)?)
        .with_context(|| format!("cannot write {}", part.display()))?;
    std::fs::rename(&part, &file).with_context(|| format!("cannot write {}", file.display()))?;
    Ok(json!({"hold": hold, "lifted": false}))
}

/// What `afkconvoy hold --help` says after its one line, which lib.rs's
/// `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

When one PR has to land alone — many PRs merged into one, landed by a watch or by one issue's turn — and the fleet's running workers must not take landing turns or run gates beside it. Raise the hold, land that PR, lift it:

```
afkconvoy hold --issue 311 --reason "28 PRs landing as one"
afkconvoy land --watch            # or the turn of #311
afkconvoy hold --lift
```

## Who runs it

The operator, or the host's watch, in the repo's checkout on this host. Not a worker.

## What it reads and writes

One note in the repo's git dir (`convoy-landing-hold`), which every worktree of the repo on this host shares: the time it was raised, the issue it lets through (`--issue`) and why (`--reason`). Raising it again replaces it; `--lift` removes it. Nothing on GitHub or the remote changes, and it holds this host alone: a peer host's fleet of the same repo is not held. Then it wakes the fleet, as `afkconvoy wake` does.

While it stands:

- `afkconvoy land --issue <n>` answers `held`, naming the hold, for every issue but the one it lets through: no turn starts, nothing is merged or gated.
- `afkconvoy gate` — and `--fast`, `--slow`, `--land` — run by a worker (AFKCONVOY_ISSUE in its environment) is refused with an error naming the hold, but for the issue it lets through. A gate the operator or the watch runs by hand still runs.
- `afkconvoy land --watch` is let through: a watch's PR lands.
- The tick asks for no turn of a held issue. Claims stay held, worktrees and branches are kept, and nothing is released or ended.
- `afkconvoy plan` and `afkconvoy doctor` show it as `landing_hold`, and `doctor` finds `a landing hold stands` in place of `its fleet lands nothing`.

Once it is lifted, the next tick asks again for every turn it held, with no worker restarted; a worker its gate refused is nudged on as any stopped worker is.

## What it answers

{"hold", "lifted", "woke"}

- `hold`: the hold standing now — {"at", "issue", "reason"} — or null once lifted
- `lifted`: true when `--lift` removed a hold that stood
- `woke`: whether a `run` on this host was told, as `afkconvoy wake` answers

Errors, as {"error": …} with a non-zero exit: run in a worker's worktree; "is not in a git checkout", or git's own words for a checkout it refuses; the note cannot be written.

## What trips you up

- `afkconvoy stop` stops the fleet instance only: its workers go on running, and land.
- `afkconvoy stop --release` ends the workers and releases their claims: their work is left for another start to take up.
- `afkconvoy lock --issue <n>` raises a fleet lock on GitHub, for every host: it changes what the fleet dispatches and which PR lands, but a running worker still gates.
- `afkconvoy hold` pauses this host's landings and workers' gates, keeping every claim and worktree, until it is lifted.
- It holds this host alone: a peer host's fleet of the same repo still lands, and a fleet lock is what holds every host.
- A worker its gate refused stops; the tick does not nudge it while the hold stands, and takes it up as any stopped worker once the hold is lifted.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hold_lets_only_its_issue_through() {
        let hold = Hold { at: 1, issue: Some(311), reason: None };
        assert!(!hold.keeps_off(311));
        assert!(hold.keeps_off(7));
        let all = Hold { issue: None, ..hold };
        assert!(all.keeps_off(311));
    }

    #[test]
    fn its_line_names_it_and_how_to_lift_it() {
        let hold = Hold { at: 9, issue: Some(311), reason: Some("28 as one".into()) };
        let line = hold.line();
        assert!(line.contains("landing hold"), "{line}");
        assert!(line.contains("#311"), "{line}");
        assert!(line.contains("28 as one"), "{line}");
        assert!(line.contains("afkconvoy hold --lift"), "{line}");
    }
}
