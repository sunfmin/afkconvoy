//! A checkout's tracker: the repo its fleet reads and writes its issues in,
//! said on its host as its concurrency is (docs/adr/0027) — kept with its
//! entry on the checkout list, set by a `run` that names it, and read as each
//! tick opens the fleet, so a change holds at the next tick. Until its
//! checkout says one, it is the checkout's own repo.
//!
//! A checkout the list holds no entry of yet goes by what its `run` asked,
//! kept in that process alone until its first tick puts it on the list with
//! it (`checkout_list::Asked`).

use afkconvoy_core::config::{Config, RepoName};
use afkconvoy_core::tracker::{names_a_ref, repo_label, says_private};
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::checkout_list::{List, asked, named_repo, said_tracker};
use crate::claims::Scan;
use crate::doctor::{Finding, Kind, finding};
use crate::flows::{READY_LABEL_SAYS, create_missing};
use crate::gh::{GitHub, Tracker};
use crate::git::Repo;
use crate::host_settings::To;
use crate::probe::refused_public;
use crate::topic::See;
use crate::train::Train;

/// What the repo label says of itself in the tracker.
const REPO_LABEL_SAYS: &str = "afkconvoy: an issue of the repo this names, which its fleet works";
/// What the escalate label says of itself.
const ESCALATE_LABEL_SAYS: &str =
    "afkconvoy: the fleet handed an issue that carries this to a human";
/// What the lock label says of itself.
const LOCK_LABEL_SAYS: &str =
    "afkconvoy: while an open issue carries this, the fleet works lock issues only";

/// `--tracker` as said: a repo, `<owner>/<repo>`, or `default`.
pub fn repo_or_default(said: &str) -> Result<To<RepoName>, String> {
    To::parse(said, |said| {
        RepoName::read(said)
            .ok_or_else(|| format!("`{said}` is no repo, `<owner>/<repo>`, nor `default`"))
    })
}

/// The tracker the checkout `repo` says on the host of `list`, as `checkouts`,
/// `doctor` and `plan` say it: the repo it names, or null where it names
/// none and the checkout's own repo is its tracker; {error} where the list
/// cannot be read.
pub fn json_of(list: &List, repo: &Repo) -> Value {
    match said_tracker(list, repo) {
        Ok(said) => json!(said),
        Err(error) => json!({"error": format!("{error:#}")}),
    }
}

/// Has the checkout `repo`, whose fleet stamps its claims with `host`, say
/// `to` of its tracker on the host of `list` (`checked`): with its entry,
/// where the list holds one; where it holds none yet, as what its `run`
/// asked, in this process → what that changed, as `checked` says it.
pub fn say(list: &List, repo: &Repo, to: To<RepoName>, host: &str) -> Result<Value> {
    let (tracker, changed) = checked(list, repo, to, host)?;
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
pub fn ask(list: &List, repo: &Repo, to: To<RepoName>, host: &str) -> Result<Value> {
    let (tracker, changed) = checked(list, repo, to, host)?;
    asked().entry(repo.dir.clone()).or_default().tracker = tracker;
    Ok(changed)
}

/// What the checkout `repo` says of its tracker once it says `to` → the
/// tracker it then names, none where that is the checkout's own repo, and
/// what that changes, as {from, to}, each the repo named or null for the
/// checkout's own; null where it changes nothing. A tracker GitHub says is
/// public is refused before anything is made in it, whether it changes
/// or not, the checkout's own among them (`probe::refused_public`). A tracker another repo
/// than the checkout's is given the labels its fleet's issues carry there
/// where it lacks them — the repo label, and the ready, escalate and lock
/// labels of the repo's config. A change is refused while the checkout's
/// fleet holds a claim on an issue of the tracker it had, or the landing
/// train carries a PR of one: their issues would be read in a tracker that
/// does not hold them.
fn checked(
    list: &List,
    repo: &Repo,
    to: To<RepoName>,
    host: &str,
) -> Result<(Option<RepoName>, Value)> {
    let said = |name: &Option<RepoName>| name.as_ref().map(|name| name.as_str().to_lowercase());
    let tracker = match to.kept() {
        Some(named) if !named.as_str().eq_ignore_ascii_case(named_repo(repo)?.as_str()) => {
            if !names_a_ref(&named) {
                bail!(
                    "{named} cannot name a git ref, as the fleet's claims on its issues are \
                     named: no part of a tracker's name may begin with `.` or end with `.` or \
                     `.lock`"
                );
            }
            Some(named)
        }
        _ => None,
    };
    let was = said_tracker(list, repo)?;
    let config = crate::concurrency::config_of(repo)?;
    let gh = GitHub {
        dir: repo.dir.clone(),
        tracker: match &tracker {
            Some(named) => Tracker::Other { tracker: named.clone(), repo: named_repo(repo)? },
            None => Tracker::Own,
        },
    };
    if let Some(why) = refused_public(repo, &gh)? {
        return Err(See::Troubleshooting.refusal(why));
    }
    if tracker.is_some() {
        labelled(&gh, repo, &config)?;
    }
    if said(&was) == said(&tracker) {
        return Ok((tracker, Value::Null));
    }
    let held = held(repo, &config, host)?;
    if !held.is_empty() {
        let from = was.as_ref().map_or("its own repo", RepoName::as_str);
        bail!(
            "{} works the issues of {from} still: {}. Run this again once they are done, so \
             that none of them is read in a tracker that does not hold it",
            repo.dir.display(),
            held.join("; ")
        );
    }
    Ok((tracker.clone(), json!({"from": was, "to": tracker})))
}

/// Makes in the tracker `gh` names, another repo than the checkout `repo`'s,
/// each label the checkout's fleet puts on its issues there that it lacks.
fn labelled(gh: &GitHub, repo: &Repo, config: &Config) -> Result<()> {
    let label = repo_label(&named_repo(repo)?);
    create_missing(gh, &[label], REPO_LABEL_SAYS)?;
    create_missing(gh, std::slice::from_ref(&config.ready_label), READY_LABEL_SAYS)?;
    create_missing(gh, std::slice::from_ref(&config.escalate_label), ESCALATE_LABEL_SAYS)?;
    create_missing(gh, std::slice::from_ref(&config.lock_label), LOCK_LABEL_SAYS)?;
    Ok(())
}

/// What `doctor` finds of the tracker of the checkout `repo`: that GitHub
/// says it is public, and its fleet works nothing — none where GitHub cannot
/// be asked, which `doctor` finds otherwise.
pub fn findings(repo: &Repo) -> Vec<Finding> {
    let Ok(gh) = GitHub::of(repo) else { return vec![] };
    let Ok(Some(why)) = refused_public(repo, &gh) else { return vec![] };
    let checkout = repo.dir.display().to_string();
    let named = crate::probe::named(repo, &gh.tracker);
    vec![finding(
        Kind::PublicTracker,
        format!(
            "this checkout's tracker, {named}, is public: its fleet instance works nothing, as \
             anyone can comment on its issues"
        ),
        why,
        says_private(&checkout),
    )]
}

/// What the fleet of the checkout `repo` holds of the tracker it says now:
/// its claims on that tracker's issues, those stamped with `host`, and the
/// PRs of that tracker's issues on the landing train — each said.
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
    let joins = Train::read(repo, config)?.joins(repo, &scan.tracker)?;
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
