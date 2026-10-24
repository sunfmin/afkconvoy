//! The caster (docs/adr/0020): a worker in no role that casts, in one
//! session, every issue of the repo that has a choice to make and no
//! casting — in a bound repo, one of whose stages' ranges leaves one. A tick starts one in the caster's worktree, where it runs
//! `afkconvoy cast`; a human runs that in the foreground. What is held back
//! for it, and for how long, is the core's (`casting::for_caster`).

use std::time::Duration;

use afkconvoy_core::casting::{
    self, CASTER_WORKTREE, Caster, Casting, Held, Model, Offered, Paused, ToCast, any_choice,
    caster_comment, caster_prompt, choice, held_by_none, holds, is_casting, paused_of,
    stage_ranges, to_cast as is_to_cast, to_recast, uncast, without_casting,
};
use afkconvoy_core::config::Config;
use afkconvoy_core::role::workflow_of;
use afkconvoy_core::team::Team;
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::worker::WorkerState;
use afkconvoy_core::worker::recorded_branches;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::dispatch::Agent;
use crate::fleet::Fleet;
use crate::gh::{Comment, OpenIssues};
use crate::git::Repo;
use crate::kit::Kit;
use crate::orca::{Source, Worktree};

/// How long the caster's model is given to answer: less than the issues are
/// held back for it.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(540);

/// The open issues of an unbound repo a caster started now is given, lowest
/// number first: those with no casting (`casting::to_cast`), and those to be
/// cast again because a caster cast them to a harness now `paused`, or to
/// what the host kit `kit` holds nothing by (`recast`).
pub fn to_cast(config: &Config, open: &OpenIssues, paused: &[Paused], kit: &[Held]) -> Vec<u64> {
    let (ready, epics) = (&config.ready_label, &config.epic_labels);
    let mut numbers: Vec<u64> = open
        .rows
        .iter()
        .filter(|issue| {
            is_to_cast(&issue.labels, ready, epics)
                || (is_to_cast(&without_casting(&issue.labels), ready, epics)
                    && recast(&issue.labels, open.comments(issue.number), paused, kit))
        })
        .map(|issue| issue.number)
        .collect();
    numbers.sort_unstable();
    numbers
}

/// The open issues of a bound repo a caster started now is given, lowest
/// number first: each follows one workflow of `team`, carries no epic label,
/// and has no casting or is to be cast again (`recast`) — and one of its
/// workflow's stages' ranges on a host whose host kit holds `kit` leaves a
/// choice to make.
pub fn to_cast_bound(
    config: &Config,
    team: &Team,
    open: &OpenIssues,
    kit: &[Held],
    paused: &[Paused],
) -> Vec<u64> {
    let mut numbers: Vec<u64> = open
        .rows
        .iter()
        .filter(|issue| {
            let labels = &issue.labels;
            (uncast(labels)
                || (uncast(&without_casting(labels))
                    && recast(labels, open.comments(issue.number), paused, kit)))
                && !labels.iter().any(|label| config.epic_labels.contains(label))
                && workflow_of(team, labels)
                    .is_ok_and(|workflow| any_choice(&stage_ranges(team, workflow, kit)))
        })
        .map(|issue| issue.number)
        .collect();
    numbers.sort_unstable();
    numbers
}

/// Whether an issue labelled `labels`, with `comments`, is to be cast again:
/// a caster cast it to a harness now `paused`, or to what the host kit `kit`
/// holds nothing by, and no worker has started on it — none has recorded its
/// branch on it (`casting::to_recast`).
pub fn recast(labels: &[String], comments: &[Comment], paused: &[Paused], kit: &[Held]) -> bool {
    let bodies = || comments.iter().map(|comment| comment.body.as_str());
    to_recast(labels, bodies(), !recorded_branches(bodies()).is_empty(), paused, kit)
}

/// The last caster this host started for the repo, where it can be read: a
/// caster that cannot is none, and the tick starts one, which says why it
/// cannot.
pub fn last(repo: &Repo, now: u64) -> Option<Caster> {
    let worktree = Worktree::named(&Source::of(repo).ok()?, CASTER_WORKTREE).ok()??;
    let (since, given) = worktree.caster().ok()??;
    let live = worktree.state(now).ok()? != WorkerState::Gone;
    Some(Caster { since, given, live })
}

/// Starts a caster on `given`, in the caster's worktree — cut at the base
/// branch's tip first when this machine has none — ending whatever ran there
/// before. It runs `afkconvoy cast` under the host kit's first harness's
/// worker environment, and its terminal ends with it.
pub fn start(fleet: &Fleet, agent: &Agent, given: &[u64]) -> Result<Outcome> {
    let source = Source::of(&fleet.repo)?;
    let worktree = match Worktree::named(&source, CASTER_WORKTREE)? {
        Some(worktree) => worktree,
        None => Worktree::cut(&source, CASTER_WORKTREE, &fleet.base_tip()?, None)?,
    };
    let first = fleet.first()?;
    let command = crate::own_command().fixed(" cast; exit").own();
    agent.start(&worktree, first.harness, &command, fleet.now)?;
    worktree.record_caster(fleet.now, given)?;
    Ok(Outcome::Done)
}

