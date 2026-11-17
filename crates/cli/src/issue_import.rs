//! `afkconvoy issue import --from <owner>/<tracker>`: moves a GitHub
//! tracker's open issues into this host's local tracker (docs/adr/0091,
//! "Taking the issues off GitHub").
//!
//! The issues it moves are those of the repos this host has a checkout of —
//! an issue's repo is its repo label, or the tracker itself for one that
//! carries none in its own repo — claimed by this host or by no host. Each is
//! kept in the local tracker whole, in one write (`Store::import`), named by
//! where it was (`<owner>/<tracker>#<n>`); then the GitHub issue is given a
//! comment naming its new name, with a marker the import reads back, and
//! closed as not planned, so no other host takes it too.
//!
//! The move converges: whatever step a run stopped at, the next one goes on
//! from there. An issue the local tracker keeps already, by its source, is
//! not kept again; one whose GitHub comments carry the marker is not
//! commented on again, only closed where it is still open.
//!
//! What stays, and why: an issue another host claimed — it is that host's to
//! move; one this host's fleet holds a live claim on — its worker, branch and
//! PR name it by its GitHub name, and its landing closes it there, so moving
//! it under it would leave the local copy open and worked twice; and one
//! blocked by an open issue that stays — moved alone, it would be worked
//! before its blocker. A claim of this host whose fleet instance's heartbeat
//! lapsed is no one's work: the issue moves and the claim is released.

use std::collections::{BTreeMap, BTreeSet};

use afkconvoy_core::claims::Claim;
use afkconvoy_core::config::RepoName;
use afkconvoy_core::lease::is_stale;
use afkconvoy_core::record;
use afkconvoy_core::tracker::{Tracker, local_name, repo_labels};
use afkconvoy_issues::{Import, Store};
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::checkout_list::{List, named_repo, tracker_of};
use crate::claims::Scan;
use crate::gh::GitHub;
use crate::gh::issues::GitHubIssues;
use crate::git::Repo;
use crate::issues::{BlockedBy, Comment, Issues, local};
use crate::remote::Fresh;

/// The kind of the marker a moved GitHub issue's comment carries: `to`, the
/// name it has in the local tracker it was moved to.
const MOVED: &str = "convoy:moved";

/// Moves the open issues of the GitHub tracker `from` that this host's
/// checkouts' repos have — `here` and every one on the host's checkout list —
/// into the host's local tracker at `now`; with `dry_run`, says what would
/// move and what stays, changing nothing → {from, to, dry_run, moved
/// [{from, to, repo}], skipped [{from, why}], errors, next}.
pub fn import(here: &Repo, from: &RepoName, dry_run: bool, now: u64) -> Result<Value> {
    let host = crate::host_settings::host_name();
    let to = local_name(&host);
    let from = lowered(from);
    let mut answer = Answer::default();
    let mut reads = vec![];
    for (repo, checkouts) in host_checkouts(here)? {
        match Read::of(&from, repo.clone(), checkouts) {
            Ok(read) => reads.push(read),
            Err(error) => answer.errors.push(json!({"repo": repo, "error": format!("{error:#}")})),
        }
    }
    let path = local::store_path()?;
    let mut store = match dry_run && !path.exists() {
        true => None,
        false => Some(local::open_store(&path)?),
    };
    let found = found(&reads);
    let mut kept = BTreeMap::new();
    if let Some(store) = store.as_mut() {
        let named = found.values().flat_map(|issue| {
            let edges = issue.blockers.iter().filter(|edge| same(edge, &from));
            std::iter::once(issue.number).chain(edges.map(|edge| edge.number))
        });
        for number in named.collect::<BTreeSet<_>>() {
            if let Some(local) = store.imported(&source(&from, number))? {
                kept.insert(number, local);
            }
        }
    }
    let plan = Plan::of(&from, &to, &host, &reads, &found, &kept, now);
    match store {
        Some(store) if !dry_run => plan.carry_out(store, &mut reads, &found, now, &mut answer),
        _ => plan.say(&reads, &found, &mut answer),
    }
    let next = next(&from, &reads);
    Ok(json!({
        "from": from, "to": to, "dry_run": dry_run,
        "moved": answer.moved, "skipped": answer.skipped, "errors": answer.errors,
        "next": next,
    }))
}

/// What the answer lists, as the run goes.
#[derive(Default)]
struct Answer {
    moved: Vec<Value>,
    skipped: Vec<Value>,
    errors: Vec<Value>,
}

