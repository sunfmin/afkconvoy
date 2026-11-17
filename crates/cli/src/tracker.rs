//! A checkout's tracker: the repo its fleet reads and writes its issues in,
//! or its host's local tracker (docs/adr/0091), said on its host as its
//! concurrency is (docs/adr/0027) — kept with its entry on the checkout list,
//! set by a `run` that names it, and read as each tick opens the fleet, so a
//! change holds at the next tick. Until its checkout says one, it is its
//! host's local tracker (`Said::DEFAULT`); the checkout's own repo is its
//! tracker only where it names it.
//!
//! A checkout the list holds no entry of yet goes by what its `run` asked,
//! kept in that process alone until its first tick puts it on the list with
//! it (`checkout_list::Asked`).

use std::collections::BTreeMap;
use std::sync::Mutex;

use afkconvoy_core::config::{Config, RepoName};
use afkconvoy_core::tracker::{LOCAL, Said, names_a_ref, repo_label, says_private};
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::checkout_list::{List, asked, named_repo, said_tracker, tracker_for};
use crate::claims::Scan;
use crate::doctor::{Finding, Kind, finding};
use crate::flows::{READY_LABEL_SAYS, create_missing};
use crate::gh::GitHub;
use crate::git::Repo;
use crate::host_settings::To;
use crate::legacy::Legacy;
use crate::probe::refused_public;
use crate::topic::See;

/// What the repo label says of itself in the tracker.
pub(crate) const REPO_LABEL_SAYS: &str =
    "afkconvoy: an issue of the repo this names, which its fleet works";
/// What the escalate label says of itself.
const ESCALATE_LABEL_SAYS: &str =
    "afkconvoy: the fleet handed an issue that carries this to a human";
/// What the lock label says of itself.
const LOCK_LABEL_SAYS: &str =
    "afkconvoy: while an open issue carries this, the fleet works lock issues only";

/// `--tracker` as said: a repo, `<owner>/<repo>` — the checkout's own among
/// them — the host's local tracker, `local`, or `default`, which is the
/// local tracker too.
pub fn repo_or_default(said: &str) -> Result<To<Said>, String> {
    To::parse(said, |said| {
        Said::read(said).ok_or_else(|| {
            format!("`{said}` is no repo, `<owner>/<repo>`, nor `default`, nor `{LOCAL}`")
        })
    })
}

/// `--tracker` as said, where it takes a repo alone: `<owner>/<repo>`.
pub fn repo_named(said: &str) -> Result<RepoName, String> {
    RepoName::read(said).ok_or_else(|| format!("`{said}` is no repo, `<owner>/<repo>`"))
}

/// The tracker the checkout `repo` says on the host of `list`, as `checkouts`,
/// `doctor` and `plan` say it: the name of the repo it names, or of the
/// host's local tracker (`local/<host>`), which it is where the checkout
/// says none; null where it is the checkout's own repo; {error} where the
/// list cannot be read.
pub fn json_of(list: &List, repo: &Repo) -> Value {
    match said_tracker(list, repo).and_then(|said| tracker_for(said, repo)) {
        Ok(tracker) => json!(tracker.named()),
        Err(error) => json!({"error": format!("{error:#}")}),
    }
}

/// The same, of the checkout whose entry is `entry`, `repo` where it is a
/// git checkout still: where it is none, the name of what the entry says.
pub fn json_of_entry(entry: &crate::checkout_list::Entry, repo: Option<&Repo>) -> Value {
    let said = entry.tracker.clone();
    match repo {
        Some(repo) => match tracker_for(said, repo) {
            Ok(tracker) => json!(tracker.named()),
            Err(error) => json!({"error": format!("{error:#}")}),
        },
        None => json!(Said::or_default(said).name(&crate::host_settings::host_name())),
    }
}

/// Has the checkout `repo`, whose fleet stamps its claims with `host`, say
/// `to` of its tracker on the host of `list` (`check`): with its entry,
/// where the list holds one; where it holds none yet, as what its `run`
/// asked, in this process → what that changed, as `check` says it.
pub fn say(list: &List, repo: &Repo, to: To<Said>, host: &str) -> Result<Value> {
    let Checked { tracker, changed } = check(list, repo, to, host)?;
    list.change(|listed| {
        let root = &repo.dir;
        match listed.checkouts.iter_mut().find(|entry| entry.checkout == *root) {
            Some(entry) => entry.tracker = tracker,
            None => asked().entry(root.clone()).or_default().tracker = tracker,
        }
    })?;
    Ok(changed)
}

/// Has the checkout `repo` ask `to` of its tracker in this process alone, as
/// `run --foreground` does, which leaves the checkout list alone: what its
/// entry says, where the list holds one, holds over it.
pub fn ask(list: &List, repo: &Repo, to: To<Said>, host: &str) -> Result<Value> {
    let Checked { tracker, changed } = check(list, repo, to, host)?;
    asked().entry(repo.dir.clone()).or_default().tracker = tracker;
    Ok(changed)
}

/// A tracker a checkout says, checked (`check`) and not kept yet: the one it
/// names, none where it says the default, and what saying it changes, as
/// `check` says it.
pub struct Checked {
    tracker: Option<Said>,
    changed: Value,
}

impl Checked {
    /// Keeps it with the entry of the checkout `repo` on `list` → what that
    /// changed; `None` where the list holds no entry of it, and nothing is
    /// kept.
    pub fn kept(self, list: &List, repo: &Repo) -> Result<Option<Value>> {
        let Checked { tracker, changed } = self;
        let kept = list.change(|listed| {
            let entry = listed.checkouts.iter_mut().find(|entry| entry.checkout == repo.dir)?;
            entry.tracker = tracker;
            Some(())
        })?;
        Ok(kept.map(|()| changed))
    }
}

