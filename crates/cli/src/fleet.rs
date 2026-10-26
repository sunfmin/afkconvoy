//! One process's view of a repo: its checkout, its config, GitHub and the
//! remote's claims — and the rebuild that assembles a working set from them.
//!
//! A rebuild asks the remote once and GitHub a fixed number of times, side by
//! side: one listing of the base branch and every ref of the fleet
//! (`Repo::listing`), and what is open on GitHub (`GitHub::open`). Neither
//! grows with the issues in flight.

use afkconvoy_core::casting::{self, Cast, ForCaster, HELD_FOR_CASTER, Standing, Why};
use afkconvoy_core::claims::{Claim, InstanceId};
use afkconvoy_core::config::Config;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::plan::{Excluded, Issue, Observed, WorkingSet, assemble, closing_pr};
use afkconvoy_core::position::Position;
use afkconvoy_core::stock;
use afkconvoy_core::stopped::standing_verdict;
use afkconvoy_core::team::{Consult, Role, Team, Workflow};
use afkconvoy_core::verdict::latest_verdict;
use afkconvoy_core::worker::recorded_branches;
use anyhow::{Context, Result, anyhow};
use std::thread;

use crate::claims::Scan;
use crate::gh::{GitHub, Gone, Open, OpenIssues, beside, gone};
use crate::git::{Listing, Repo};
use crate::orca::{Source, Worktree};
use crate::patrol;
use crate::precedent;
use crate::settle::cut_landing_of;
use crate::slow_gate;
use crate::stage::Flows;

struct Runnable {
    kit: crate::kit::Kit,
    skipped: Vec<(Harness, String)>,
}

pub struct Fleet {
    pub repo: Repo,
    /// Candidates excluded for this continuation only, never a preference.
    pub avoiding: Vec<Harness>,
    runnable: std::cell::RefCell<Option<Runnable>>,
    /// One view of the issue's refusal comments, seeded by the paged rebuild.
    refusals:
        std::cell::RefCell<std::collections::BTreeMap<u64, Vec<afkconvoy_core::worker::Refusal>>>,
    /// The repo's config, with the concurrency its checkout says on this
    /// host laid over it (`concurrency::config_of`).
    pub config: Config,
    pub gh: GitHub,
    pub scan: Scan,
    /// The base branch and the fleet's refs as the remote had them when this
    /// view was opened: what `scan` was read from.
    listing: Listing,
    pub now: u64,
    /// The issues this view escalated, each with why in a few words: what a
    /// tick's line says of them. Every escalation is made through
    /// `judge::escalate_claim`, which is what adds to it.
    pub escalated: Vec<(u64, String)>,
    /// The issues this view found gone — deleted or transferred — as a step
    /// met them, each with whether a claim on it was released: what a tick's
    /// line says of them.
    pub forgotten: Vec<(Gone, bool)>,
    /// The claimed issues the last rebuild could not read, each with why: the
    /// pass goes on with them as open, and its tick reports each.
    pub unread: Vec<(u64, anyhow::Error)>,
    /// The closed claimed issues the last rebuild read, each with the login of
    /// whoever closed it, where GitHub says: what a tick's line names.
    pub closers: Vec<(u64, String)>,
    /// What this view's start and end of a cut said: what a tick's line says
    /// of them, and its log keeps.
    pub cut_said: Vec<crate::log::What>,
    /// The open proposals on the team's repo, as the last rebuild read them.
    pub proposals: Option<precedent::Open>,
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