/// `name` in lower case: as a source and a claim's ref name it.
fn lowered(name: &RepoName) -> RepoName {
    RepoName::read(&name.as_str().to_ascii_lowercase()).expect("a repo's name, lowered, is one")
}

/// What names GitHub issue `number` of `from`, as the local tracker keeps
/// its source: `<owner>/<tracker>#<n>`.
fn source(from: &RepoName, number: u64) -> String {
    format!("{from}#{number}")
}

/// Whether `edge` is to an issue of `from`: one whose tracker GitHub does not
/// name is.
fn same(edge: &BlockedBy, from: &RepoName) -> bool {
    edge.repo.as_deref().is_none_or(|repo| repo.eq_ignore_ascii_case(from.as_str()))
}

/// Every repo this host has a checkout of — `here`, and each on its checkout
/// list — with its checkouts, by the repo's name.
fn host_checkouts(here: &Repo) -> Result<Vec<(RepoName, Vec<Repo>)>> {
    let listed = List::of_host()?.read()?.checkouts;
    let listed = listed.iter().filter_map(|entry| Repo::at(&entry.checkout).ok());
    let mut repos: Vec<(RepoName, Vec<Repo>)> = vec![];
    let mut seen = BTreeSet::new();
    for checkout in std::iter::once(here.clone()).chain(listed) {
        if !seen.insert(checkout.dir.clone()) {
            continue;
        }
        let Ok(repo) = named_repo(&checkout) else { continue };
        match repos.iter_mut().find(|(named, _)| *named == repo) {
            Some((_, checkouts)) => checkouts.push(checkout),
            None => repos.push((repo, vec![checkout])),
        }
    }
    if repos.is_empty() {
        bail!("{} names no repo on GitHub, and this host lists no checkout", here.dir.display());
    }
    Ok(repos)
}

/// One repo's part of the tracker: its open issues there, and the claims of
/// them on the repo's remote, as one read saw them.
struct Read {
    repo: RepoName,
    checkouts: Vec<Repo>,
    issues: GitHubIssues,
    open: crate::issues::OpenIssues,
    /// The claims on the tracker's issues, under each name a fleet of the
    /// repo may give them: `<owner>/<tracker>/<n>`, and `<n>` too where the
    /// tracker is the repo itself.
    scans: Vec<Scan>,
}

impl Read {
    fn of(from: &RepoName, repo: RepoName, checkouts: Vec<Repo>) -> Result<Read> {
        let checkout = checkouts[0].clone();
        let other = Tracker::Other { tracker: from.clone(), repo: repo.clone() };
        let own = repo.as_str().eq_ignore_ascii_case(from.as_str());
        let tracker = if own { Tracker::Own } else { other.clone() };
        let issues = GitHubIssues::new(GitHub::new(checkout.dir.clone(), tracker.clone()));
        let open = issues.open_issues(None)?.issues;
        let layout = checkout.layout();
        let records = checkout.remote().records(&[&layout.every("")], Fresh::Live)?;
        let mut scans = vec![Scan::among(layout, other, records.clone())];
        if own {
            scans.push(Scan::among(layout, Tracker::Own, records));
        }
        Ok(Read { repo, checkouts, issues, open, scans })
    }

    /// The claim on issue `number`, with the scan it is in.
    fn claim(&self, number: u64) -> Option<(usize, &Claim)> {
        self.scans.iter().enumerate().find_map(|(at, scan)| Some((at, scan.claim_of(number)?)))
    }
}

/// One open issue of the tracker, of one repo this host has a checkout of.
struct Found {
    number: u64,
    /// Its repo's read (`reads`), or `None` where several repos' reads have
    /// it — it carries their repo labels.
    read: Option<usize>,
    title: String,
    body: String,
    /// Its labels, but its repo label.
    labels: Vec<String>,
    created: Option<u64>,
    edited: Option<u64>,
    comments: Vec<Comment>,
    blockers: Vec<BlockedBy>,
}

impl Found {
    /// The name its marker comment gives it, where one does.
    fn marked(&self) -> Option<String> {
        self.comments.iter().find_map(|comment| {
            Some(record::read_marker(MOVED, &comment.body)?.get("to")?.to_string())
        })
    }
}