/// Each harness and model `range` holds, in order, by its id, with what
/// `team`'s roster says each is good at — of an entry naming it by any of its
/// names — or else the host kit: a harness held with no model is one, on its
/// command.
fn offered(kit: &Kit, team: Option<&Team>, range: &[Held]) -> Vec<Offered> {
    let rostered = |harness, model: Option<&Model>| {
        let entries = team.into_iter().flat_map(|team| &team.roles);
        let allowed = entries.filter_map(|role| role.roster.as_ref()).flat_map(|e| &e.run);
        let names = |allowed: &Option<String>| match (allowed, model) {
            (None, None) => true,
            (Some(name), Some(model)) => {
                Held { harness, models: vec![model.clone()] }.model(name).is_ok()
            }
            _ => false,
        };
        allowed
            .filter(|allowed| allowed.harness == harness && names(&allowed.model))
            .find_map(|allowed| allowed.good_at.clone())
    };
    let seen = |harness, model: &Model| {
        let held = kit.harnesses.iter().find(|held| held.harness == harness)?;
        held.models.iter().find(|seen| seen.model == model.id)?.good_at.clone()
    };
    let mut offered = vec![];
    for held in range {
        if held.models.is_empty() {
            let good_at = rostered(held.harness, None);
            offered.push(Offered { harness: held.harness, model: None, good_at });
        }
        for model in &held.models {
            let good_at = rostered(held.harness, Some(model)).or_else(|| seen(held.harness, model));
            let model = Some(model.id.clone());
            offered.push(Offered { harness: held.harness, model, good_at });
        }
    }
    offered
}

/// Every harness and model any of `ranges` holds, in the order each is
/// first held.
fn union(ranges: &[(String, Vec<Held>)]) -> Vec<Held> {
    let mut all: Vec<Held> = vec![];
    for (_, range) in ranges {
        for held in range {
            match all.iter_mut().find(|had| had.harness == held.harness) {
                Some(had) => {
                    for model in &held.models {
                        if !had.models.contains(model) {
                            had.models.push(model.clone());
                        }
                    }
                }
                None => all.push(held.clone()),
            }
        }
    }
    all
}

/// The castings a caster's model answered in `answer`: one JSON object a
/// line, whatever it printed around them. The first said of an issue stands.
fn read_castings(answer: &str) -> Vec<Casting> {
    let mut read: Vec<Casting> = vec![];
    for line in answer.lines() {
        let line = line.trim().trim_matches('`').trim();
        let Ok(casting) = serde_json::from_str::<Casting>(line) else { continue };
        if !read.iter().any(|seen| seen.issue == casting.issue) {
            read.push(casting);
        }
    }
    read
}