/// What the checkout `repo` says of its tracker once it says `to` → the
/// tracker it then names — a repo, its own among them, or `local` — none
/// where it says the default, the host's local tracker, and what that
/// changes, as {from, to}, each the tracker said, or null for the default;
/// null where it changes nothing.
/// A tracker GitHub says is public is refused before anything is made in it,
/// whether it changes or not, the checkout's own among them
/// (`probe::refused_public`); the host's local tracker is no GitHub repo,
/// and is written by the host's own user alone (docs/adr/0091). A tracker
/// another repo than the checkout's is given the labels its fleet's issues
/// carry there where it lacks them — the repo label, and the ready, escalate
/// and lock labels of the repo's config; the local tracker makes no label.
/// A change is refused while the checkout's fleet holds a claim on an issue
/// of the tracker it had, or the landing train carries a PR of one: their
/// issues would be read in a tracker that does not hold them. Nothing is
/// kept: `say`, `ask` and `Checked::kept` keep it.
pub fn check(list: &List, repo: &Repo, to: To<Said>, host: &str) -> Result<Checked> {
    let said = |said: &Option<Said>| said.as_ref().map(|said| said.to_string().to_lowercase());
    let tracker = match to.kept() {
        Some(Said::Repo(named)) if !names_a_ref(&named) => bail!(
            "{named} cannot name a git ref, as the fleet's claims on its issues are named: no \
             part of a tracker's name may begin with `.` or end with `.` or `.lock`"
        ),
        kept => kept,
    };
    let was = said_tracker(list, repo)?;
    let config = crate::config::of(repo)?;
    let gh = GitHub::new(repo.dir.clone(), tracker_for(tracker.clone(), repo)?);
    if let Some(why) = refused_public(repo, &gh)? {
        return Err(See::Troubleshooting.refusal(why));
    }
    if gh.tracker.repo_label().is_some() {
        labelled(&gh, repo, &config)?;
    }
    if said(&was) == said(&tracker) {
        return Ok(Checked { tracker, changed: Value::Null });
    }
    let held = held(repo, &config, host)?;
    if !held.is_empty() {
        let from = Said::or_default(was.clone()).to_string();
        bail!(
            "{} works the issues of {from} still: {}. Run this again once they are done, so \
             that none of them is read in a tracker that does not hold it",
            repo.dir.display(),
            held.join("; ")
        );
    }
    let changed = json!({"from": was, "to": tracker});
    Ok(Checked { tracker, changed })
}

/// Makes in the tracker `gh` names, another repo than the checkout `repo`'s,
/// each label the checkout's fleet puts on its issues there that it lacks.
fn labelled(gh: &GitHub, repo: &Repo, config: &Config) -> Result<()> {
    let issues = crate::issues::of(gh);
    let label = repo_label(&named_repo(repo)?);
    create_missing(&*issues, &[label], REPO_LABEL_SAYS)?;
    create_missing(&*issues, std::slice::from_ref(&config.ready_label), READY_LABEL_SAYS)?;
    create_missing(&*issues, std::slice::from_ref(&config.escalate_label), ESCALATE_LABEL_SAYS)?;
    create_missing(&*issues, std::slice::from_ref(&config.lock_label), LOCK_LABEL_SAYS)?;
    Ok(())
}

/// Tracker visibility observations shared by one look, keyed by tracker.
#[derive(Default)]
pub struct Public {
    why: Mutex<BTreeMap<String, Option<String>>>,
}

impl Public {
    /// What `doctor` finds of the tracker of the checkout `repo`: that GitHub
    /// says it is public, and its fleet works nothing — none where GitHub cannot
    /// be asked, which `doctor` finds otherwise. Each tracker is asked once.
    pub fn findings(&self, repo: &Repo) -> Vec<Finding> {
        let Ok(gh) = GitHub::of(repo) else { return vec![] };
        let named = crate::probe::named(repo, &gh.tracker);
        let mut observed = self.why.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let why = observed
            .entry(named.clone())
            .or_insert_with(|| refused_public(repo, &gh).ok().flatten())
            .clone();
        drop(observed);
        let Some(why) = why else { return vec![] };
        let checkout = repo.dir.display().to_string();
        vec![finding(
            Kind::PublicTracker,
            serde_json::json!(["public-tracker", named]),
            format!(
                "this checkout's tracker, {named}, is public: its fleet instance works nothing, as \
                 anyone can comment on its issues"
            ),
            why,
            says_private(&checkout),
        )]
    }
}

/// What the fleet of the checkout `repo` holds of the tracker it says now:
/// its claims on that tracker's issues, those stamped with `host`, and the
/// PRs of that tracker's issues on the base branch — each said.
fn held(repo: &Repo, config: &Config, host: &str) -> Result<Vec<String>> {
    let scan = Scan::read(repo)?;
    let issue = |number: u64| scan.tracker.issue(number);
    let mine =
        scan.claims.iter().filter(|claim| claim.owner.as_ref().is_some_and(|o| o.host == host));
    let claims: Vec<String> = mine.map(|claim| issue(claim.number)).collect();
    let mut held = vec![];
    if !claims.is_empty() {
        held.push(format!("it holds the claims on {}", claims.join(", ")));
    }
    let joins = Legacy::read(repo, config)?.joins(repo, &scan.tracker)?;
    let riding: Vec<String> = joins
        .iter()
        .filter_map(|join| {
            join.issue().map(|number| format!("PR #{} of {}", join.pr, issue(number)))
        })
        .collect();
    if !riding.is_empty() {
        held.push(format!("the landing train carries {}", riding.join(", ")));
    }
    Ok(held)
}
