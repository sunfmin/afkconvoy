//! One process's view of a repo: its checkout, its config, GitHub and the
//! remote's claims — and the rebuild that assembles a working set from them.
//!
//! A rebuild asks the remote once and GitHub a fixed number of times, side by
//! side: one listing of branch tips and every ref of the fleet
//! (`Remote::open`), and what is open on GitHub (`GitHub::open`). A patrol
//! that bounds how many issues it lands in a day is read in that same issues
//! query. Neither grows with the issues in flight.

use afkconvoy_core::casting;
use afkconvoy_core::claims::{Claim, InstanceId};
use afkconvoy_core::config::Config;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::plan::{Excluded, Observed, WorkingSet, assemble, closing_pr};
use afkconvoy_core::position::Position;
use afkconvoy_core::selection::{self, Facts, Named, Selection, Unsafe, UserConfig};
use afkconvoy_core::stopped::standing_verdict;
use afkconvoy_core::team::{Role, Team, Workflow};
use afkconvoy_core::worker::recorded_branches;
use anyhow::{Context, Result, anyhow};
use std::thread;

use crate::claims::Scan;
use crate::gh::{GitHub, Open, OpenIssues, beside, gone};
use crate::git::Repo;
use crate::orca::{Source, Worktree};
use crate::patrol;
use crate::remote::{Fresh, Listing, Remote};
use crate::settle::cut_landing_of;
use crate::slow_gate;
use crate::stage::Flows;

/// What a rebuild read of the claimed issues besides the working set: what
/// the tick says of them.
#[derive(Debug, Default)]
pub struct Claimed {
    /// The claimed issues it could not read, each with why: the pass goes on
    /// with them as open, and its tick reports each.
    pub unread: Vec<(u64, anyhow::Error)>,
    /// The closed claimed issues it read, each with the login of whoever
    /// closed it, where GitHub says: what a tick's line names.
    pub closers: Vec<(u64, String)>,
}

struct Runnable {
    kit: crate::kit::Kit,
    skipped: Vec<(Harness, String)>,
}

pub struct Fleet {
    pub repo: Repo,
    /// The harness this checkout names, where it names one (`named_harness`).
    /// None, and the host kit's order chooses the first eligible harness.
    pub pinned: Option<Harness>,
    /// Candidates excluded for this continuation only, never a preference.
    pub avoiding: Vec<Harness>,
    runnable: std::cell::RefCell<Option<Runnable>>,
    /// The user configs the harness user default is read from, read once.
    user: std::cell::RefCell<Option<Vec<UserConfig>>>,
    /// One view of the issue's refusal comments, seeded by the paged rebuild.
    refusals:
        std::cell::RefCell<std::collections::BTreeMap<u64, Vec<afkconvoy_core::worker::Refusal>>>,
    /// The repo's config, with the concurrency its checkout says on this
    /// host laid over it (`concurrency::config_of`).
    pub config: Config,
    pub gh: GitHub,
    pub scan: Scan,
    /// The remote, opened on the base branch and the fleet's refs as it had
    /// them when this view was opened: what `scan` was read from, and what a
    /// `Fresh::Opened` read answers.
    pub remote: Remote,
    pub now: u64,
    /// A startup probe that still stands already checked every harness.
    /// `runnable` then keeps the kit `running` built and starts none again.
    trust_launches: bool,
    /// Patrol landings of the day, read with the open issues. `None`: this
    /// view did not ask, or the closed page may be short of the day, and
    /// `patrol::seen` asks. `Some` includes a day in which nothing landed.
    pub(crate) patrol_landings: Option<Vec<(String, u64)>>,
}

/// Why no worker of a role can be started on this host: it holds limits for
/// no harness the host kit holds (`casting::nothing_safe`). Its stages wait
/// while it lasts; nothing else does.
#[derive(Debug)]
pub struct NothingSafe(pub String);

impl std::fmt::Display for NothingSafe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NothingSafe {}

impl Fleet {
    /// The fleet's view from the checkout `repo`.
    pub fn open(repo: &Repo, now: u64) -> Result<Fleet> {
        Fleet::open_on(repo, GitHub::of(repo)?, now)
    }

