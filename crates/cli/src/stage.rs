//! Stage records on GitHub (docs/adr/0010): `verdict`, which writes the one
//! record a stage leaves; `show --issue`, which reads an issue's position off
//! its records; and a rebuild's read of them, from which a tick derives every
//! position it acts on and brings each `stage:` label to it. What a record is and what the records come to is the core's
//! (`afkconvoy_core::stage_record`).

use afkconvoy_core::plan::Issue;
use afkconvoy_core::position::{Declared, Position};
use afkconvoy_core::role::{Due, due, followed, workflow_of};
use afkconvoy_core::stage_record::{
    Advance, Author, StageRecord, Stated, advance, history, label_moves, stage_records,
};
use afkconvoy_core::team::Team;
use afkconvoy_core::tick::Outcome;
use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::claims::Scan;
use crate::copies::Copies;
use crate::fleet::Fleet;
use crate::gh::{Comment, GitHub};
use crate::git::Repo;
use crate::team::{Kept, keep};
use crate::wake::wake;
use crate::workflow::position;

/// What a stage declares as it ends.
pub struct Declaring {
    pub issue: u64,
    pub stage: String,
    pub declared: Declared,
    pub blocked_by: Vec<u64>,
    /// What the stage produced.
    pub text: String,
    /// The commits of the role's repo and of the team's a worker ran under;
    /// `None` of a human, who declares under neither.
    pub commits: Option<(String, String)>,
}

/// `afkconvoy verdict`: write the stage record of the stage `declaring` ends,
/// and wake the fleet. A worker's is written under the claim the issue is
/// held by, and a stage that has a record under that claim already gets no
/// second one:
///
///   {"issue", "stage", "verdict", "claim", "declared": true, "woke"}
///   {…, "declared": false, "standing": <the verdict already on record>, "woke"}
///
/// Whether a worker's record is a verdict is not decided here: the fleet
/// reads it against the stage it dispatched under that claim. A human's — one
/// that names no commits — is `steer::declare`'s.
pub fn verdict(repo: &Repo, declaring: Declaring) -> Result<Value> {
    let Declaring { issue, stage, declared, blocked_by, text, commits } = declaring;
    if stage.trim().is_empty() {
        bail!("--stage names no stage");
    }
    if text.trim().is_empty() {
        bail!("a stage record carries the text its stage produced: there is none");
    }
    if !blocked_by.is_empty() && declared != Declared::Blocked {
        bail!("--blocked-by goes with `blocked` only, not with `{}`", declared.word());
    }
    let Some((role_commit, team_commit)) = commits else {
        if !blocked_by.is_empty() {
            bail!("--blocked-by is a worker's: nobody parks an issue no fleet instance holds");
        }
        let team = crate::team::load(crate::team::From::Config)?.team;
        return crate::steer::declare(repo, &team, issue, &stage, declared, text);
    };
    let scan = Scan::read(repo)?;
    let Some(claim) = scan.claim_of(issue) else {
        bail!(
            "issue #{issue} is not claimed: a verdict is declared under the claim its stage was \
             dispatched under"
        );
    };
    let record = StageRecord {
        stage,
        stated: Stated::Declared(declared),
        by: Author::Worker { claim: claim.sha.clone(), role_commit, team_commit },
        blocked_by,
        text,
    };
    let gh = GitHub { dir: repo.dir.clone() };
    let comments = gh.comments(issue)?;
    let standing = stage_records(comments.iter().map(|comment| comment.body.as_str()))
        .into_iter()
        .map(|(_, standing)| standing)
        .find(|standing| standing.stage == record.stage && standing.claim() == record.claim());
    if standing.is_none() {
        gh.comment(issue, &record.comment())?;
    }
    let mut answer = json!({"issue": issue, "stage": record.stage,
        "verdict": record.stated.word(), "claim": claim.sha,
        "declared": standing.is_none(), "woke": wake(repo)?["woke"]});
    if let Some(standing) = standing {
        answer["standing"] = json!(standing.stated.word());
    }
    Ok(answer)
}

/// The stage records among `comments`, oldest first, each with the URL of
/// its comment.
fn records_among(comments: &[Comment]) -> (Vec<StageRecord>, Vec<String>) {
    let found = stage_records(comments.iter().map(|comment| comment.body.as_str()));
    let url = |at: usize| comments.get(at).map(|c| c.url.clone()).unwrap_or_default();
    found.into_iter().map(|(at, record)| (record, url(at))).unzip()
}