/// The open issues of every repo's read, by number.
fn found(reads: &[Read]) -> BTreeMap<u64, Found> {
    let mut found: BTreeMap<u64, Found> = BTreeMap::new();
    for (at, read) in reads.iter().enumerate() {
        for row in &read.open.rows {
            if let Some(had) = found.get_mut(&row.number) {
                had.read = None;
                continue;
            }
            let body = read.open.body(row.number);
            let carried = repo_labels(&row.labels);
            let labels = row.labels.iter().filter(|label| !carried.contains(&label.as_str()));
            found.insert(
                row.number,
                Found {
                    number: row.number,
                    read: (carried.len() <= 1).then_some(at),
                    title: row.title.clone(),
                    body: body.map(|body| body.body.clone()).unwrap_or_default(),
                    labels: labels.cloned().collect(),
                    created: body.and_then(|body| body.at),
                    edited: read.open.body_edited(row.number),
                    comments: read.open.comments(row.number).to_vec(),
                    blockers: read.open.blockers.get(&row.number).cloned().unwrap_or_default(),
                },
            );
        }
    }
    found
}

/// What becomes of one open issue.
#[derive(Debug, Clone)]
enum Fate {
    /// It moves; `release`: this host's lapsed claim on it, in that scan of
    /// its read, which is released as it moves.
    Moves { release: Option<(usize, Claim)> },
    /// It was moved before, to that name, and its marker comment is on it:
    /// what is left is to close it on GitHub.
    Moved(String),
    /// It stays on GitHub, for this reason.
    Stays(String),
}

/// What one of an issue's blockers comes to in the local tracker.
enum Edge {
    /// The local tracker's issue of that number.
    Local(u64),
    /// The tracker's issue of that number, which moves in this run.
    Moving(u64),
    /// No edge: a closed issue that is not moved, as the moved issue's note
    /// names it.
    Note(String),
    /// The issue cannot move while this blocks it, for this reason.
    Holds(String),
}

/// What the run does, issue by issue: each one's fate, and the order the
/// moving ones are kept in — every blocker before the issues it blocks.
struct Plan {
    from: RepoName,
    to: RepoName,
    fates: BTreeMap<u64, Fate>,
    order: Vec<u64>,
    kept: BTreeMap<u64, u64>,
}

impl Plan {
    fn of(
        from: &RepoName,
        to: &RepoName,
        host: &str,
        reads: &[Read],
        found: &BTreeMap<u64, Found>,
        kept: &BTreeMap<u64, u64>,
        now: u64,
    ) -> Plan {
        let mut plan = Plan {
            from: from.clone(),
            to: to.clone(),
            fates: BTreeMap::new(),
            order: vec![],
            kept: kept.clone(),
        };
        for issue in found.values() {
            let fate = plan.first_fate(issue, reads, host, now);
            plan.fates.insert(issue.number, fate);
        }
        // An issue held by an open blocker that stays, stays: and so, in
        // turn, does any it blocks.
        loop {
            let mut held = vec![];
            for issue in found.values() {
                if !matches!(plan.fates[&issue.number], Fate::Moves { .. }) {
                    continue;
                }
                let holds = issue.blockers.iter().find_map(|edge| match plan.edge(edge) {
                    Edge::Holds(why) => Some(why),
                    _ => None,
                });
                held.extend(holds.map(|why| (issue.number, why)));
            }
            if held.is_empty() {
                break;
            }
            for (number, why) in held {
                plan.fates.insert(number, Fate::Stays(why));
            }
        }
        plan.order(found);
        plan
    }

    /// Its fate by itself, before its blockers are weighed.
    fn first_fate(&self, issue: &Found, reads: &[Read], host: &str, now: u64) -> Fate {
        let Some(at) = issue.read else {
            return Fate::Stays(
                "it carries the repo labels of several repos: it is no one repo's to move".into(),
            );
        };
        if let Some(marked) = issue.marked() {
            if let Some(local) = self.kept.get(&issue.number) {
                return Fate::Moved(self.local(*local));
            }
            return Fate::Stays(match marked.strip_prefix(&format!("{}#", self.to)) {
                Some(_) => format!(
                    "its comment says it moved to {marked}, which this host's local tracker does \
                     not hold: it stays open on GitHub, where it is still whole"
                ),
                None => format!("it moved to {marked} already, another host's local tracker"),
            });
        }
        let read = &reads[at];
        let Some((scan, claim)) = read.claim(issue.number) else {
            return Fate::Moves { release: None };
        };
        let Some(owner) = &claim.owner else {
            return Fate::Stays("its claim names no host: it is no host's to move".into());
        };
        if owner.host != host {
            return Fate::Stays(format!(
                "claimed by host {} (instance {}): it stays for that host's own import",
                owner.host, owner.instance
            ));
        }
        let beat = read.scans[scan].last_beat(&owner.instance);
        if !is_stale(beat, now) {
            return Fate::Stays(format!(
                "this host's fleet instance {} holds a live claim on it: its worker, branch and \
                 PR name it on GitHub, where its landing closes it. It moves once the claim is \
                 released — its work landed, or `afkconvoy release {} --instance {}`",
                owner.instance, issue.number, owner.instance
            ));
        }
        Fate::Moves { release: Some((scan, claim.clone())) }
    }