    /// The fleet's view from the checkout `repo`, reading GitHub through `gh`.
    fn open_on(repo: &Repo, gh: GitHub, now: u64) -> Result<Fleet> {
        let repo = repo.clone();
        // As this checkout's fleet instance goes by it on this host: read
        // anew at each view, so a concurrency said since holds now.
        let config = crate::concurrency::config_of(&repo)?;
        let pinned = crate::named_harness::named(&repo)?;

        let remote = Remote::open(&repo, &config.base_branch)?;
        let scan =
            Scan::among(repo.layout(), gh.tracker.clone(), remote.listing()?.records.clone());
        Ok(Fleet {
            repo,
            pinned,
            avoiding: vec![],
            runnable: std::cell::RefCell::new(None),
            user: std::cell::RefCell::new(None),
            refusals: std::cell::RefCell::new(Default::default()),
            config,
            gh,
            scan,
            remote,
            now,

            trust_launches: false,
            patrol_landings: None,
        })
    }

    /// The startup probe that stands checked these harnesses. Later selection
    /// in this view starts none of them again.
    pub fn trust_launches(&mut self) {
        self.trust_launches = true;
    }

    /// The fleet's view, opened for a rebuild → it, and what is open on
    /// GitHub. The remote and GitHub do not depend on each other: they are
    /// asked side by side, of the one tracker the view goes by.
    pub fn read(repo: &Repo, now: u64) -> Result<(Fleet, Open)> {
        let gh = GitHub::of(repo)?;
        // Before the issues query, so a patrol that bounds its day is counted
        // in that query instead of a second one. A team that cannot be read
        // leaves the landings unasked.
        let since = patrol::landings_since(repo, now);
        let (fleet, open) = thread::scope(|scope| {
            let open = scope.spawn(|| gh.open_with_landings(since));
            (Fleet::open_on(repo, gh.clone(), now), beside(open))
        });
        let mut fleet = fleet?;
        let opened = open?;
        fleet.patrol_landings = opened.landings;
        Ok((fleet, opened.open))
    }

    /// The one worker selection (`selection::select`) of a worker of `role`
    /// — `None` of no role — carrying `labels`, on `issue` where it is an
    /// issue's, from the facts this view gathers once: the host kit whole,
    /// what of it can start now and why the rest cannot, the harness this
    /// checkout names, the pauses, and the harness user default. An error is
    /// a fact that could not be read.
    pub fn select(
        &self,
        role: Option<&Role>,
        labels: &[String],
        issue: Option<u64>,
    ) -> Result<Result<Selection, Unsafe>> {
        let whole = crate::kit::load()?.range();
        let runnable = self.runnable_for(issue)?.range();
        let left_out = self.left_out(&whole, &runnable, issue);
        let paused: Vec<(Harness, String)> = crate::paused::read()
            .iter()
            .map(|pause| (pause.harness, crate::paused::said(pause, self.now)))
            .collect();
        let user = self.user_configs()?;
        let checkout = self.repo.dir.display().to_string();
        let named = self.pinned.map(|harness| Named { harness, checkout: &checkout });
        Ok(selection::select(&Facts {
            kit: &whole,
            runnable: &runnable,
            left_out: &left_out,
            named,
            paused: &paused,
            role,
            labels,
            of_issue: issue.is_some(),
            user: &user,
        }))
    }

    /// `select`, with why nothing is safe as an error a caller can tell
    /// apart: a pause (`paused::Held`), nothing safe (`NothingSafe`), or a
    /// refused user default.
    pub fn select_in(
        &self,
        role: Option<&Role>,
        labels: &[String],
        issue: Option<u64>,
    ) -> Result<Selection> {
        self.select(role, labels, issue)?.map_err(|why| match why {
            Unsafe::Paused(why) => crate::paused::Held(why).into(),
            Unsafe::NothingSafe(why) => NothingSafe(why).into(),
            Unsafe::Refused { why, .. } => anyhow!(why),
        })
    }

    /// Each user config a harness reads its user default from, read once in
    /// this view (`codex_launch::user_config`).
    fn user_configs(&self) -> Result<Vec<UserConfig>> {
        if self.user.borrow().is_none() {
            let mut user = vec![];
            for harness in Harness::ALL {
                if let Some(config) = crate::codex_launch::user_config(harness)? {
                    user.push(config);
                }
            }
            *self.user.borrow_mut() = Some(user);
        }
        Ok(self.user.borrow().clone().unwrap_or_default())
    }

