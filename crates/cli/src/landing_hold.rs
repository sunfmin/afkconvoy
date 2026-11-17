//! An operator's landing hold (docs/adr/0115): one row of the host's
//! database, by the repo's common git dir, which every worktree of the repo
//! on this host shares, so a worker's own `land` and `gate` read the same
//! hold the fleet's tick does. While it stands no turn of a held issue
//! starts and no worker's gate runs; claims, worktrees and branches are
//! kept, and lifting it is all that resumes them.

use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::git::Repo;

/// The note an older release kept the hold in, in the repo's git dir: taken
/// into the host's database as it is read.
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

/// The hold of a note that cannot be read as one: a hold all the same,
/// letting nothing through, as a hold is lifted only by removing it.
fn unreadable() -> Hold {
    Hold { at: 0, issue: None, reason: Some("its note is unreadable".into()) }
}

/// The host's database, with the note an older release kept of `repo`'s
/// hold in its git dir taken in (`host_db::converge_file`), and the key of
/// `repo`'s row of `landing_holds`.
fn converged(repo: &Repo) -> Result<(rusqlite::Connection, String)> {
    let mut db = crate::host_db::open()?;
    let key = crate::host_db::repo_key(repo)?;
    let cache = crate::copies::cache()?;
    crate::host_db::converge_file(&mut db, &repo.note(NOTE)?, &cache, |tx, bytes| {
        let hold = serde_json::from_slice(bytes).unwrap_or_else(|_| unreadable());
        let hold = serde_json::to_string(&hold)?;
        tx.execute(
            "INSERT OR REPLACE INTO landing_holds (repo, hold) VALUES (?1, ?2)",
            rusqlite::params![key, hold],
        )?;
        Ok(())
    })?;
    Ok((db, key))
}

/// The hold standing on `repo` on this host, if any: its row of the host's
/// database, `landing_holds`, by the repo's common git dir, which every
/// worktree of the repo on this host shares. One that cannot be read is a
/// hold all the same, letting nothing through.
pub fn standing(repo: &Repo) -> Option<Hold> {
    let (db, key) = converged(repo).ok()?;
    let text: Option<String> = db
        .query_row("SELECT hold FROM landing_holds WHERE repo = ?1", [&key], |row| row.get(0))
        .optional()
        .ok()?;
    text.map(|text| serde_json::from_str(&text).unwrap_or_else(|_| unreadable()))
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
    let (db, key) = converged(repo)?;
    if lift {
        let lifted = db.execute("DELETE FROM landing_holds WHERE repo = ?1", [&key])? > 0;
        // An older release's note, which its host runner may read still.
        let note = repo.note(NOTE)?;
        let removed = match std::fs::remove_file(&note) {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(error).with_context(|| format!("cannot remove {}", note.display()));
            }
        };
        return Ok(json!({"hold": null, "lifted": lifted || removed}));
    }
    let hold = Hold { at: now, issue, reason };
    db.execute(
        "INSERT OR REPLACE INTO landing_holds (repo, hold) VALUES (?1, ?2)",
        rusqlite::params![key, serde_json::to_string(&hold)?],
    )
    .context("the hold cannot be kept in the host's database")?;
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

One row of the host's database (`afkconvoy help data`), by the repo, which every worktree of the repo on this host shares: the time it was raised, the issue it lets through (`--issue`) and why (`--reason`). Raising it again replaces it; `--lift` removes it. Nothing on GitHub or the remote changes, and it holds this host alone: a peer host's fleet of the same repo is not held. Then it wakes the fleet, as `afkconvoy wake` does.

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

Errors, as {"error": …} with a non-zero exit: run in a worker's worktree; "is not in a git checkout", or git's own words for a checkout it refuses; the host's database cannot be written.

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
    fn an_older_releases_note_is_taken_in_by_repo_and_moved_aside_and_lifting_ends_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let git = |args: &[&str]| {
            let ran =
                crate::process::output(crate::census::command("git").args(args).current_dir(&root))
                    .unwrap();
            assert!(ran.status.success(), "{ran:?}");
        };
        git(&["init", "--quiet", "w"]);
        git(&["-C", "w", "commit", "--quiet", "--allow-empty", "-m", "initial"]);
        git(&["-C", "w", "worktree", "add", "--quiet", "../wt"]);
        let (checkout, worktree) = (Repo::new(root.join("w")), Repo::new(root.join("wt")));
        let note = checkout.note(NOTE).unwrap();
        let old = Hold { at: 5, issue: Some(311), reason: Some("as one".into()) };
        std::fs::write(&note, serde_json::to_string(&old).unwrap()).unwrap();

        assert_eq!(standing(&worktree), Some(old.clone()), "every worktree of the repo reads it");
        assert!(!note.exists(), "moved aside");
        assert!(note.with_file_name(format!("{NOTE}.migrated")).exists());
        assert_eq!(standing(&checkout), Some(old), "a second read takes in nothing");

        // an unreadable note is a hold all the same
        std::fs::write(&note, "{").unwrap();
        assert_eq!(standing(&checkout).unwrap().issue, None);
        assert_eq!(hold(&checkout, None, None, true, 9).unwrap()["lifted"], true);
        assert_eq!(standing(&worktree), None);
        hold(&checkout, Some(7), None, false, 10).unwrap();
        assert_eq!(standing(&worktree).unwrap().issue, Some(7));
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