    /// The local tracker's issue `number`, by name.
    fn local(&self, number: u64) -> String {
        format!("{}#{number}", self.to)
    }

    fn edge(&self, edge: &BlockedBy) -> Edge {
        let open = edge.state.eq_ignore_ascii_case("open");
        if !same(edge, &self.from) {
            let name = format!("{}#{}", edge.repo.as_deref().unwrap_or_default(), edge.number);
            return match open {
                true => Edge::Holds(format!(
                    "blocked by {name}, of another tracker, which stays on GitHub"
                )),
                false => Edge::Note(format!("{name} (closed)")),
            };
        }
        if let Some(local) = self.kept.get(&edge.number) {
            return Edge::Local(*local);
        }
        let name = source(&self.from, edge.number);
        match self.fates.get(&edge.number) {
            Some(Fate::Moves { .. }) => Edge::Moving(edge.number),
            Some(Fate::Stays(why)) if open => {
                Edge::Holds(format!("blocked by {name}, which stays on GitHub: {why}"))
            }
            _ if open => Edge::Holds(format!(
                "blocked by {name}, which stays on GitHub: it is of no repo this host has a \
                 checkout of"
            )),
            _ => Edge::Note(format!("{name} (closed)")),
        }
    }

    /// Puts the moving issues in an order that keeps every blocker before the
    /// issues it blocks; those blocked through a cycle stay.
    fn order(&mut self, found: &BTreeMap<u64, Found>) {
        let mut left: BTreeMap<u64, BTreeSet<u64>> = found
            .values()
            .filter(|issue| matches!(self.fates[&issue.number], Fate::Moves { .. }))
            .map(|issue| {
                let moving = issue.blockers.iter().filter_map(|edge| match self.edge(edge) {
                    Edge::Moving(number) => Some(number),
                    _ => None,
                });
                (issue.number, moving.collect())
            })
            .collect();
        loop {
            let ready: Vec<u64> = left
                .iter()
                .filter(|(_, blockers)| blockers.iter().all(|blocker| self.order.contains(blocker)))
                .map(|(number, _)| *number)
                .collect();
            if ready.is_empty() {
                break;
            }
            for number in ready {
                left.remove(&number);
                self.order.push(number);
            }
        }
        for number in left.into_keys() {
            let why = "it is blocked, through its blockers on GitHub, by itself: a cycle";
            self.fates.insert(number, Fate::Stays(why.into()));
        }
    }

    /// What a dry run answers: what would move, and what stays and why.
    fn say(&self, reads: &[Read], found: &BTreeMap<u64, Found>, answer: &mut Answer) {
        for number in &self.order {
            let repo = found[number].read.map(|at| &reads[at].repo);
            let to = self.kept.get(number).map(|local| self.local(*local));
            let from = source(&self.from, *number);
            answer.moved.push(json!({"from": from, "to": to, "repo": repo, "released": null}));
        }
        for (number, fate) in &self.fates {
            let from = source(&self.from, *number);
            match fate {
                Fate::Moves { .. } => {}
                Fate::Moved(to) => answer.skipped.push(json!({
                    "from": from, "to": to,
                    "why": format!("moved already to {to}: it is closed on GitHub next"),
                })),
                Fate::Stays(why) => answer.skipped.push(json!({"from": from, "why": why})),
            }
        }
    }