/// `afkconvoy cast`: sends the host kit's first, in one prompt, every issue
/// of this repo that is to be cast, the harnesses and models they may run in
/// order with what each is good at — in a bound repo, each issue's stages
/// with their ranges and how the roster says to cast — then puts on each
/// issue the casting it answered, where its range holds it (in a bound repo,
/// one of its stages' ranges), with a comment saying why. An issue that was
/// cast in the meantime is left as it is. The host kit is the one this host
/// can start now, without its paused harnesses; an issue a caster cast to
/// one of them is cast again, its casting taken off, and its comment says
/// why.
///
///   {"cast": [{issue, harness, model, why}], "uncast": [{issue, why}]}
pub fn cast(repo: &Repo, now: u64) -> Result<Value> {
    let (fleet, open) = Fleet::read(repo, now)?;
    let (kit, paused) = crate::kit::running()?;
    let range = kit.range();
    let team = match fleet.config.bound {
        Some(_) => Some(crate::team::load(crate::team::From::Config, Some(repo))?.team),
        None => None,
    };
    // Each issue's stages' ranges, in a bound repo.
    let ranges_of = |labels: &[String]| -> Vec<(String, Vec<Held>)> {
        let Some(team) = &team else { return vec![] };
        workflow_of(team, labels).map_or(vec![], |workflow| stage_ranges(team, workflow, &range))
    };
    let numbers = match &team {
        None if !choice(&range) => {
            return Ok(json!({"cast": [], "uncast": [],
                "said": "the host kit holds one harness and model: there is no choice to make"}));
        }
        None => to_cast(&fleet.config, &open.issues, &paused, &range),
        Some(team) => to_cast_bound(&fleet.config, team, &open.issues, &range, &paused),
    };
    if numbers.is_empty() {
        return Ok(json!({"cast": [], "uncast": [], "said": "no issue is to be cast"}));
    }
    let mut issues = vec![];
    let mut each_ranges = vec![];
    for &number in &numbers {
        let read = fleet.gh.issue(number)?;
        let ranges = ranges_of(&read.labels);
        let stages = ranges.iter().map(|(by, range)| format!("{by}: {}", holds(range))).collect();
        issues.push(ToCast {
            number,
            title: read.title,
            body: read.body.unwrap_or_default(),
            stages,
        });
        each_ranges.push((number, ranges));
    }
    let offers = match &team {
        Some(_) => union(&each_ranges.iter().flat_map(|(_, r)| r.clone()).collect::<Vec<_>>()),
        None => range.clone(),
    };
    let how: Vec<String> = team
        .iter()
        .flat_map(|team| &team.roles)
        .filter_map(|role| {
            let how = role.roster.as_ref()?.cast.as_ref()?;
            Some(format!("{}: {how}", role.name))
        })
        .collect();
    let first = fleet.first()?;
    let launch = kit.command(first.harness).context("the host kit holds no harness")?;
    let kept = crate::kept::keep(first.harness, false)?.display().to_string();
    let prompt = caster_prompt(&issues, &offered(&kit, team.as_ref(), &offers), &how);
    let question = first.harness.prompt_question(launch, &kept, first.model.as_deref(), &prompt);
    let asked = crate::launch::ask_login_shell_within(&repo.dir, &question, ANSWER_TIMEOUT);
    let Some((true, answer)) = asked else {
        bail!("the host kit's first, `{}`, did not answer the caster", first.harness.key());
    };
    let castings = read_castings(&answer);
    let (mut cast, mut left) = (vec![], vec![]);
    for number in numbers {
        let Some(casting) = castings.iter().find(|casting| casting.issue == number) else {
            left.push(json!({"issue": number, "why": "its model named no casting for it"}));
            continue;
        };
        // In a bound repo, one of its stages' ranges is to hold it.
        let ranges = each_ranges.iter().find(|(at, _)| *at == number).map(|(_, r)| r.as_slice());
        let labels = match (casting.labels(&offers), ranges) {
            (Ok(labels), Some(ranges)) if team.is_some() => {
                match held_by_none(&labels, ranges, None) {
                    Some(unheld) => Err(unheld.why),
                    None => Ok(labels),
                }
            }
            (labels, _) => labels,
        };
        let labels = match labels {
            Ok(labels) => labels,
            Err(why) => {
                left.push(json!({"issue": number, "why": why}));
                continue;
            }
        };
        // A human may have cast it while the model was asked. One cast to a
        // paused harness, or to what the host kit holds nothing by, is cast
        // again: what was read of it said so.
        let now_labels = fleet.gh.issue(number)?.labels;
        let again = match paused_of(&now_labels, &paused) {
            Some(pause) => Some(crate::paused::said(pause, now)),
            None => casting::unheld(&now_labels, &range).map(|unheld| unheld.why),
        };
        if !uncast(&now_labels) && again.is_none() {
            left.push(json!({"issue": number, "why": "it was cast meanwhile"}));
            continue;
        }
        for label in now_labels.iter().filter(|label| is_casting(label)) {
            fleet.gh.remove_label(number, label)?;
        }
        for label in &labels {
            fleet.gh.add_label(number, label)?;
        }
        let mut said = caster_comment(&labels, &casting.why);
        if let Some(why) = again {
            let was = now_labels.iter().filter(|label| is_casting(label));
            let was: Vec<String> = was.map(|label| format!("`{label}`")).collect();
            said.push_str(&format!("\n\nCast again, in place of {}: {why}", was.join(" and ")));
        }
        fleet.gh.comment(number, &said)?;
        let model = labels.iter().find_map(|label| label.strip_prefix(casting::MODEL_LABEL));
        cast.push(json!({"issue": number, "harness": casting.harness,
            "model": model, "why": casting.why}));
    }
    Ok(json!({"cast": cast, "uncast": left}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_s_answer_is_read_a_casting_a_line_whatever_is_around_it() {
        let answer = "Sure.\n```json\n{\"issue\": 7, \"harness\": \"pi\", \"model\": \"a/b\", \
             \"why\": \"small\"}\nnot json\n{\"issue\": 8, \"harness\": \"codex\", \"model\": \
             null, \"why\": \"big\"}\n{\"issue\": 7, \"harness\": \"dsh\", \"why\": \"again\"}\n```";
        let read = read_castings(answer);
        assert_eq!(read.len(), 2);
        assert_eq!((read[0].issue, read[0].harness.as_str()), (7, "pi"));
        assert_eq!(read[0].model.as_deref(), Some("a/b"));
        assert_eq!((read[1].issue, read[1].model.as_deref()), (8, None));
    }
}
