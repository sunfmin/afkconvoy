//! One process's view of a repo: its checkout, its config, GitHub and the
//! remote's claims — and the rebuild that assembles a working set from them.
//!
//! A rebuild asks the remote once and GitHub a fixed number of times, side by
//! side: one listing of the base branch and every ref of the fleet
//! (`Repo::listing`), and what is open on GitHub (`GitHub::open`). Neither
//! grows with the issues in flight.

use afkconvoy_core::casting::{self, Cast, ForCaster, HELD_FOR_CASTER, Standing, Why, choice};
use afkconvoy_core::claims::{Claim, InstanceId};
use afkconvoy_core::config::Config;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::plan::{Excluded, Issue, Observed, WorkingSet, assemble, closing_pr};
use afkconvoy_core::position::Position;
use afkconvoy_core::stock;
use afkconvoy_core::stopped::standing_verdict;
use afkconvoy_core::team::{Consult, Role, Team, Workflow};
use afkconvoy_core::train::{Built, on_train};
use afkconvoy_core::verdict::latest_verdict;
use afkconvoy_core::worker::recorded_branches;
use anyhow::{Context, Result, anyhow};
use std::thread;

use crate::claims::Scan;
use crate::gh::{GitHub, Gone, Open, OpenIssues, beside, gone};
use crate::git::{Listing, Repo};
use crate::measure;
use crate::orca::{Source, Worktree};
use crate::patrol;
use crate::precedent;
use crate::settle::cut_landing_of;
use crate::slow_gate;
use crate::stage::Flows;
use crate::train::Train;