/// The stage records of issue `number`, oldest first, each with the URL of
/// its comment.
pub(crate) fn records_of(gh: &GitHub, number: u64) -> Result<(Vec<StageRecord>, Vec<String>)> {
    Ok(records_among(&gh.comments(number)?))
}

/// `afkconvoy show --issue`: where issue `number` is in the workflow of
/// `team` its flow label names — or, escalated, the one it left — read off
/// its stage records.
///
///   {"issue", "workflow", "position", "follows", "rework": {"had", "left"},
///    "consecutive_rework": {<stage>: {"had", "left"}}, "records": [{"stage",
///    "verdict", "reads", "by", "claim", "role_commit", "team_commit", "url"}]}
///
/// `follows` is the position a `pass` declared now leads to; null where none
/// can be declared.
pub fn show_issue(gh: &GitHub, team: &Team, number: u64) -> Result<Value> {
    let issue = gh.issue(number)?;
    let comments = gh.comments(number)?;
    let workflow = followed(team, &issue.labels, comments.iter().map(|c| c.body.as_str()))
        .map_err(|why| anyhow!("issue #{number} has no position: {why}"))?;
    let (records, urls) = records_among(&comments);
    let history = history(workflow, &records);
    let place = &history.place;
    let follows = place
        .takes(Declared::Pass)
        .then(|| position(workflow, place.clone().after(Declared::Pass).position));
    let mut in_a_row = Map::new();
    for stage in workflow.stages().iter().filter(|stage| stage.reject().is_some()) {
        let had = place.consecutive_rework(stage);
        let left = place.consecutive_rework_left(stage);
        in_a_row.insert(stage.name.clone(), json!({"had": had, "left": left}));
    }
    let records: Vec<Value> = records
        .iter()
        .zip(&urls)
        .zip(&history.readings)
        .map(|((record, url), reading)| {
            let (role_commit, team_commit) = match &record.by {
                Author::Worker { role_commit, team_commit, .. } => {
                    (Some(role_commit), Some(team_commit))
                }
                Author::Human => (None, None),
            };
            json!({"stage": record.stage, "verdict": record.stated.word(),
                "reads": reading.word(), "by": record.by.word(), "claim": record.claim(),
                "role_commit": role_commit, "team_commit": team_commit, "url": url})
        })
        .collect();
    Ok(json!({
        "issue": number,
        "workflow": workflow.name,
        "position": position(workflow, place.position),
        "follows": follows,
        "rework": {"had": place.rework(), "left": place.rework_left()},
        "consecutive_rework": in_a_row,
        "records": records,
    }))
}

/// One issue that follows a workflow, as a rebuild read it.
struct Flowing {
    number: u64,
    labels: Vec<String>,
    records: Vec<StageRecord>,
    /// The URL of each record's comment.
    urls: Vec<String>,
}

/// What a claim's stage records call for, with what a tick says of it.
pub struct Called {
    pub advance: Advance,
    /// The stage the issue is at, while it is at one.
    pub stage: Option<String>,
    /// The comment the verdict under the claim is in, once there is one.
    pub said: Option<String>,
    /// Every round of the rework a breaker stopped, where one did.
    pub rounds: Option<String>,
    /// The stage the issue waits at for a human, when it is at a human's.
    pub waits_at: Option<String>,
}

/// One rebuild's read of a bound repo's issues that follow a workflow: the
/// team as it stands, and the stage records of each. Everything a tick
/// derives of a position, it derives from this one read.
pub struct Flows {
    /// The team, or why it does not load. `None` where nothing was read:
    /// an unbound repo, or no issue that carries a flow label.
    team: Option<Result<Kept, String>>,
    issues: Vec<Flowing>,
}