    /// Moves each moving issue, blockers first, and closes each moved one on
    /// GitHub; an issue whose step fails is an error, and the next run goes
    /// on from where it stopped.
    fn carry_out(
        &self,
        mut store: Store,
        reads: &mut [Read],
        found: &BTreeMap<u64, Found>,
        now: u64,
        answer: &mut Answer,
    ) {
        let mut moved: BTreeMap<u64, u64> = self.kept.clone();
        for number in &self.order {
            let issue = &found[number];
            let from = source(&self.from, *number);
            let read = &mut reads[issue.read.expect("a moving issue is one repo's")];
            let local = match self.keep(&mut store, issue, &read.repo, &moved, now) {
                Ok(local) => local,
                Err(error) => {
                    answer.errors.push(json!({"from": from, "error": format!("{error:#}")}));
                    continue;
                }
            };
            moved.insert(*number, local);
            let to = self.local(local);
            let Fate::Moves { release } = &self.fates[number] else { unreachable!() };
            match self.close(read, issue, &to, release.as_ref()) {
                Ok(released) => answer.moved.push(json!({
                    "from": from, "to": to, "repo": read.repo, "released": released,
                })),
                Err(error) => answer.errors.push(json!({
                    "from": from, "to": to,
                    "error": format!("kept as {to}, but not closed on GitHub: {error:#}"),
                })),
            }
        }
        for (number, fate) in &self.fates {
            let from = source(&self.from, *number);
            match fate {
                Fate::Moves { .. } => {}
                Fate::Stays(why) => answer.skipped.push(json!({"from": from, "why": why})),
                Fate::Moved(to) => {
                    let issue = &found[number];
                    let read = &mut reads[issue.read.expect("a moved issue is one repo's")];
                    match self.close(read, issue, to, None) {
                        Ok(_) => answer.skipped.push(json!({
                            "from": from, "to": to,
                            "why": format!("moved already to {to}; closed on GitHub now"),
                        })),
                        Err(error) => answer.errors.push(json!({
                            "from": from, "to": to,
                            "error": format!("moved already to {to}, but not closed on GitHub: {error:#}"),
                        })),
                    }
                }
            }
        }
    }

    /// Keeps `issue` in the local tracker → its number there: the one kept
    /// of it already, where there is one. `moved`: the local number of each
    /// of the tracker's issues kept so far.
    fn keep(
        &self,
        store: &mut Store,
        issue: &Found,
        repo: &RepoName,
        moved: &BTreeMap<u64, u64>,
        now: u64,
    ) -> Result<u64> {
        let mut blockers = vec![];
        let mut notes = vec![];
        for edge in &issue.blockers {
            match self.edge(edge) {
                Edge::Local(local) => blockers.push(local),
                Edge::Moving(number) => match moved.get(&number) {
                    Some(local) => blockers.push(*local),
                    None => bail!(
                        "not moved: its blocker {} could not be moved",
                        source(&self.from, number)
                    ),
                },
                Edge::Note(note) => notes.push(note),
                Edge::Holds(why) => bail!("not moved: {why}"),
            }
        }
        let created = issue.created.unwrap_or(now);
        let mut comments: Vec<(&str, u64)> = issue
            .comments
            .iter()
            .map(|comment| (comment.body.as_str(), comment.at.unwrap_or(created)))
            .collect();
        let from = source(&self.from, issue.number);
        let note = (!notes.is_empty()).then(|| {
            format!(
                "Moved from {from}, where it was blocked by {} too: not moved, so no edge to it \
                 is kept here.",
                notes.join(", ")
            )
        });
        comments.extend(note.as_deref().map(|note| (note, now)));
        let labels: Vec<&str> = issue.labels.iter().map(String::as_str).collect();
        let import = Import {
            source: &from,
            repo: repo.as_str(),
            title: &issue.title,
            body: &issue.body,
            labels: &labels,
            created,
            body_edited: issue.edited,
            comments: &comments,
            blockers: &blockers,
        };
        Ok(store.import(&import, now)?)
    }

    /// Closes the GitHub issue of `issue`, moved to `to`, as not planned:
    /// releases this host's lapsed claim on it first, where `release` names
    /// one, and comments its new name where no comment does yet → the claim's
    /// ref released.
    fn close(
        &self,
        read: &mut Read,
        issue: &Found,
        to: &str,
        release: Option<&(usize, Claim)>,
    ) -> Result<Option<String>> {
        let mut released = None;
        if let Some((scan, claim)) = release {
            let checkout = read.checkouts[0].clone();
            let deleted =
                crate::claims::delete(&checkout, &mut read.scans[*scan], issue.number, &claim.sha)?;
            released = deleted["ref"].as_str().map(str::to_string);
        }
        if issue.marked().is_none() {
            read.issues.comment(issue.number, &marker(to))?;
        }
        read.issues.close_not_planned(issue.number)?;
        Ok(released)
    }
}