    /// Each harness of `whole` that `runnable` leaves out for `issue`, with
    /// why, where this view knows why: the issue refused it, it cannot
    /// start, it is faulted, or it is paused.
    fn left_out(
        &self,
        whole: &[casting::Held],
        runnable: &[casting::Held],
        issue: Option<u64>,
    ) -> Vec<(Harness, String)> {
        let refusals = issue.and_then(|issue| self.refusals.borrow().get(&issue).cloned());
        let skipped = self.runnable.borrow().as_ref().map(|runnable| runnable.skipped.clone());
        let paused = crate::paused::read();
        let mut left = vec![];
        for held in whole {
            let harness = held.harness;
            if runnable.iter().any(|runnable| runnable.harness == harness) {
                continue;
            }
            let refused = refusals.iter().flatten().find(|refusal| refusal.harness == harness).map(
                |refusal| format!("{} refused this task: {}", harness.key(), refusal.evidence),
            );
            let skipped = || {
                skipped
                    .iter()
                    .flatten()
                    .find(|(skipped, _)| *skipped == harness)
                    .map(|(_, why)| why.clone())
            };
            let paused = || {
                paused.iter().find(|pause| pause.harness == harness).map(|pause| {
                    format!("`{}` is paused on this host: {}", harness.key(), pause.why)
                })
            };
            let why = refused
                .or_else(skipped)
                .or_else(|| crate::health::fault_reason(harness))
                .or_else(paused);
            if let Some(why) = why {
                left.push((harness, why));
            }
        }
        left
    }

    /// One view's capability snapshot. Failed startability checks skip only
    /// that candidate; they never rewrite the kit or remote casting.
    pub fn runnable(&self) -> Result<crate::kit::Kit> {
        if self.runnable.borrow().is_none() {
            let (mut kit, _) = crate::kit::running()?;
            let mut skipped = vec![];
            let caller = match self.pinned {
                Some(_) => "the harness this checkout names",
                None => "kit priority",
            };
            if let Some(pinned) = self.pinned {
                if !kit.harnesses.iter().any(|held| held.harness == pinned) {
                    skipped.push((pinned, self.why_absent(pinned)));
                    kit.harnesses.clear();
                } else {
                    kit.harnesses.retain(|held| held.harness == pinned);
                }
            }
            // A standing probe already checked these commands. Trusting it
            // keeps the named harness and starts nothing.
            if !self.trust_launches {
                let launches = crate::launch::Launches::foreseen(&self.repo);
                kit.harnesses.retain(|held| {
                    match crate::role::startable(&launches, held.harness, caller) {
                        Ok(()) => true,
                        Err(e) => {
                            skipped.push((held.harness, format!("{e:#}")));
                            false
                        }
                    }
                });
            }
            *self.runnable.borrow_mut() = Some(Runnable { kit, skipped });
        }
        let mut kit = self.runnable.borrow().as_ref().context("runnable kit")?.kit.clone();
        kit.harnesses.retain(|held| !self.avoiding.contains(&held.harness));
        Ok(kit)
    }

    /// Why the harness this checkout names is not among those the host can
    /// start now: an account or model fault, a pause, or not in the host kit.
    fn why_absent(&self, pinned: Harness) -> String {
        if let Some(why) = crate::health::fault_reason(pinned) {
            return why;
        }
        if let Some(pause) = crate::paused::read().into_iter().find(|pause| pause.harness == pinned)
        {
            return format!("`{}` is paused on this host: {}", pinned.key(), pause.why);
        }
        format!("`{}` is not a harness this host kit holds", pinned.key())
    }

    /// Eligible on this issue, across every stage and host. The full kit is
    /// still used to resolve role permissions before this exclusion applies.
    pub fn runnable_for(&self, issue: Option<u64>) -> Result<crate::kit::Kit> {
        let mut kit = self.runnable()?;
        if let Some(issue) = issue {
            let refusals = self.refusals(issue)?;
            kit.harnesses.retain(|h| !refusals.iter().any(|r| r.harness == h.harness));
        }
        Ok(kit)
    }

