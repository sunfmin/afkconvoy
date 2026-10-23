//! The caster (docs/adr/0020): a worker in no role that casts, in one
//! session, every issue of an unbound repo that has a choice to make and no
//! casting. A tick starts one in the caster's worktree, where it runs
//! `afkconvoy cast`; a human runs that in the foreground. What is held back
//! for it, and for how long, is the core's (`casting::for_caster`).

use std::time::Duration;

use afkconvoy_core::casting::{
    CASTER_WORKTREE, Caster, Casting, Offered, ToCast, caster_prompt, choice,
    to_cast as is_to_cast, uncast,
};
use afkconvoy_core::config::Config;
use afkconvoy_core::plan::Issue;
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::worker::WorkerState;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::dispatch::Agent;
use crate::fleet::Fleet;
use crate::git::Repo;
use crate::kit::Kit;
use crate::orca::{Source, Worktree};

/// How long the caster's model is given to answer: less than the issues are
/// held back for it.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(540);

/// The open issues of an unbound repo a caster started now is given, lowest
/// number first (`casting::to_cast`).
pub fn to_cast(config: &Config, issues: &[Issue]) -> Vec<u64> {
    let mut numbers: Vec<u64> = issues
        .iter()
        .filter(|issue| is_to_cast(&issue.labels, &config.ready_label, &config.epic_labels))
        .map(|issue| issue.number)
        .collect();
    numbers.sort_unstable();
    numbers
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

/// Each harness and model of the host kit, in order, with what it says
/// each is good at: a harness held with no model is one, on its command.
fn offered(kit: &Kit) -> Vec<Offered> {
    let mut offered = vec![];
    for held in &kit.harnesses {
        if held.models.is_empty() {
            offered.push(Offered { harness: held.harness, model: None, good_at: None });
        }
        for seen in &held.models {
            offered.push(Offered {
                harness: held.harness,
                model: Some(seen.model.clone()),
                good_at: seen.good_at.clone(),
            });
        }
    }
    offered
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

/// What a caster puts on one issue as its comment.
fn comment(casting: &Casting, labels: &[String]) -> String {
    let named: Vec<String> = labels.iter().map(|label| format!("`{label}`")).collect();
    format!("Cast {}: {}", named.join(" and "), casting.why.trim())
}

/// `afkconvoy cast`: sends the host kit's first, in one prompt, every issue
/// of this unbound repo that is to be cast, the host kit's harnesses and
/// models in order with what each is good at — then puts on each issue the
/// casting it answered, where the host kit holds it, with a comment saying
/// why. An issue that was cast in the meantime is left as it is.
///
///   {"cast": [{issue, harness, model, why}], "uncast": [{issue, why}]}
pub fn cast(now: u64) -> Result<Value> {
    let (fleet, open) = Fleet::read(now)?;
    if fleet.config.bound.is_some() {
        bail!("a bound repo's issues are cast within its roster, which no caster reads yet");
    }
    let kit = crate::kit::load()?;
    let range = kit.range();
    if !choice(&range) {
        return Ok(json!({"cast": [], "uncast": [],
            "said": "the host kit holds one harness and model: there is no choice to make"}));
    }
    let numbers = to_cast(&fleet.config, &open.issues.rows);
    if numbers.is_empty() {
        return Ok(json!({"cast": [], "uncast": [], "said": "no issue is to be cast"}));
    }
    let mut issues = vec![];
    for &number in &numbers {
        let read = fleet.gh.issue(number)?;
        issues.push(ToCast { number, title: read.title, body: read.body.unwrap_or_default() });
    }
    let first = fleet.first()?;
    let launch = kit.command(first.harness).context("the host kit holds no harness")?;
    let kept = crate::kept::keep(first.harness, false)?.display().to_string();
    let prompt = caster_prompt(&issues, &offered(&kit));
    let Some(question) =
        first.harness.prompt_question(launch, &kept, first.model.as_deref(), &prompt)
    else {
        bail!(
            "the host kit's first, `{}`, can be sent a prompt only in a worker's terminal: put a \
             harness first that can be, with `afkconvoy kit`",
            first.harness.key()
        );
    };
    let Some((true, answer)) = crate::launch::ask_login_shell_within(&question, ANSWER_TIMEOUT)
    else {
        bail!("the host kit's first, `{}`, did not answer the caster", first.harness.key());
    };
    let castings = read_castings(&answer);
    let (mut cast, mut left) = (vec![], vec![]);
    for number in numbers {
        let Some(casting) = castings.iter().find(|casting| casting.issue == number) else {
            left.push(json!({"issue": number, "why": "its model named no casting for it"}));
            continue;
        };
        let labels = match casting.labels(&range) {
            Ok(labels) => labels,
            Err(why) => {
                left.push(json!({"issue": number, "why": why}));
                continue;
            }
        };
        // A human may have cast it while the model was asked.
        if !uncast(&fleet.gh.issue(number)?.labels) {
            left.push(json!({"issue": number, "why": "it was cast meanwhile"}));
            continue;
        }
        for label in &labels {
            fleet.gh.add_label(number, label)?;
        }
        fleet.gh.comment(number, &comment(casting, &labels))?;
        cast.push(json!({"issue": number, "harness": casting.harness,
            "model": casting.model, "why": casting.why}));
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