pub struct Fleet {
    pub repo: Repo,
    /// The repo's config, with the concurrency its checkout says on this
    /// host laid over it (`concurrency::config_of`).
    pub config: Config,
    /// The harness a switch on this host chose for the repo: every worker
    /// this host starts for it is of this one, whatever a role's range or an
    /// issue's casting says (`switch::chosen`).
    pub chosen: Option<Harness>,
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
    /// What this machine's train's worktree held when this view cut it again:
    /// what a tick's line says of it.
    pub cut_train: Option<Built>,
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
        let repo = repo.clone();
        // As this checkout's fleet instance goes by it on this host: read
        // anew at each view, so a concurrency said since holds now.
        let config = crate::concurrency::config_of(&repo)?;
        let chosen = crate::switch::chosen(&repo)?;
        let listing = repo.listing(&config.base_branch)?;
        let scan = Scan::among(repo.layout(), listing.records.clone());
        let gh = GitHub { dir: repo.dir.clone() };
        Ok(Fleet {
            repo,
            config,
            chosen,
            gh,
            scan,
            listing,
            now,
            escalated: vec![],
            forgotten: vec![],
            unread: vec![],
            closers: vec![],
            cut_train: None,
        })
    }

    /// The fleet's view, opened for a rebuild → it, and what is open on
    /// GitHub. The remote and GitHub do not depend on each other: they are
    /// asked side by side.
    pub fn read(repo: &Repo, now: u64) -> Result<(Fleet, Open)> {
        let gh = GitHub { dir: repo.dir.clone() };
        let (fleet, open) = thread::scope(|scope| {
            let open = scope.spawn(|| gh.open());
            (Fleet::open(repo, now), beside(open))
        });
        Ok((fleet?, open?))
    }

    /// What a worker of `role` runs for an issue labelled `labels`
    /// (`casting::cast_within`) within its range on this host — its roster
    /// entry cut to the host kit without the harnesses paused here — and
    /// this host's switch → it, or why it cannot be started. A casting to a
    /// paused harness the role may run holds the issue back (`paused::Held`);
    /// so does a range that holds nothing (`NothingSafe`): no worker of the
    /// role runs without its limits.
    pub fn cast_in(&self, role: &Role, labels: &[String]) -> Result<Cast> {
        let (kit, paused) = crate::kit::running()?;
        let kit = kit.range();
        let range = casting::range(role, &kit);
        if let Some(why) = casting::nothing_safe(role, &kit) {
            return Err(NothingSafe(why).into());
        }
        if self.chosen.is_none() {
            let allowed = |pause: &&casting::Paused| {
                let whole = crate::kit::load().map(|kit| kit.range()).unwrap_or_default();
                casting::range(role, &whole).iter().any(|held| held.harness == pause.harness)
            };
            if let Some(pause) = casting::paused_of(labels, &paused).filter(allowed) {
                return Err(crate::paused::Held::of(pause, self.now).into());
            }
        }
        casting::cast_within(labels, &range, &kit, self.chosen)
            .map_err(|why| anyhow!("the role `{}`: {why}", role.name))
    }

    /// The harness and model the stock consult runs: the host kit's
    /// (`Kit::stock_consult`), without the harnesses paused on this host —
    /// its first where what it names matches nothing held, `unmatched` — but
    /// this host's chosen harness, where a switch chose one the host kit
    /// holds, without the model of another; its first, `unmatched`, where the
    /// switch matches nothing held. Every harness paused holds it back
    /// (`paused::Held`).
    pub fn stock_consult(&self) -> Result<Cast> {
        let (kit, paused) = crate::kit::running()?;
        if kit.harnesses.is_empty() && !paused.is_empty() {
            return Err(crate::paused::Held::every(&paused, self.now).into());
        }
        let named =
            kit.stock_consult().context("the host kit holds no harness: run `afkconvoy kit`")?;
        let (why, unmatched) = match (&kit.stock_consult, kit.stock_unmatched()) {
            (Some(said), Some(_)) => (Why::Unmatched, Some(format!("`{}`", said.harness.key()))),
            (Some(_), None) => (Why::Casting, None),
            (None, _) => (Why::First, None),
        };
        let cast = Cast { harness: named.harness, model: named.model, why, unmatched };
        Ok(match self.chosen {
            Some(chosen) if kit.command(chosen).is_none() => {
                let first = kit.first().context("the host kit holds no harness")?;
                let unmatched = Some(format!("this host's switch to `{}`", chosen.key()));
                Cast { harness: first.harness, model: first.model, why: Why::Unmatched, unmatched }
            }
            Some(chosen) if chosen != cast.harness => {
                Cast { harness: chosen, model: None, why: Why::Switch, unmatched: None }
            }
            _ => cast,
        })
    }

    /// What the worker of an unbound repo's issue labelled `labels` runs
    /// (`casting::cast`) within the host kit without the harnesses paused on
    /// this host, and this host's switch → it: a casting or a switch that
    /// matches nothing held runs the host kit's first. A casting to be cast
    /// again (`recast`) is taken for none. A casting to a paused harness, or
    /// a host kit every harness of which is paused, holds the issue back
    /// (`paused::Held`); any other error is the host kit's.
    pub fn cast(&self, labels: &[String], recast: bool) -> Result<Result<Cast, String>> {
        let (kit, paused) = crate::kit::running()?;
        let range = kit.range();
        let labels = if recast { casting::without_casting(labels) } else { labels.to_vec() };
        if let Some(pause) = casting::paused_of(&labels, &paused).filter(|_| self.chosen.is_none())
        {
            return Err(crate::paused::Held::of(pause, self.now).into());
        }
        if range.is_empty() && !paused.is_empty() {
            return Err(crate::paused::Held::every(&paused, self.now).into());
        }
        Ok(casting::cast(&labels, &range, self.chosen))
    }

    /// Whether issue `number`, labelled `labels`, is to be cast again: a
    /// caster cast it to a harness paused on this host, or to what the host
    /// kit holds nothing by, and no worker has started on it
    /// (`casting::to_recast`). Its comments are read only where its casting
    /// names one of those.
    pub fn recast(&self, number: u64, labels: &[String]) -> Result<bool> {
        let (kit, paused) = crate::kit::running()?;
        let kit = kit.range();
        if casting::paused_of(labels, &paused).is_none() && !casting::names_nothing(labels, &kit) {
            return Ok(false);
        }
        Ok(crate::caster::recast(labels, &self.gh.comments(number)?, &paused, &kit))
    }

    /// What a worker nothing casts runs — the proposal of a precedent, and
    /// a landing whose casting the host kit does not hold: the host kit's
    /// first, or this host's switch.
    pub fn first(&self) -> Result<Cast> {
        self.cast(&[], false)?.map_err(|why| anyhow!("{why}: run `afkconvoy kit`"))
    }

    /// The base branch's tip on the remote, its commits in this repo.
    pub fn base_tip(&self) -> Result<String> {
        let branch = &self.config.base_branch;
        self.repo
            .fetched(&format!("refs/heads/{branch}"))?
            .ok_or_else(|| anyhow!("the remote has no branch {branch:?}"))
    }

    /// Rebuilds the working set of `me` from `open` and the remote as this
    /// view was opened on it (`Fleet::read`) → it, and the read of the issues
    /// that follow a workflow it was assembled from. Reads only. A claimed
    /// issue that cannot be read costs only itself: it is taken for open,
    /// and noted in `unread`.
    pub fn rebuild(&mut self, open: &Open, me: Option<&InstanceId>) -> Result<(WorkingSet, Flows)> {
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
        // What is on the train is read off the train's own commits.
        let train = Train::listed(&self.repo, &self.config, &self.listing)?;
        let joins = train.joins(&self.repo)?;
        // …and what a cut landing left open, off the base branch.
        let mut landed = vec![];
        for claim in &self.scan.claims {
            if me.is_some_and(|me| claim.is_held_by(me))
                && !closed.contains(&claim.number)
                && cut_landing_of(&self.repo, &train.base, claim, prs)?.is_some()
            {
                landed.push(claim.number);
            }
        }
        let joined: Vec<u64> = on_train(&joins, prs).iter().map(|join| join.issue).collect();
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
                Some(_) => stages.declared(claim.number, &claim.id),
                None => self.declared_since(open, claim),
            };
            if under {
                declared.push(claim.number);
            }
        }
        let consults = self.stock_consults(open);
        let heartbeats = self.scan.beats();
        let (landings, slow_gate) = slow_gate::seen(self, &self.listing.records, &train.base)?;
        let seen = Observed {
            issues,
            prs,
            claims: &self.scan.claims,
            heartbeats: &heartbeats,
            closed: &closed,
            gone: &deleted,
            joined: &joined,
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
            idle_train: if train.tip == train.base { self.train_built() } else { None },
            consults: &consults,
        };
        let mut set = assemble(&seen, me, self.now, &self.config);
        match self.config.bound {
            None => self.cast_frontier(&mut set, &open.issues, &consults),
            Some(_) => self.cast_bound_frontier(&mut set, &open.issues, &stages),
        }
        Ok((set, stages))
    }

    /// Says of each issue on an unbound repo's frontier what its worker
    /// would run, and why; one whose casting the host kit does not hold runs
    /// its first, `unmatched`, naming the label. The stock consult's runs
    /// what the host kit names for it, and is cast by nothing. A host kit
    /// that cannot be read casts nothing: a start refuses it, saying so.
    ///
    /// An uncast issue with a choice to make is held back for the caster
    /// while one runs, and starts one where none does
    /// (`casting::for_caster`); one a caster had and did not cast runs the
    /// host kit's first.
    ///
    /// The harnesses paused on this host are left out of the host kit: an
    /// issue a caster cast to one, which no worker started on, is cast again
    /// as one with no casting; one a human cast to it, or that a worker
    /// started on, is held back while it is paused, and so is every issue
    /// while every harness is.
    fn cast_frontier(&self, set: &mut WorkingSet, open: &OpenIssues, consults: &[u64]) {
        let Ok((kit, paused)) = crate::kit::running() else { return };
        let range = kit.range();
        let issues = &open.rows;
        let labels_of = |number: u64| -> Vec<String> {
            let Some(issue) = issues.iter().find(|issue| issue.number == number) else {
                return vec![];
            };
            match crate::caster::recast(&issue.labels, open.comments(number), &paused, &range) {
                true => casting::without_casting(&issue.labels),
                false => issue.labels.clone(),
            }
        };
        // This host's caster is asked after only where an issue waits for one.
        let waits = set.frontier.iter().any(|issue| {
            let labels = labels_of(issue.number);
            casting::for_caster(issue.number, &labels, choice(&range), self.chosen, None, 0)
                != ForCaster::Dispatch
        });
        let caster = if waits { crate::caster::last(&self.repo, self.now) } else { None };
        let mut standing = Standing {
            running_since: caster.as_ref().filter(|c| c.running(self.now)).map(|c| c.since),
            due: false,
            held: vec![],
            to_cast: crate::caster::to_cast(&self.config, open, &paused, &range),
        };
        let frontier = std::mem::take(&mut set.frontier);
        for mut issue in frontier {
            let labels = labels_of(issue.number);
            if consults.contains(&issue.number) {
                set.frontier.push(issue);
                continue;
            }
            let paused_held = match casting::paused_of(&labels, &paused) {
                _ if self.chosen.is_some() => None,
                Some(pause) => Some(crate::paused::Held::of(pause, self.now)),
                None if range.is_empty() && !paused.is_empty() => {
                    Some(crate::paused::Held::every(&paused, self.now))
                }
                None => None,
            };
            if let Some(held) = paused_held {
                set.excluded.push(Excluded {
                    number: issue.number,
                    title: issue.title,
                    reason: held.0,
                });
                continue;
            }
            let held = casting::for_caster(
                issue.number,
                &labels,
                choice(&range),
                self.chosen,
                caster.as_ref(),
                self.now,
            );
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
            match casting::cast(&labels, &range, self.chosen) {
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

    /// Says of each issue on a bound repo's frontier what the worker due on
    /// it runs, and why (`casting::cast_within`): its casting where its
    /// role's range holds it, the range's first where not — `unmatched` where
    /// none of its workflow's stages' ranges holds it (`casting::unmatched_in`).
    /// One at a stage whose role can run nothing safely on this host waits,
    /// excluded with why (`casting::nothing_safe`). The stock consult's runs
    /// what the host kit names for it, and is cast by nothing. A host kit or
    /// a team that cannot be read casts nothing: a start refuses it.
    ///
    /// An uncast issue one of whose stages has a choice to make is held back
    /// for the caster as in an unbound repo (`cast_frontier`).
    ///
    /// The harnesses paused on this host are left out of every range, as in
    /// an unbound repo: an issue a caster cast to one is cast again, and one
    /// a human cast to it is held back while it is paused (`cast_in`).
    fn cast_bound_frontier(&self, set: &mut WorkingSet, open: &OpenIssues, stages: &Flows) {
        let Ok((kit, paused)) = crate::kit::running() else { return };
        let kit = kit.range();
        let Ok(team) = stages.team() else { return };
        let positions = stages.positions();
        let issues = &open.rows;
        let labels_of = |number: u64| -> Vec<String> {
            let Some(issue) = issues.iter().find(|issue| issue.number == number) else {
                return vec![];
            };
            match crate::caster::recast(&issue.labels, open.comments(number), &paused, &kit) {
                true => casting::without_casting(&issue.labels),
                false => issue.labels.clone(),
            }
        };
        let placed = |number: u64| positions.iter().find(|(at, ..)| *at == number);
        let choice_of = |number: u64| {
            placed(number).is_some_and(|(_, workflow, _)| {
                casting::any_choice(&casting::stage_ranges(team, workflow, &kit))
            })
        };
        let waits = set.frontier.iter().any(|issue| {
            let choice = choice_of(issue.number);
            casting::for_caster(
                issue.number,
                &labels_of(issue.number),
                choice,
                self.chosen,
                None,
                0,
            ) != ForCaster::Dispatch
        });
        let caster = if waits { crate::caster::last(&self.repo, self.now) } else { None };
        let mut standing = Standing {
            running_since: caster.as_ref().filter(|c| c.running(self.now)).map(|c| c.since),
            due: false,
            held: vec![],
            to_cast: crate::caster::to_cast_bound(&self.config, team, open, &kit, &paused),
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
            if let Err(error) = self.cast_in(role, &labels) {
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
            let held = casting::for_caster(
                issue.number,
                &labels,
                casting::any_choice(&ranges),
                self.chosen,
                caster.as_ref(),
                self.now,
            );
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
            let range = casting::range(role, &kit);
            match casting::cast_within(&labels, &range, &kit, self.chosen) {
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

    /// What the last landing measured of this machine's train's worktree;
    /// `None` when there is none here, no landing measured it, or orca or the
    /// worktree cannot be read — nothing is cut on what is not known.
    fn train_built(&self) -> Option<Built> {
        let worktree = Worktree::of_train(&Source::of(&self.repo).ok()?).ok()??;
        measure::measured(&Repo::new(worktree.path)).ok()?
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
    fn declared_since(&self, open: &Open, claim: &Claim) -> bool {
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