    pub fn refusals(&self, issue: u64) -> Result<Vec<afkconvoy_core::worker::Refusal>> {
        if !self.refusals.borrow().contains_key(&issue) {
            let comments = self.gh.comments(issue)?;
            self.refusals.borrow_mut().insert(
                issue,
                afkconvoy_core::worker::refusals(comments.iter().map(|c| c.body.as_str())),
            );
        }
        Ok(self.refusals.borrow().get(&issue).cloned().unwrap_or_default())
    }

    /// Publish before continuation: losing this process cannot forget why
    /// this harness must never be chosen again for the issue.
    pub fn remember_refusal(
        &self,
        issue: u64,
        refusal: afkconvoy_core::worker::Refusal,
    ) -> Result<()> {
        let mut refusals = self.refusals(issue)?;
        if !refusals.iter().any(|r| r.harness == refusal.harness) {
            self.gh.comment(issue, &refusal.comment())?;
            refusals.push(refusal);
            self.refusals.borrow_mut().insert(issue, refusals);
        }
        Ok(())
    }

    pub fn selection(&self) -> serde_json::Value {
        let kit = self.runnable();
        let why = match self.pinned {
            Some(pinned) => format!(
                "this checkout names {}; the host kit orders that harness's models",
                pinned.key()
            ),
            None => {
                "first eligible harness in kit order; role limits and issue refusals still apply"
                    .to_string()
            }
        };
        serde_json::json!({"order": crate::kit::load().map(|k| k.harnesses.iter().map(|h| h.harness).collect::<Vec<_>>()).unwrap_or_default(),
            "eligible": kit.map(|k| k.harnesses.iter().map(|h| h.harness).collect::<Vec<_>>()).unwrap_or_default(),
            "skipped": self.runnable.borrow().as_ref().map(|runnable| &runnable.skipped),
            "pinned": self.pinned, "health": crate::health::json(&crate::health::show()), "refusals": *self.refusals.borrow(), "why": why})
    }

    pub(crate) fn invalidate_selection(&self) {
        *self.runnable.borrow_mut() = None;
    }

    pub fn role_selection(&self, role: &Role) -> serde_json::Value {
        let chosen = self.select_in(Some(role), &[], None).ok().map(|chosen| chosen.harness());
        let skipped = chosen.map_or_else(Vec::new, |to| self.skipped_before(to, Some(role), None));
        serde_json::json!({"role":role.name,"harness":chosen,"skipped":skipped})
    }

    /// Every kit candidate ahead of the one a worker landed on, with the
    /// concrete constraint that excluded it and the change that makes it
    /// eligible. The same selection snapshot feeds plan, dispatch and switch.
    pub fn skipped_before(
        &self,
        to: Harness,
        role: Option<&Role>,
        issue: Option<u64>,
    ) -> Vec<serde_json::Value> {
        // A named harness has no earlier kit harness standing in for it.
        if self.pinned == Some(to) {
            return vec![];
        }
        let refusals = issue.and_then(|n| self.refusals(n).ok()).unwrap_or_default();
        let whole = crate::kit::load().map(|kit| kit.range()).unwrap_or_default();
        let allowed = role.map_or_else(|| whole.clone(), |role| casting::range(role, &whole));
        let eligible = self.runnable().map(|kit| kit.range()).unwrap_or_default();
        let start_faults = self
            .runnable
            .borrow()
            .as_ref()
            .map(|runnable| runnable.skipped.clone())
            .unwrap_or_default();
        let paused = crate::paused::read();
        whole
            .iter()
            .take_while(|held| held.harness != to)
            .map(|held| {
                let harness = held.harness;
                let key = harness.key();
                let (why, fix) = if let Some(refusal) = refusals.iter().find(|r| r.harness == harness) {
                    (format!("{key} refused this task: {}", refusal.evidence), "continue this issue on another eligible harness".into())
                } else if let Some(role) = role.filter(|role| {
                    afkconvoy_core::team::unlimited(role, harness).is_some()
                }) {
                    (
                        format!(
                            "role {} holds no limits for {key} ({})",
                            role.name,
                            harness.limits()
                        ),
                        format!("write the role's limits for {key} in its {}", harness.limits()),
                    )
                } else if !allowed.iter().any(|held| held.harness == harness) {
                    let role = role.map_or("this worker", |role| role.name.as_str());
                    (
                        format!("{key} is outside the roster range of role {role}"),
                        format!("allow {key} in that role's roster, or switch to a harness it allows"),
                    )
                } else if let Some((_, why)) =
                    start_faults.iter().find(|(skipped, _)| *skipped == harness)
                {
                    (why.clone(), format!("correct that start fault, then switch to {key} again"))
                } else if let Some(pause) = paused.iter().find(|pause| pause.harness == harness) {
                    (
                        format!("{key} is paused on this host: {}", pause.why),
                        format!("make {key} answer its probes; a successful recovery probe restores it"),
                    )
                } else if !eligible.iter().any(|held| held.harness == harness) {
                    let why = crate::health::fault_reason(harness).unwrap_or_else(|| {
                        format!("{key} is unavailable on this host; see health evidence")
                    });
                    (why, format!("restore {key} to health; a successful recovery probe restores it"))
                } else {
                    (
                        format!("{key} has no eligible model within this role's range"),
                        format!("allow one of {key}'s models in the role's roster, or switch to another harness"),
                    )
                };
                serde_json::json!({"harness": harness, "why": why, "fix": fix})
            })
            .collect()
    }

