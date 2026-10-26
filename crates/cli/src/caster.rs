//! Optional model casting within each stage's kit-selected harness. The kit
//! owns the host setting, default off; the same eligibility explains plan,
//! doctor and panel and controls the frontier's bounded wait. Casting adds
//! model labels and preserves every existing casting label.

use std::time::Duration;

use afkconvoy_core::casting::{
    self, CASTER_WORKTREE, Caster, Casting, Held, Model, ModelRun, ModelStage, ModelStatus,
    Offered, ToCast, caster_comment, caster_prompt, choice, holds,
};
use afkconvoy_core::role::workflow_of;
use afkconvoy_core::team::{Performer, Team};
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::worker::WorkerState;
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::dispatch::Agent;
use crate::fleet::Fleet;
use crate::gh::OpenIssues;
use crate::git::Repo;
use crate::kit::Kit;
use crate::orca::{Source, Worktree};

const ANSWER_TIMEOUT: Duration = Duration::from_secs(540);

/// Each stage's eligible range in kit order. Permissions are cut against the
/// whole kit before temporary health exclusions, exactly as at dispatch.
fn ranges(
    fleet: &Fleet,
    team: Option<&Team>,
    labels: &[String],
    number: u64,
) -> Vec<(String, Vec<Held>)> {
    let Some(team) = team else {
        return vec![(
            "worker".into(),
            fleet.runnable_for(Some(number)).map(|kit| kit.range()).unwrap_or_default(),
        )];
    };
    let Ok(workflow) = workflow_of(team, labels) else { return vec![] };
    workflow
        .stages()
        .iter()
        .filter_map(|stage| {
            let Performer::Role(work) = &stage.by else { return None };
            let role = team.role_of(work);
            Some((
                format!("{} ({})", stage.name, role.name),
                fleet.range_in(role, Some(number)).unwrap_or_default(),
            ))
        })
        .collect()
}

fn bodies(open: &OpenIssues, number: u64) -> Vec<&str> {
    open.comments(number).iter().map(|comment| comment.body.as_str()).collect()
}

/// Ranges offered to the caster: only the selected harness at each stage
/// still missing a model. Other harnesses and already matched stages are absent.
fn missing(ranges: &[(String, Vec<Held>)], labels: &[String]) -> Vec<(String, Vec<Held>)> {
    ranges
        .iter()
        .filter_map(|(stage, range)| {
            let held = range.first()?;
            (choice(range) && casting::labelled_model(labels, held).is_none())
                .then(|| (stage.clone(), vec![held.clone()]))
        })
        .collect()
}

/// One authoritative explanation of casting eligibility and model selection.
/// An unreadable kit is reported elsewhere and never enables casting.
pub fn status(fleet: &Fleet, open: &OpenIssues, team: Option<&Team>) -> Vec<ModelStatus> {
    let kit = crate::kit::load().unwrap_or_default();
    let whole = kit.range();
    let source = Source::of(&fleet.repo).ok();
    let mut statuses: Vec<ModelStatus> = open.rows.iter().map(|issue| {
        let labels = &issue.labels;
        let ranges = ranges(fleet, team, labels, issue.number);
        let bodies = bodies(open, issue.number);
        let ready = !labels.iter().any(|label| fleet.config.epic_labels.contains(label))
            && match team {
                Some(team) => workflow_of(team, labels).is_ok(),
                None => labels.contains(&fleet.config.ready_label),
            };
        let human = casting::human_casting(labels, &bodies);
        let needed = kit.cast && ready && casting::needs_model(labels, &bodies, &ranges);
        let why = if !kit.cast {
            "automatic casting is off on this host; enable with `afkconvoy kit --cast on`"
        } else if !ready {
            "this issue is outside the fleet's ready workflows"
        } else if human {
            "human casting labels are preserved; no recast"
        } else if needed {
            "a selected harness has multiple models and no matching model label"
        } else if ranges.iter().any(|(_, range)| choice(range)) {
            "the selected harnesses already have matching model labels; no recast"
        } else {
            "no model choice: each selected harness holds zero or one model"
        }.to_string();
        let stages = ranges.iter().map(|(stage, range)| {
            let selected = range.first();
            let cast = casting::preferred(labels, range, range).ok();
            let followed = cast.as_ref().is_some_and(|c| c.why == casting::Why::Casting);
            let why = if followed {
                "matching model: label followed within the selected harness"
            } else if labels.iter().any(|label| label.starts_with(casting::MODEL_LABEL)) {
                "no unique model: label matches this harness's eligible range; using its first model"
            } else {
                "no model: label; using the selected harness's first eligible model"
            }.to_string();
            let skipped = selected.into_iter().flat_map(|held| {
                whole.iter().filter(|h| h.harness == held.harness).flat_map(|h| &h.models)
                    .take_while(|model| cast.as_ref().and_then(|c| c.model.as_deref()) != Some(model.id.as_str()))
                    .map(|model| {
                        let reason = if !held.models.iter().any(|m| m.id == model.id) {
                            "unavailable or outside the role's range; see host health and roster"
                        } else { "the matching model: label takes priority" };
                        format!("{}: {reason}", model.id)
                    })
            }).collect();
            ModelStage { stage: stage.clone(), harness: cast.as_ref().map(|c| c.harness),
                model: cast.and_then(|c| c.model), followed, why, skipped }
        }).collect();
        let running = source.as_ref().filter(|_| fleet.scan.claim_of(issue.number).is_some()).and_then(|source| Worktree::of_issue(source, issue.number).ok().flatten())
            .filter(|worktree| worktree.state(fleet.now).is_ok_and(|state| state != WorkerState::Gone))
            .and_then(|worktree| {
                let harness = worktree.harness().ok().flatten()?;
                let model = std::fs::read_to_string(worktree.note("convoy-worker-model").ok()?).ok()
                    .map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                Some(ModelRun { harness, model })
            });
        ModelStatus { number: issue.number, needed, why, stages, running }
    }).collect();
    statuses.sort_by_key(|status| status.number);
    statuses
}