impl Flows {
    /// Reads the team `fleet`'s repo is bound to and the stage records of
    /// each of `flowing`. A team that does not load is not an error here:
    /// the issues it leaves without a position are said so, one by one.
    pub fn read(fleet: &Fleet, flowing: &[&Issue]) -> Result<Flows> {
        let Some(bound) = fleet.config.bound.as_ref().filter(|_| !flowing.is_empty()) else {
            return Ok(Flows { team: None, issues: vec![] });
        };
        let team = Copies::here()
            .and_then(|copies| keep(&fleet.gh, &copies, &bound.team))
            .map_err(|error| format!("{error:#}"));
        let mut issues = vec![];
        for issue in flowing {
            let (records, urls) = records_of(&fleet.gh, issue.number)?;
            let (number, labels) = (issue.number, issue.labels.clone());
            issues.push(Flowing { number, labels, records, urls });
        }
        Ok(Flows { team: Some(team), issues })
    }

    fn team(&self) -> Result<&Team> {
        match &self.team {
            Some(Ok(kept)) => Ok(&kept.team),
            Some(Err(why)) => Err(anyhow!("{why}")),
            None => Err(anyhow!("no issue of this repo follows a workflow")),
        }
    }

    /// What is due on each issue read (`role::due`), for the frontier.
    pub fn due(&self) -> Vec<(u64, Due<'_>)> {
        let due = |issue: &Flowing| match self.team() {
            Ok(team) => due(team, &issue.labels, &issue.records),
            Err(why) => {
                let first = format!("{why:#}").lines().next().unwrap_or_default().to_string();
                Due::Nothing(format!("its position cannot be derived: {first}"))
            }
        };
        self.issues.iter().map(|issue| (issue.number, due(issue))).collect()
    }

    /// What the stage records of issue `number` call for of the claim
    /// `claim` on it. An issue that follows no one workflow is handed to a
    /// human: a bound repo's issue is the fleet's only while it does.
    pub fn called(&self, number: u64, claim: &str) -> Result<Called> {
        let unfollowed = |why: String| Called {
            advance: Advance::Escalate(format!("its stage cannot be carried on: {why}")),
            stage: None,
            said: None,
            rounds: None,
            waits_at: None,
        };
        let Some(issue) = self.issues.iter().find(|issue| issue.number == number) else {
            return Ok(unfollowed("it carries no flow label".to_string()));
        };
        let workflow = match workflow_of(self.team()?, &issue.labels) {
            Ok(workflow) => workflow,
            Err(why) => return Ok(unfollowed(why)),
        };
        let history = history(workflow, &issue.records);
        let verdict = history.verdict_under(&issue.records, claim);
        let waits_at = match history.place.position {
            Position::Waiting(stage) => Some(stage.name.clone()),
            _ => None,
        };
        Ok(Called {
            waits_at,
            advance: advance(&history, &issue.records, claim),
            stage: history.place.position.stage().map(|stage| stage.name.clone()),
            said: verdict.and_then(|at| issue.urls.get(at).cloned()),
            rounds: history.rounds_said(&issue.records, &issue.urls),
        })
    }

    /// Every round of the rework a breaker stopped on issue `number`, as the
    /// human it is handed to is told it; `None` where no breaker did.
    pub fn rounds(&self, number: u64) -> Option<String> {
        let issue = self.issues.iter().find(|issue| issue.number == number)?;
        let workflow = workflow_of(self.team().ok()?, &issue.labels).ok()?;
        history(workflow, &issue.records).rounds_said(&issue.records, &issue.urls)
    }

    /// Brings the `stage:` label of issue `number` to the position its stage
    /// records give it → whether a label was moved. The labels it carries
    /// are read only to say what to write. An issue whose labels name no one
    /// workflow of the team has no position, and is left as it is.
    pub fn bring(&self, fleet: &Fleet, number: u64) -> Result<Outcome> {
        let Some(issue) = self.issues.iter().find(|issue| issue.number == number) else {
            return Ok(Outcome::Nothing);
        };
        let Ok(workflow) = workflow_of(self.team()?, &issue.labels) else {
            return Ok(Outcome::Nothing);
        };
        let position = history(workflow, &issue.records).place.position;
        let moves = label_moves(&issue.labels, position);
        if let Some(label) = &moves.add {
            fleet.gh.add_label(issue.number, label)?;
        }
        for label in &moves.remove {
            fleet.gh.remove_label(issue.number, label)?;
        }
        let moved = moves.add.is_some() || !moves.remove.is_empty();
        Ok(if moved { Outcome::Done } else { Outcome::Nothing })
    }
}