    /// The listing this view was opened on.
    fn listing(&self) -> &Listing {
        self.remote.listing().expect("a fleet's view of the remote is opened listed")
    }

    /// What a worker with no role and no model label runs: a cut, an
    /// inspection, a continuation with no issue.
    pub fn first(&self) -> Result<Selection> {
        self.select_in(None, &[], None)
    }

    /// The harness `first` selects, even where its user default refuses the
    /// start: what a start that runs no model goes by.
    pub fn first_harness(&self) -> Result<Harness> {
        match self.select(None, &[], None)? {
            Ok(chosen) => Ok(chosen.harness()),
            Err(Unsafe::Refused { cast, .. }) => Ok(cast.harness),
            Err(why) => Err(anyhow!(why)),
        }
    }

    /// Every version tag the remote had when this view was opened, each with
    /// the commit it is on.
    pub fn version_tags(&self) -> &[(String, String)] {
        &self.listing().tags
    }

    /// A branch or fleet ref's sha in this view's remote listing, with its
    /// commit brought into the local repo when needed.
    pub fn fetched_at_open(&self, refname: &str) -> Result<Option<String>> {
        self.remote.fetched(refname, Fresh::Opened)
    }

    /// The fleet records matching `pattern` in this view's remote listing.
    pub fn records_at_open(&self, pattern: &str) -> Vec<crate::remote::RemoteRecord> {
        self.listing()
            .records
            .iter()
            .filter(|record| match pattern.strip_suffix('*') {
                Some(prefix) => record.name.starts_with(prefix),
                None => record.name == pattern,
            })
            .cloned()
            .collect()
    }

    /// The recorded gate run in this view's remote listing that still stands.
    pub fn recorded_gate(
        &self,
        tree: &afkconvoy_core::gate::Tree,
        command: &afkconvoy_core::gate::GateCommand,
    ) -> Option<afkconvoy_core::gate::GreenRun> {
        crate::gate::recorded_among(
            &self.listing().records,
            self.repo.layout(),
            tree,
            command,
            self.now,
        )
    }

    /// The base branch's tip on the remote, its commits in this repo.
    pub fn base_tip(&self) -> Result<String> {
        let branch = &self.config.base_branch;
        let listing = self.listing();
        let tip =
            listing.base.as_ref().ok_or_else(|| anyhow!("the remote has no branch {branch:?}"))?;
        self.repo.bring(tip, &listing.base_ref)?;
        Ok(tip.clone())
    }

    /// Destinations derived from this view's GitHub and remote snapshots.
    pub fn targets(&self, open: &OpenIssues) -> Result<serde_json::Value> {
        crate::target::listing(
            &self.repo,
            &self.config,
            &self.gh.tracker,
            &open.rows,
            self.listing(),
        )
    }