/// The comment a moved GitHub issue is given: its new name, under a marker
/// the import reads back.
fn marker(to: &str) -> String {
    format!(
        "{}\nMoved to {to}: this issue is in that host's local tracker now, and its work goes on \
         there (`afkconvoy issue view` on that host).",
        record::marker(MOVED, &[("to", to)])
    )
}

/// The line that has each checkout whose fleet works `from` work this host's
/// local tracker instead, once its issues are moved.
fn next(from: &RepoName, reads: &[Read]) -> Vec<String> {
    let mut lines = vec![];
    for read in reads {
        for checkout in &read.checkouts {
            let works = match tracker_of(checkout) {
                Ok(Tracker::Own) => read.repo.as_str().eq_ignore_ascii_case(from.as_str()),
                Ok(Tracker::Other { tracker, .. }) => {
                    tracker.as_str().eq_ignore_ascii_case(from.as_str())
                }
                _ => false,
            };
            if works {
                lines.push(format!("afkconvoy run {} --tracker local", checkout.dir.display()));
            }
        }
    }
    lines
}

/// What `afkconvoy issue import --help` says after its one line.
pub(crate) const HELP: &str = r#"## When to run it

Once on each host, when its fleets move off a GitHub tracker to the host's local tracker (docs/adr/0091), in any checkout of the host: `--dry-run` first, to read what would move and what stays. Run it again at any time: it moves nothing twice, and goes on from wherever a run before it stopped. Stop the host's fleets of those checkouts first (`afkconvoy stop`): a claim a fleet takes while the import runs is left on an issue the import closed on GitHub.

## Who runs it

The operator, on the host whose issues they are.

## What it does

It reads the open issues of the GitHub tracker `--from` of every repo this host has a checkout of — the checkout it is run in and every one on the host's checkout list. An issue is a repo's by its repo label, `repo:<owner>/<repo>`, or, in a repo that is its own tracker, by carrying none. For each, in an order that puts every blocker before the issues it blocks:

1. It keeps it in this host's local tracker, in one write: its title, body and labels — but its repo label: its repo is a field there — and every comment, in order, each body kept exactly as written (claim comments, stage records, verdicts and work accounts are read from their text). It is opened, edited and commented on when GitHub says; its labels are put on when it moves. Each blocker that moves too, or moved before, is kept as an edge to its local issue. The local issue names its source, `<owner>/<tracker>#<n>`: an issue kept already of that source is not kept again.
2. Where this host holds a claim on it whose fleet instance's heartbeat has lapsed, it releases the claim.
3. It comments on the GitHub issue `Moved to local/<host>#<n>`, under a marker it reads back, and closes it as not planned — so no other host takes it too.

What stays on GitHub, each with why:
- An issue claimed by another host: it is that host's own import's to move.
- An issue this host's fleet holds a live claim on: its worker, branch and PR name it on GitHub, where its landing closes it. It moves once the claim is released.
- An issue blocked by an open issue that stays — another host's, another tracker's, another repo's — or blocked through a cycle: moved alone, it would be worked before its blocker. A closed blocker that does not move keeps no edge; the moved issue's last comment names it.
- An issue whose comment says it moved to another host's local tracker.

It does not have a checkout work the local tracker: the answer's `next` says the line, `afkconvoy run <checkout> --tracker local`, for each checkout whose tracker is `--from`.

## What it reads and writes

It reads the tracker's open issues, with their comments and blockers, and the claims and heartbeats on each repo's remote. It writes this host's local tracker, the GitHub issues it moves — a comment, a close — and the remote's claim refs it releases. `--dry-run` writes nothing.

## What it answers

{"from", "to", "dry_run", "moved": [{"from", "to", "repo", "released"}], "skipped": [{"from", "why", "to"?}], "errors": [{"from" or "repo", "error", "to"?}], "next": [line…]}. `from` is `<owner>/<tracker>#<n>`, `to` `local/<host>#<n>`: in a dry run, the local name where it is kept already, else null. `released` is the claim ref released. An issue kept locally but not yet closed on GitHub is an error naming both; the next run closes it.

## What trips you up

- Who wrote each comment is not kept: the local tracker keeps text, not authors.
- An issue another host claimed stays open on GitHub until that host imports it."#;