/// The last caster this host started for the repo, where it can be read: a
/// caster that cannot is none, and the tick starts one, which says why it
/// cannot.
pub fn last(fleet: &Fleet) -> Option<Caster> {
    let worktree = Worktree::named(&Source::of(&fleet.repo).ok()?, CASTER_WORKTREE).ok()??;
    let (since, mut given, previous) = worktree.caster().ok()??;
    // A ten-minute timeout belongs to that set of candidates. A switch or
    // recovery opens a different model choice without forgetting live work.
    if previous != context(fleet).ok()? {
        given.clear();
    }
    let live = worktree.state(fleet.now).ok()? != WorkerState::Gone;
    Some(Caster { since, given, live })
}

fn context(fleet: &Fleet) -> Result<String> {
    let range = fleet.runnable()?.range();
    let entries: Vec<_> = range.iter().map(|held| json!([held.harness, held.ids()])).collect();
    Ok(serde_json::to_string(&entries)?)
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
    agent.start(&worktree, first.harness, &command, None, fleet.now)?;
    worktree.record_caster(fleet.now, given, &context(fleet)?)?;
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
    // Different roles may allow overlapping subsets. Their union's first
    // appearance order is not the host's model priority.
    for mut held in kit.range() {
        let Some(allowed) = range.iter().find(|allowed| allowed.harness == held.harness) else {
            continue;
        };
        held.models.retain(|model| allowed.models.iter().any(|allowed| allowed.id == model.id));
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
        if !read.iter().any(|seen| seen.issue == casting.issue && seen.harness == casting.harness) {
            read.push(casting);
        }
    }
    read
}