    /// The branch is this process's last listing, or a later exact read. A push
    /// by this process drops that ref, so the next read is live — `target::pin`
    /// does, and that is what notices a concurrent selection.
    pub fn task_branch(&self, issue: u64) -> Result<String> {
        crate::target::branch(&self.repo, &self.config, issue)
    }

    pub fn task_tip(&self, issue: u64) -> Result<String> {
        let branch = self.task_branch(issue)?;
        if branch == self.config.base_branch {
            return self.base_tip();
        }
        self.remote.fetched(&format!("refs/heads/{branch}"), Fresh::Process)?.ok_or_else(|| {
            anyhow!(
                "target branch {branch:?} disappeared; restore it before continuing task {issue}"
            )
        })
    }

    /// The task tip in this view's remote listing.
    pub fn task_tip_at_open(&self, issue: u64) -> Result<String> {
        let branch = crate::target::snapshot_branch(
            &self.repo,
            &self.config,
            &self.gh.tracker,
            issue,
            &self.listing().records,
        )?;
        let name = format!("refs/heads/{branch}");
        self.remote.fetched(&name, Fresh::Opened)?.ok_or_else(|| {
            anyhow!(
                "target branch {branch:?} is missing; restore it before continuing {}",
                self.gh.tracker.issue(issue)
            )
        })
    }