        let listing = repo.listing(&config.base_branch)?;
        let scan = Scan::among(repo.layout(), gh.tracker.clone(), listing.records.clone());
        Ok(Fleet {
            repo,
            avoiding: vec![],
            runnable: std::cell::RefCell::new(None),
            refusals: std::cell::RefCell::new(Default::default()),
            config,
            gh,
            scan,
            listing,
            now,
            escalated: vec![],
            forgotten: vec![],
            unread: vec![],
            closers: vec![],
            cut_said: vec![],
            proposals: None,
        })
    }

    /// The fleet's view, opened for a rebuild → it, and what is open on
    /// GitHub. The remote and GitHub do not depend on each other: they are
    /// asked side by side, of the one tracker the view goes by.
    pub fn read(repo: &Repo, now: u64) -> Result<(Fleet, Open)> {
        let gh = GitHub::of(repo)?;
        let (fleet, open) = thread::scope(|scope| {
            let open = scope.spawn(|| gh.open());
            (Fleet::open_on(repo, gh.clone(), now), beside(open))
        });
        Ok((fleet?, open?))
    }

    /// Select the first eligible harness and model in kit order within the
    /// role's range. Matching model labels override model order within that
    /// harness alone; a role without an eligible candidate waits safely.
    pub fn cast_in(&self, role: &Role, labels: &[String], issue: Option<u64>) -> Result<Cast> {
        let range = self.range_in(role, issue)?;
        casting::preferred(labels, &range, &range)
            .map_err(|why| anyhow!("the role `{}`: {why}", role.name))
    }

    /// The role's permitted range in current kit order, with health and
    /// startability exclusions applied after its permissions are resolved.
    pub fn range_in(&self, role: &Role, issue: Option<u64>) -> Result<Vec<casting::Held>> {
        let kit = self.runnable_for(issue)?;
        let kit = kit.range();
        // Apply the role to the whole kit before excluding unhealthy
        // candidates. Temporary unavailability is never an unmatched roster
        // entry and cannot broaden the role's permissions.
        let whole = crate::kit::load()?.range();
        if let Some(why) = casting::nothing_safe(role, &whole) {
            return Err(NothingSafe(why).into());
        }
        let allowed = casting::range(role, &whole);
        let mut range = vec![];
        for available in &kit {
            if let Some(held) = allowed.iter().find(|h| h.harness == available.harness) {
                let mut held = held.clone();
                held.models.retain(|model| available.models.iter().any(|m| m.id == model.id));
                if !held.models.is_empty() || available.models.is_empty() {
                    range.push(held);
                }
            }
        }
        if range.is_empty() {
            return Err(NothingSafe(format!("the role `{}` has no currently eligible harness in its range; waiting for recovery", role.name)).into());
        }
        Ok(range)
    }

    /// The stock consult follows the same kit order as every other worker.
    pub fn stock_consult(&self, issue: Option<u64>) -> Result<Cast> {
        let kit = self.runnable_for(issue)?;
        let paused = crate::paused::read();
        if kit.harnesses.is_empty() && !paused.is_empty() {
            return Err(crate::paused::Held::every(&paused, self.now).into());
        }
        let named = kit.stock_consult();
        let labels: Vec<String> = named
            .into_iter()
            .flat_map(|named| {
                let mut labels = vec![format!("harness:{}", named.harness.key())];
                labels.extend(named.model.map(|model| format!("model:{model}")));
                labels
            })
            .collect();
        let range = kit.range();
        casting::preferred(&labels, &range, &range).map_err(|why| anyhow!("{why}"))
    }

    /// Kit order chooses the harness. Labels can only choose a model within it.
    pub fn cast(&self, labels: &[String], issue: Option<u64>) -> Result<Result<Cast, String>> {
        let kit = self.runnable_for(issue)?;
        let paused = crate::paused::read();
        let range = kit.range();
        if range.is_empty() && !paused.is_empty() {
            return Err(crate::paused::Held::every(&paused, self.now).into());
        }
        if range.is_empty() {
            let why = self
                .runnable
                .borrow()
                .as_ref()
                .map(|r| {
                    r.skipped.iter().map(|(_, why)| why.as_str()).collect::<Vec<_>>().join("; ")
                })
                .unwrap_or_default();
            return Ok(Err(format!("no eligible harness in the host kit: {why}")));
        }
        Ok(casting::preferred(labels, &range, &range))
    }

    /// One view's capability snapshot. Failed startability checks skip only
    /// that candidate; they never rewrite the kit or remote casting.
    pub fn runnable(&self) -> Result<crate::kit::Kit> {
        if self.runnable.borrow().is_none() {
            let mut kit = crate::kit::running()?.0;
            let launches = crate::launch::Launches::foreseen(&self.repo);
            let mut skipped = vec![];
            kit.harnesses.retain(|held| {
                match crate::role::startable(&launches, held.harness, "kit priority") {
                    Ok(()) => true,
                    Err(e) => {
                        skipped.push((held.harness, format!("{e:#}")));
                        false
                    }
                }
            });
            *self.runnable.borrow_mut() = Some(Runnable { kit, skipped });
        }
        let mut kit = self.runnable.borrow().as_ref().context("runnable kit")?.kit.clone();
        kit.harnesses.retain(|held| !self.avoiding.contains(&held.harness));
        Ok(kit)
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
        serde_json::json!({"order": crate::kit::load().map(|k| k.harnesses.iter().map(|h| h.harness).collect::<Vec<_>>()).unwrap_or_default(),
            "eligible": kit.map(|k| k.harnesses.iter().map(|h| h.harness).collect::<Vec<_>>()).unwrap_or_default(),
            "skipped": self.runnable.borrow().as_ref().map(|runnable| &runnable.skipped),
            "health": crate::health::show(), "refusals": *self.refusals.borrow(), "why": "first eligible harness in kit order; role limits and issue refusals still apply"})
    }

    pub(crate) fn invalidate_selection(&self) {
        *self.runnable.borrow_mut() = None;
    }

    pub fn role_selection(&self, role: &Role) -> serde_json::Value {
        let chosen = self.cast_in(role, &[], None).ok().map(|cast| cast.harness);
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
                    (
                        format!("{key} is unavailable on this host; see health evidence"),
                        format!("restore {key} to health; a successful recovery probe restores it"),
                    )
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

    /// What a worker nothing casts runs — the proposal of a precedent, and
    /// a landing whose casting the host kit does not hold: the host kit's
    /// first, or this host's switch.
    pub fn first(&self) -> Result<Cast> {
        self.cast(&[], None)?.map_err(|why| anyhow!("{why}: run `afkconvoy kit`"))
    }

    /// Every version tag the remote had when this view was opened, each with
    /// the commit it is on.
    pub fn version_tags(&self) -> &[(String, String)] {
        &self.listing.tags
    }

    /// The base branch's tip on the remote, its commits in this repo.
    pub fn base_tip(&self) -> Result<String> {
        let branch = &self.config.base_branch;
        let tip = self
            .listing
            .base
            .as_ref()
            .ok_or_else(|| anyhow!("the remote has no branch {branch:?}"))?;
        self.repo.bring(tip, &self.listing.base_ref)?;
        Ok(tip.clone())
    }

    /// Rebuilds the working set of `me` from `open` and the remote as this
    /// view was opened on it (`Fleet::read`) → it, and the read of the issues
    /// that follow a workflow it was assembled from. Reads only. A claimed
    /// issue that cannot be read costs only itself: it is taken for open,
    /// and noted in `unread`.
    pub fn rebuild(&mut self, open: &Open, me: Option<&InstanceId>) -> Result<(WorkingSet, Flows)> {
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
        // The list holds only open issues: a claim missing from it is asked
        // about, and is closed, gone — deleted or transferred — or unread.
        let (mut closed, mut deleted) = (vec![], vec![]);
        self.unread.clear();
        self.closers.clear();
        for claim in &self.scan.claims {
            if issues.iter().any(|issue| issue.number == claim.number) {
                continue;
            }
            match self.gh.issue(claim.number) {
                Ok(read) if read.is_closed() => {
                    closed.push(claim.number);
                    self.closers.extend(read.closed_by.map(|by| (claim.number, by)));
                }
                Ok(_) => {}
                Err(error) if gone(&error).is_some() => deleted.push(claim.number),
                Err(error) => self.unread.push((claim.number, error)),
            }
        }
        let base = self.base_tip()?;
        let mut landed = vec![];
        for claim in &self.scan.claims {
            if me.is_some_and(|me| claim.is_held_by(me))
                && !closed.contains(&claim.number)
                && cut_landing_of(&self.repo, &self.gh.tracker, &base, claim, prs)?.is_some()
            {
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
        let consults = self.stock_consults(open);
        let heartbeats = self.scan.beats();
        let (landings, slow_gate) = slow_gate::seen(self, &self.listing.records, &base)?;
        let cut = crate::cut::seen(self, &stages, issues, &base)?;
        self.proposals = precedent::open(self, &stages)?;
        let seen = Observed {
            issues,
            prs,
            claims: &self.scan.claims,
            heartbeats: &heartbeats,
            closed: &closed,
            gone: &deleted,
            kept: &kept,
            landed: &landed,
            declared: &declared,
            due: &stages.due(prs),
            flows: &stages.flow_labels(),
            landing: &stages.landing(&self.scan.claims),
            begun: &stages.begun(),
            patrols: &patrol::seen(self, &stages, &open.issues)?,
            proposals: &precedent::seen(self, &stages)?,
            waiting: &stages.waiting(self, open),
            landings: &landings,
            slow_gate: &slow_gate,
            consults: &consults,
            tracker: &self.gh.tracker,
            cut: cut.as_ref(),
        };
        let mut set = assemble(&seen, me, self.now, &self.config);
        open.issues.explain(&mut set.waiting_on_human);
        set.casting = crate::caster::status(self, &open.issues, stages.team().ok());
        match self.config.bound {
            None => self.cast_frontier(&mut set, &open.issues, &consults),
            Some(_) => self.cast_bound_frontier(&mut set, &open.issues, &stages),
        }
        Ok((set, stages))
    }

    /// Apply host priority and the shared model-casting eligibility to the
    /// unbound frontier. Only an enabled, missing model choice waits, at most
    /// ten minutes; human labels and ineligible harnesses never hold it back.
    fn cast_frontier(&self, set: &mut WorkingSet, open: &OpenIssues, consults: &[u64]) {
        let paused = crate::paused::read();
        let issues = &open.rows;
        let labels_of = |number: u64| -> Vec<String> {
            issues
                .iter()
                .find(|issue| issue.number == number)
                .map(|issue| issue.labels.clone())
                .unwrap_or_default()
        };
        let needed =
            |number| set.casting.iter().any(|status| status.number == number && status.needed);
        let waits = set.frontier.iter().any(|issue| needed(issue.number));
        let caster = if waits { crate::caster::last(self) } else { None };
        let mut standing = Standing {
            running_since: caster.as_ref().filter(|c| c.running(self.now)).map(|c| c.since),
            due: false,
            held: vec![],
            to_cast: set.casting.iter().filter(|s| s.needed).map(|s| s.number).collect(),
        };
        let frontier = std::mem::take(&mut set.frontier);
        for mut issue in frontier {
            let range = match self.runnable_for(Some(issue.number)) {
                Ok(kit) => kit.range(),
                Err(error) => {
                    set.excluded.push(Excluded {
                        number: issue.number,
                        title: issue.title,
                        reason: format!("{error:#}"),
                    });
                    continue;
                }
            };
            let labels = labels_of(issue.number);
            if consults.contains(&issue.number) {
                set.frontier.push(issue);
                continue;
            }
            let paused_held = (range.is_empty() && !paused.is_empty())
                .then(|| crate::paused::Held::every(&paused, self.now));
            if let Some(held) = paused_held {
                set.excluded.push(Excluded {
                    number: issue.number,
                    title: issue.title,
                    reason: held.0,
                });
                continue;
            }
            let held =
                casting::for_caster(issue.number, needed(issue.number), caster.as_ref(), self.now);
            if matches!(held, ForCaster::Wait | ForCaster::Due) {
                standing.due |= held == ForCaster::Due;
                standing.held.push(issue.number);
                set.excluded.push(Excluded {
                    number: issue.number,
                    title: issue.title,
                    reason: HELD_FOR_CASTER.to_string(),
                });
                continue;
            }
            match casting::preferred(&labels, &range, &range) {
                Ok(mut cast) => {
                    if held == ForCaster::Late {
                        cast.why = Why::Uncast;
                    }
                    issue.cast = Some(cast);
                    set.frontier.push(issue);
                }
                Err(why) => set.excluded.push(Excluded {
                    number: issue.number,
                    title: issue.title,
                    reason: format!("{why}: run `afkconvoy kit`"),
                }),
            }
        }
        set.excluded.sort_by_key(|excluded| excluded.number);
        if standing.running_since.is_some() || !standing.held.is_empty() {
            set.caster = Some(standing);
        }
    }

    /// Apply kit priority within each due role's constraints. Missing models
    /// across its workflow's selected harnesses wait only when casting is on;
    /// ranges with no safe candidate wait for recovery, independently.
    fn cast_bound_frontier(&self, set: &mut WorkingSet, open: &OpenIssues, stages: &Flows) {
        let Ok(kit) = self.runnable() else { return };
        let kit = kit.range();
        let Ok(team) = stages.team() else { return };
        let positions = stages.positions();
        let issues = &open.rows;
        let labels_of = |number: u64| -> Vec<String> {
            issues
                .iter()
                .find(|issue| issue.number == number)
                .map(|issue| issue.labels.clone())
                .unwrap_or_default()
        };
        let placed = |number: u64| positions.iter().find(|(at, ..)| *at == number);
        let needed =
            |number| set.casting.iter().any(|status| status.number == number && status.needed);
        let waits = set.frontier.iter().any(|issue| needed(issue.number));
        let caster = if waits { crate::caster::last(self) } else { None };
        let mut standing = Standing {
            running_since: caster.as_ref().filter(|c| c.running(self.now)).map(|c| c.since),
            due: false,
            held: vec![],
            to_cast: set.casting.iter().filter(|s| s.needed).map(|s| s.number).collect(),
        };
        let frontier = std::mem::take(&mut set.frontier);
        for mut issue in frontier {
            let labels = labels_of(issue.number);
            let Some((_, workflow, position)) = placed(issue.number) else {
                set.frontier.push(issue);
                continue;
            };
            // The stock consult is no role's, and has no range.
            let Some(role) = due_role(team, workflow, *position) else {
                set.frontier.push(issue);
                continue;
            };
            let ranges = casting::stage_ranges(team, workflow, &kit);
            // Held back while a harness it is cast to is paused, and while its
            // role can run nothing safely.
            if let Err(error) = self.cast_in(role, &labels, Some(issue.number)) {
                let reason = match (
                    error.downcast_ref::<crate::paused::Held>(),
                    error.downcast_ref::<NothingSafe>(),
                ) {
                    (Some(held), _) => Some(held.0.clone()),
                    (_, Some(nothing)) => Some(nothing.0.clone()),
                    _ => None,
                };
                if let Some(reason) = reason {
                    let title = issue.title;
                    set.excluded.push(Excluded { number: issue.number, title, reason });
                    continue;
                }
            }
            let held =
                casting::for_caster(issue.number, needed(issue.number), caster.as_ref(), self.now);
            if matches!(held, ForCaster::Wait | ForCaster::Due) {
                standing.due |= held == ForCaster::Due;
                standing.held.push(issue.number);
                set.excluded.push(Excluded {
                    number: issue.number,
                    title: issue.title,
                    reason: HELD_FOR_CASTER.to_string(),
                });
                continue;
            }
            match self.cast_in(role, &labels, Some(issue.number)).map_err(|e| format!("{e:#}")) {
                Ok(cast) => {
                    let mut cast = casting::unmatched_in(cast, &labels, &ranges);
                    if held == ForCaster::Late && cast.why == Why::RangeFirst {
                        cast.why = Why::Uncast;
                    }
                    issue.cast = Some(cast);
                    set.frontier.push(issue);
                }
                Err(why) => set.excluded.push(Excluded {
                    number: issue.number,
                    title: issue.title,
                    reason: format!("the role `{}`: {why}", role.name),
                }),
            }
        }
        set.excluded.sort_by_key(|excluded| excluded.number);
        if standing.running_since.is_some() || !standing.held.is_empty() {
            set.caster = Some(standing);
        }
    }

    /// The open issues of an unbound repo that are the stock consult's
    /// (`plan::Observed::consults`): escalated and unclaimed with the consult
    /// due, or claimed under a claim it was asked under.
    fn stock_consults(&self, open: &Open) -> Vec<u64> {
        if !self.config.stock_consult() {
            return vec![];
        }
        let consults = |issue: &&Issue| {
            let bodies = || open.issues.comments(issue.number).iter().map(|c| c.body.as_str());
            match self.scan.claim_of(issue.number) {
                // Whatever labels its answer left.
                Some(claim) => stock::asked_under(bodies(), &claim.id),
                None => issue.labels.contains(&self.config.escalate_label) && stock::due(bodies()),
            }
        };
        open.issues.rows.iter().filter(consults).map(|issue| issue.number).collect()
    }

    /// Whether the worker of `claim`, in an unbound repo, declared a verdict
    /// that stands: the latest on the issue, written since the claim was
    /// created — one from an earlier claim is not its own — and not answered
    /// by a worker started on the issue again (`stopped::standing_verdict`).
    /// What cannot be read says no: the claim then takes a slot.
    pub(crate) fn declared_since(&self, open: &Open, claim: &Claim) -> bool {
        let comments = open.issues.comments(claim.number);
        let Some(verdict) = latest_verdict(comments.iter().map(|c| c.body.as_str())) else {
            return false;
        };
        let Some(said) = comments.get(verdict.comment) else { return false };
        let since = said.at.zip(claim.since).is_some_and(|(at, since)| at >= since);
        // Only a verdict that is the claim's own is looked for in its worktree.
        let answered = || -> Option<Option<String>> {
            let worktree =
                Worktree::of_issue(&Source::of(&self.repo).ok()?, claim.number).ok()??;
            worktree.answered().ok()
        };
        since
            && answered().is_some_and(|answered| {
                standing_verdict(Some(verdict), Some(&said.url), answered.as_deref()).is_some()
            })
    }
}

/// The role of the worker due on an issue at `position` in `workflow`, one
/// of `team`'s: the stage's, its consult's or its landing's. `None` of the
/// stock consult, and where no worker is due.
pub fn due_role<'t>(
    team: &'t Team,
    workflow: &'t Workflow,
    position: Position<'t>,
) -> Option<&'t Role> {
    let work = match position {
        Position::Landing => workflow.land.as_ref(),
        Position::Consult(_, Consult::Stock, _) => None,
        at => at.worker(),
    };
    work.map(|work| team.role_of(work))
}