/// Cast models only when this host explicitly enables it. The question is
/// bounded by each issue's selected harnesses, and the answer is checked again
/// against live labels and current kit priority before any label is added.
pub fn cast(repo: &Repo, now: u64) -> Result<Value> {
    if !crate::kit::load()?.cast {
        return Ok(
            json!({"cast": [], "uncast": [], "said": "automatic casting is off on this host; enable with `afkconvoy kit --cast on`"}),
        );
    }
    let (fleet, open) = Fleet::read(repo, now)?;
    let kit = fleet.runnable()?;
    let team = match fleet.config.bound {
        Some(_) => Some(crate::team::load(crate::team::From::Config, Some(repo))?.team),
        None => None,
    };
    let statuses = status(&fleet, &open.issues, team.as_ref());
    let numbers: Vec<_> = statuses.iter().filter(|s| s.needed).map(|s| s.number).collect();
    if numbers.is_empty() {
        return Ok(
            json!({"cast": [], "uncast": [], "said": "no issue needs a model: no model choice, existing matching labels, or human casting", "casting": statuses}),
        );
    }
    let mut issues = vec![];
    let mut each_ranges = vec![];
    for number in numbers {
        let read = fleet.gh.issue(number)?;
        let bodies = bodies(&open.issues, number);
        let eligible = ranges(&fleet, team.as_ref(), &read.labels, number);
        if !casting::needs_model(&read.labels, &bodies, &eligible) {
            continue;
        }
        let ranges = missing(&eligible, &read.labels);
        let stages = ranges.iter().map(|(by, range)| format!("{by}: {}", holds(range))).collect();
        issues.push(ToCast {
            number,
            title: read.title,
            body: read.body.unwrap_or_default(),
            stages,
        });
        each_ranges.push((number, ranges));
    }
    if issues.is_empty() {
        return Ok(json!({"cast": [], "uncast": [], "said": "the issues were cast meanwhile"}));
    }
    let offers = union(&each_ranges.iter().flat_map(|(_, r)| r.clone()).collect::<Vec<_>>());
    let how: Vec<String> = team
        .iter()
        .flat_map(|team| &team.roles)
        .filter_map(|role| {
            let how = role.roster.as_ref()?.cast.as_ref()?;
            Some(format!("{}: {how}", role.name))
        })
        .collect();
    let first = fleet.first()?;
    let launches = crate::launch::Launches::new(repo, true)?;
    let launch = launches.command(first.harness)?;
    crate::probe::announce(&launches.warned());
    let prompt = caster_prompt(&issues, &offered(&kit, team.as_ref(), &offers), &how);
    let question = if first.harness == afkconvoy_core::harness::Harness::Codex {
        crate::codex_launch::caster_question(
            &Worktree::of(repo)?,
            launch.as_str(),
            first.model.as_deref(),
            &prompt,
        )?
    } else {
        let kept = crate::kept::keep(first.harness, false)?.display().to_string();
        first.harness.prompt_question(launch.as_str(), &kept, first.model.as_deref(), &prompt)
    };
    let asked = crate::launch::ask_login_shell_within(&repo.dir, &question, ANSWER_TIMEOUT);
    let Some((true, answer)) = asked else {
        bail!("the host kit's first, `{}`, did not answer the caster", first.harness.key());
    };
    let castings = read_castings(&answer);
    let (mut cast, mut left) = (vec![], vec![]);
    // Priority and availability may have changed while the model answered.
    let current = Fleet::open(repo, now)?;
    for issue in issues {
        let number = issue.number;
        let requested = each_ranges
            .iter()
            .find(|(at, _)| *at == number)
            .map(|(_, r)| r.as_slice())
            .unwrap_or_default();
        let answers: Vec<_> = castings.iter().filter(|c| c.issue == number).collect();
        if answers.is_empty() {
            left.push(json!({"issue": number, "why": "its model named no casting for it"}));
        }
        for casting in answers {
            let now_labels = current.gh.issue(number)?.labels;
            let comments = current.gh.comments(number)?;
            let bodies: Vec<_> = comments.iter().map(|c| c.body.as_str()).collect();
            let live = missing(&ranges(&current, team.as_ref(), &now_labels, number), &now_labels);
            let allowed = union(&live);
            let asked = union(requested);
            let checked = (|| -> Result<Vec<String>> {
                if !crate::kit::load()?.cast {
                    bail!("automatic casting was disabled meanwhile");
                }
                if casting::human_casting(&now_labels, &bodies) {
                    bail!("human casting labels are preserved");
                }
                casting.labels(&asked).map_err(anyhow::Error::msg)?;
                casting.labels(&allowed).map_err(anyhow::Error::msg)
            })();
            let labels = match checked {
                Ok(labels) => labels,
                Err(why) => {
                    left.push(json!({"issue": number, "why": why.to_string()}));
                    continue;
                }
            };
            for label in &labels {
                current.gh.add_label(number, label)?;
            }
            current.gh.comment(number, &caster_comment(&labels, &casting.why))?;
            let model = labels.iter().find_map(|label| label.strip_prefix(casting::MODEL_LABEL));
            cast.push(json!({"issue": number, "harness": casting.harness, "model": model, "why": casting.why}));
        }
    }
    Ok(json!({"cast": cast, "uncast": left}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_s_answer_is_read_a_casting_a_line_whatever_is_around_it() {
        let answer = "Sure.\n```json\n{\"issue\":7,\"harness\":\"pi\",\"model\":\"a/b\",\"why\":\"small\"}\nnot json\n{\"issue\":8,\"harness\":\"codex\",\"model\":null,\"why\":\"big\"}\n{\"issue\":7,\"harness\":\"dsh\",\"why\":\"another harness\"}\n{\"issue\":7,\"harness\":\"pi\",\"model\":\"duplicate\",\"why\":\"ignored\"}\n```";
        let read = read_castings(answer);
        assert_eq!(read.len(), 3);
        assert_eq!((read[0].issue, read[0].harness.as_str()), (7, "pi"));
        assert_eq!(read[0].model.as_deref(), Some("a/b"));
        assert_eq!((read[1].issue, read[1].model.as_deref()), (8, None));
    }

    #[test]
    fn overlapping_stage_offers_follow_kit_model_order() {
        let kit: Kit = serde_json::from_value(json!({"harnesses":[{
            "harness":"claude-code", "command":"claude", "models":[
                {"model":"opus"}, {"model":"haiku"}, {"model":"sonnet"}
            ]
        }]}))
        .unwrap();
        let ranges = vec![
            (
                "first".into(),
                vec![Held {
                    harness: afkconvoy_core::harness::Harness::ClaudeCode,
                    models: vec!["haiku".into(), "sonnet".into()],
                }],
            ),
            (
                "second".into(),
                vec![Held {
                    harness: afkconvoy_core::harness::Harness::ClaudeCode,
                    models: vec!["opus".into(), "haiku".into()],
                }],
            ),
        ];
        let offered = offered(&kit, None, &union(&ranges));
        assert_eq!(
            offered.iter().filter_map(|o| o.model.as_deref()).collect::<Vec<_>>(),
            ["opus", "haiku", "sonnet"]
        );
    }
}