    /// Rebuilds the working set of `me` from `open` and the remote as this
    /// view was opened on it (`Fleet::read`) → it, the read of the issues
    /// that follow a workflow it was assembled from, and what it read of the
    /// claimed issues besides. Reads only. A claimed issue that cannot be
    /// read costs only itself: it is taken for open, and noted in `Claimed::unread`.
    pub fn rebuild(
        &self,
        open: &Open,
        me: Option<&InstanceId>,
    ) -> Result<(WorkingSet, Flows, Claimed)> {
        for issue in &open.issues.rows {
            self.refusals.borrow_mut().insert(
                issue.number,
                afkconvoy_core::worker::refusals(
                    open.issues.comments(issue.number).iter().map(|c| c.body.as_str()),
                ),
            );
        }
        let (issues, prs) = (&open.issues.rows, &open.prs);
        let stages = Flows::read(self, &open.issues)?;
        let unworked_blockers = crate::blockers::read(&self.repo, &self.gh.tracker, &open.issues)?;
        let handoffs: Vec<u64> = open
            .issues
            .blockers
            .iter()
            .filter_map(|(&issue, edges)| {
                crate::blockers::can_hand_on(issue, edges, &unworked_blockers).then_some(issue)
            })
            .collect();
        // The list holds only open issues: a claim missing from it is asked
        // about, and is closed, gone — deleted, transferred or never issued —
        // or unread.
        let (mut closed, mut vanished) = (vec![], vec![]);
        let mut claimed = Claimed::default();
        for claim in &self.scan.claims {
            if issues.iter().any(|issue| issue.number == claim.number) {
                continue;
            }
            match self.gh.issue(claim.number) {
                Ok(read) if read.is_closed() => {
                    closed.push(claim.number);
                    claimed.closers.extend(read.closed_by.map(|by| (claim.number, by)));
                }
                Ok(_) => {}
                Err(error) if gone(&error).is_some() => vanished.push(claim.number),
                Err(error) => claimed.unread.push((claim.number, error)),
            }
        }
        let base = self.base_tip()?;
        let mut landed = vec![];
        for claim in &self.scan.claims {
            if !me.is_some_and(|me| claim.is_held_by(me)) || closed.contains(&claim.number) {
                continue;
            }
            let tip = match self.task_tip_at_open(claim.number) {
                Ok(tip) => tip,
                Err(error) => {
                    claimed.unread.push((claim.number, error));
                    continue;
                }
            };
            if cut_landing_of(&self.repo, &self.gh.tracker, &tip, claim, prs)?.is_some() {
                landed.push(claim.number);
            }
        }
        // A PR is the fleet's own when its branch is one recorded on the issue.
        let mut kept = vec![];
        for issue in issues {
            let bodies = open.issues.comments(issue.number).iter().map(|c| c.body.as_str());
            if closing_pr(prs, issue.number)
                .is_some_and(|pr| recorded_branches(bodies).contains(&pr.head_branch))
            {
                kept.push(issue.number);
            }
        }
        let mut declared = vec![];
        for claim in
            self.scan.claims.iter().filter(|claim| me.is_some_and(|me| claim.is_held_by(me)))
        {
            let under = match &self.config.bound {
                Some(_)
                    if !issues.iter().any(|issue| {
                        issue.number == claim.number
                            && afkconvoy_core::plan::is_repair(&issue.labels)
                    }) =>
                {
                    stages.declared(claim.number, &claim.id)
                }
                _ => self.declared_since(open, claim),
            };
            if under {
                declared.push(claim.number);
            }
        }
        let idle = self.idle_landings(me, prs, &closed);
        let heartbeats = self.scan.beats();
        let (landings, slow_gate) = slow_gate::seen(self, &self.listing().records, &base)?;
        let cut = crate::cut::seen(self, &stages, issues, &open.issues, &base)?;
        let seen = Observed {
            unworked_blockers: &unworked_blockers,
            handoffs: &handoffs,
            issues,
            prs,
            claims: &self.scan.claims,
            heartbeats: &heartbeats,
            closed: &closed,
            gone: &vanished,
            kept: &kept,
            landed: &landed,
            declared: &declared,
            idle: &idle,
            due: &stages.due(prs),
            flows: &stages.flow_labels(),
            landing: &stages.landing(&self.scan.claims),
            begun: &stages.begun(),
            patrols: &patrol::seen(self, &stages, &open.issues)?,
            waiting: &stages.waiting(self, open),
            landings: &landings,
            slow_gate: &slow_gate,
            tracker: &self.gh.tracker,
            cut: cut.as_ref(),
        };
        let mut set = assemble(&seen, me, self.now, &self.config);
        if let Some(row) =
            set.cut.as_mut().filter(|row| row.standing == afkconvoy_core::cut::Standing::Due)
        {
            row.not_cutting_here =
                crate::probe::check_cut_needs(&self.repo, &self.config, self.now, false)?;
        }
        // An open failure is said whatever the cut's standing: it holds no
        // cut back but where the same failure repeats (docs/adr/0072).
        if let Some(row) = set.cut.as_mut()
            && let Some(issue) = row.failure
        {
            let held = row.standing == afkconvoy_core::cut::Standing::Failed;
            row.failed = crate::cut::failed(self, &open.issues, issue, &base, held)?;
        }
        open.issues.explain(&mut set.waiting_on_human);

        for waiting in &mut set.waiting_on_human {
            waiting.answer = crate::checkout::about_checkout(&self.repo.dir, &waiting.answer);
        }
        set.casting = crate::caster::status(self, &open.issues, stages.team().ok());
        match self.config.bound {
            None => self.cast_frontier(&mut set, &open.issues),
            Some(_) => self.cast_bound_frontier(&mut set, &open.issues, &stages),
        }
        Ok((set, stages, claimed))
    }

    /// My claims whose PR is open and whose worker is not changing the tree
    /// (`landing_worker::changing_tree`): each takes no slot. One whose
    /// worker could not be asked after takes one, as it did.
    fn idle_landings(
        &self,
        me: Option<&InstanceId>,
        prs: &[afkconvoy_core::plan::PullRequest],
        closed: &[u64],
    ) -> Vec<u64> {
        let mine = self.scan.claims.iter().filter(|claim| {
            me.is_some_and(|me| claim.is_held_by(me))
                && !closed.contains(&claim.number)
                && closing_pr(prs, claim.number).is_some()
        });
        let mut idle = vec![];
        for claim in mine {
            let at_work = || -> Result<bool> {
                let source = Source::of(&self.repo)?;
                let Some(worktree) = Worktree::of_issue(&source, claim.number)? else {
                    return Ok(false);
                };
                let state = worktree.state(self.now)?;
                let answered = crate::turn_answer::read(&worktree)?;
                Ok(crate::turn_answer::handed(&worktree)?.is_some()
                    || afkconvoy_core::landing_worker::changing_tree(Some(state), &answered))
            };
            if matches!(at_work(), Ok(false)) {
                idle.push(claim.number);
            }
        }
        idle
    }

    /// Apply host priority to the unbound frontier. No issue waits for a
    /// model: its one `model:` label is followed, or it runs the user default.
    fn cast_frontier(&self, set: &mut WorkingSet, open: &OpenIssues) {
        let frontier = std::mem::take(&mut set.frontier);
        for issue in frontier {
            let labels = labels_of(open, issue.number);
            self.cast_issue(set, issue, None, &labels);
        }
        set.excluded.sort_by_key(|excluded| excluded.number);
    }

    /// Apply kit priority within each due role's constraints. A role with no
    /// safe harness waits for recovery. No issue waits for a model.
    fn cast_bound_frontier(&self, set: &mut WorkingSet, open: &OpenIssues, stages: &Flows) {
        let Ok(team) = stages.team() else { return };
        let positions = stages.positions();
        let placed = |number: u64| positions.iter().find(|(at, ..)| *at == number);
        let frontier = std::mem::take(&mut set.frontier);
        for issue in frontier {
            let Some((_, workflow, position)) = placed(issue.number) else {
                set.frontier.push(issue);
                continue;
            };
            // No worker due means no role to choose a harness for.
            let Some(role) = due_role(team, workflow, *position) else {
                set.frontier.push(issue);
                continue;
            };
            let labels = labels_of(open, issue.number);
            self.cast_issue(set, issue, Some(role), &labels);
        }
        set.excluded.sort_by_key(|excluded| excluded.number);
    }

    /// Puts `issue` on the frontier with what its worker runs, or excludes
    /// it while a harness is paused or nothing is safe for it. A user
    /// default that refuses its start keeps it there: its dispatch says why.
    fn cast_issue(
        &self,
        set: &mut WorkingSet,
        mut issue: afkconvoy_core::plan::Dispatchable,
        role: Option<&Role>,
        labels: &[String],
    ) {
        let reason = match self.select(role, labels, Some(issue.number)) {
            Ok(Ok(chosen)) => {
                issue.cast = Some(chosen.cast);
                set.frontier.push(issue);
                return;
            }
            Ok(Err(Unsafe::Refused { cast, .. })) => {
                issue.cast = Some(cast);
                set.frontier.push(issue);
                return;
            }
            Ok(Err(why)) => why.said().to_string(),
            Err(error) => format!("{error:#}"),
        };
        set.excluded.push(Excluded { number: issue.number, title: issue.title, reason });
    }

    /// Whether the worker of `claim`, in an unbound repo, declared a verdict
    /// that stands: the latest on the issue, written since the claim was
    /// created — one from an earlier claim is not its own — and not answered
    /// by a worker started on the issue again (`stopped::standing_verdict`).
    /// What cannot be read says no: the claim then takes a slot.
    pub(crate) fn declared_since(&self, open: &Open, claim: &Claim) -> bool {
        let comments = open.issues.comments(claim.number);
        let Some(verdict) = crate::gh::verdict_since(comments, claim.since) else {
            return false;
        };
        let Some(said) = comments.get(verdict.comment) else { return false };
        // Only a verdict that is the claim's own is looked for in its worktree.
        let answered = || -> Option<Option<String>> {
            let worktree =
                Worktree::of_issue(&Source::of(&self.repo).ok()?, claim.number).ok()??;
            worktree.answered().ok()
        };
        answered().is_some_and(|answered| {
            standing_verdict(Some(verdict), Some(&said.url), answered.as_deref()).is_some()
        })
    }
}

/// The role of the worker due on an issue at `position` in `workflow`, one
/// of `team`'s: the stage's, its consult's or its landing's.
/// `None` where no worker is due.
pub fn due_role<'t>(
    team: &'t Team,
    workflow: &'t Workflow,
    position: Position<'t>,
) -> Option<&'t Role> {
    let work = match position {
        Position::Landing => workflow.land.as_ref(),
        at => at.worker(),
    };
    work.map(|work| team.role_of(work))
}

/// The labels `open` read of issue `number`.
fn labels_of(open: &OpenIssues, number: u64) -> Vec<String> {
    let issue = open.rows.iter().find(|issue| issue.number == number);
    issue.map(|issue| issue.labels.clone()).unwrap_or_default()
}
