//! Stage records on GitHub (docs/adr/0010): `verdict`, which writes the one
//! record a stage leaves; `show --issue`, which reads an issue's position off
//! its records; and a rebuild's read of them, from which a tick derives every
//! position it acts on and brings each `stage:` label to it. What a record is and what the records come to is the core's
//! (`afkconvoy_core::stage_record`).

use afkconvoy_core::claims::Claim;
use afkconvoy_core::config::Bound;
use afkconvoy_core::intake::{opened_by, taken_in};
use afkconvoy_core::plan::{Issue, PullRequest, closing_pr};
use afkconvoy_core::position::{Declared, Position};
use afkconvoy_core::precedent::{Ruling, proposed, rulings};
use afkconvoy_core::role::{Due, due, flows, followed, workflow_of};
use afkconvoy_core::stage_record::{
    Advance, Author, StageRecord, Stated, advance, history, label_moves, lands, stage_records,
};
use afkconvoy_core::team::{FLOW_LABEL, LAND, Team, Workflow};
use afkconvoy_core::tick::Outcome;
use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::claims::Scan;
use crate::copies::Copies;
use crate::fleet::Fleet;
use crate::gh::{Comment, GitHub};
use crate::git::Repo;
use crate::intake::came_in;
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
    /// Whether the worker is the stage's consult.
    pub consult: bool,
}

/// `afkconvoy verdict`, and a consult's `afkconvoy rule`: write the stage
/// record of the stage `declaring` ends — or, of a consult, the one whose
/// `escalate` it answers — and wake the fleet. A worker's is written under the claim the issue is
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
    let Declaring { issue, stage, declared, blocked_by, text, commits, consult } = declaring;
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
        by: Author::Worker { consult, claim: claim.sha.clone(), role_commit, team_commit },
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
    let (records, comments) = records_in(comments);
    (records, comments.iter().map(|comment| comment.url.clone()).collect())
}

/// The stage records among `comments`, oldest first, each with its comment.
fn records_in(comments: &[Comment]) -> (Vec<StageRecord>, Vec<&Comment>) {
    let found = stage_records(comments.iter().map(|comment| comment.body.as_str()));
    found.into_iter().filter_map(|(at, record)| Some((record, comments.get(at)?))).unzip()
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
///   {"issue", "workflow", "intake": {"by", "role", "from", "url"}, "position",
///    "follows", "rework": {"had", "left"}, "consecutive_rework": {<stage>:
///    {"had", "left"}}, "records": [{"stage", "verdict", "reads", "by", "claim",
///    "role_commit", "team_commit", "url"}]}
///
/// `intake` is how the issue came in: by a `worker`, with the role, the issue
/// it was on and the intake record's comment, or by a `human`. `follows` is
/// the position a `pass` declared now leads to; null where none can be
/// declared.
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
        .then(|| position(team, workflow, place.clone().after(Declared::Pass).position));
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
        "intake": came_in(&issue.labels, &comments),
        "position": position(team, workflow, place.position),
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
    /// The id of each record's comment.
    ids: Vec<u64>,
    /// When each record's comment was written, where GitHub said.
    at: Vec<Option<u64>>,
    /// The rulings its comments say were proposed as precedents
    /// (`precedent::proposed`).
    proposed: Vec<u64>,
    /// Whether a worker took it in (`intake::taken_in`).
    taken_in: bool,
    /// The patrol that opened it, when one did (`intake::opened_by`).
    opened_by: Option<String>,
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
    /// Whether the issue is with a consult: the worker asked after is one.
    pub consulted: bool,
}

/// Where one issue that carries a flow label stands, for `doctor` to judge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stood {
    pub number: u64,
    /// The flow labels it carries.
    pub flow_labels: Vec<String>,
    /// The workflow it follows, or why it follows none.
    pub workflow: Result<String, String>,
    /// The stage it is at — `land` once its last one passed to the landing
    /// train — and the role a worker performs it in, where one does.
    pub stage: Option<String>,
    pub role: Option<String>,
    /// Its rework in total: what it had, and what it has left.
    pub rework: (u64, u64),
    /// The same of each stage's rejections in a row, by the stage's name.
    pub in_a_row: Vec<(String, u64, u64)>,
    /// When each of its stage records was written, oldest first.
    pub recorded: Vec<u64>,
    /// The patrol that opened it, when one did.
    pub opened_by: Option<String>,
}

/// One rebuild's read of a bound repo's issues that follow a workflow: the
/// team as it stands, and the stage records of each. Everything a tick
/// derives of a position, it derives from this one read.
pub struct Flows {
    /// The team, or why it does not load. `None` where nothing was read:
    /// an unbound repo, or one with no issue that carries a flow label and
    /// no patrol named.
    team: Option<Result<Kept, String>>,
    issues: Vec<Flowing>,
}

impl Flows {
    /// Reads the team `fleet`'s repo is bound to and the stage records of
    /// each of `flowing`. A team that does not load is not an error here:
    /// the issues it leaves without a position are said so, one by one.
    pub fn read(fleet: &Fleet, flowing: &[&Issue]) -> Result<Flows> {
        let read = |bound: &&Bound| !flowing.is_empty() || !bound.patrols.is_empty();
        let Some(bound) = fleet.config.bound.as_ref().filter(read) else {
            return Ok(Flows { team: None, issues: vec![] });
        };
        let team = Copies::here()
            .and_then(|copies| keep(&fleet.gh, &copies, &bound.team))
            .map_err(|error| format!("{error:#}"));
        let mut issues = vec![];
        for issue in flowing {
            let comments = fleet.gh.comments(issue.number)?;
            let (records, said) = records_in(&comments);
            let urls = said.iter().map(|comment| comment.url.clone()).collect();
            let ids = said.iter().map(|comment| comment.id).collect();
            let at = said.iter().map(|comment| comment.at).collect();
            let bodies = || comments.iter().map(|comment| comment.body.as_str());
            let proposed = proposed(bodies());
            let taken_in = taken_in(&issue.labels, bodies()).is_some();
            let opened_by = opened_by(bodies());
            let (number, labels) = (issue.number, issue.labels.clone());
            issues.push(Flowing {
                number,
                labels,
                records,
                urls,
                ids,
                at,
                proposed,
                taken_in,
                opened_by,
            });
        }
        Ok(Flows { team: Some(team), issues })
    }

    /// The team as this rebuild read it.
    pub fn team(&self) -> Result<&Team> {
        Ok(&self.kept()?.team)
    }

    /// The team as this rebuild read it, with the copies it was read from.
    pub fn kept(&self) -> Result<&Kept> {
        match &self.team {
            Some(Ok(kept)) => Ok(kept),
            Some(Err(why)) => Err(anyhow!("{why}")),
            None => Err(anyhow!("no issue of this repo follows a workflow")),
        }
    }

    /// The patrol that opened each issue read that one opened.
    pub fn opened(&self) -> Vec<String> {
        self.issues.iter().filter_map(|issue| issue.opened_by.clone()).collect()
    }

    /// Where each issue read is in the workflow it follows, with that
    /// workflow. An issue that follows no one workflow has no position.
    pub fn positions(&self) -> Vec<(u64, &Workflow, Position<'_>)> {
        let Ok(team) = self.team() else { return vec![] };
        let placed = |issue: &Flowing| {
            let workflow = workflow_of(team, &issue.labels).ok()?;
            Some((issue.number, workflow, history(workflow, &issue.records).place.position))
        };
        self.issues.iter().filter_map(placed).collect()
    }

    /// Where each issue read stands. None where the team did not load: what
    /// its issues follow cannot be said.
    pub fn stood(&self) -> Vec<Stood> {
        let Ok(team) = self.team() else { return vec![] };
        let stood = |issue: &Flowing| {
            let flow_labels: Vec<String> =
                flows(&issue.labels).iter().map(|name| format!("{FLOW_LABEL}{name}")).collect();
            let mut stood = Stood {
                number: issue.number,
                flow_labels,
                workflow: Err(String::new()),
                stage: None,
                role: None,
                rework: (0, 0),
                in_a_row: vec![],
                recorded: issue.at.iter().flatten().copied().collect(),
                opened_by: issue.opened_by.clone(),
            };
            let workflow = match workflow_of(team, &issue.labels) {
                Ok(workflow) => workflow,
                Err(why) => return Stood { workflow: Err(why), ..stood },
            };
            let place = history(workflow, &issue.records).place;
            stood.workflow = Ok(workflow.name.clone());
            let (stage, work) = match place.position {
                Position::Landing => (Some(LAND.to_string()), workflow.land.as_ref()),
                at => (at.stage().map(|stage| stage.name.clone()), at.worker()),
            };
            stood.stage = stage;
            stood.role = work.map(|work| team.role_of(work).name.clone());
            stood.rework = (place.rework(), place.rework_left());
            for stage in workflow.stages().iter().filter(|stage| stage.reject().is_some()) {
                let (had, left) =
                    (place.consecutive_rework(stage), place.consecutive_rework_left(stage));
                stood.in_a_row.push((stage.name.clone(), had, left));
            }
            stood
        };
        self.issues.iter().map(stood).collect()
    }

    /// The rulings humans gave the issues read that nothing says were
    /// proposed as precedents, lowest issue first and oldest first on each.
    pub fn unproposed(&self) -> Vec<Ruling> {
        let Ok(team) = self.team() else { return vec![] };
        let mut found = vec![];
        for issue in &self.issues {
            let Ok(workflow) = workflow_of(team, &issue.labels) else { continue };
            let readings = history(workflow, &issue.records).readings;
            for ruling in rulings(issue.number, &issue.records, &readings, &issue.ids) {
                if !issue.proposed.contains(&ruling.comment) {
                    found.push(ruling);
                }
            }
        }
        found.sort_by_key(|ruling| (ruling.issue, ruling.comment));
        found
    }

    /// How many of the issues read — the open ones that follow a workflow — a
    /// worker took in: what the repo's `intake_limit` is held against.
    pub fn taken_in(&self) -> usize {
        self.issues.iter().filter(|issue| issue.taken_in).count()
    }

    /// What is due on each issue read (`role::due`), for the frontier. `prs`
    /// are the open PRs: the one that closes an issue is what its landing
    /// worker joins.
    pub fn due(&self, prs: &[PullRequest]) -> Vec<(u64, Due<'_>)> {
        let due = |issue: &Flowing| match self.team() {
            Ok(team) => {
                let pr = closing_pr(prs, issue.number).map(|pr| pr.number);
                due(team, &issue.labels, &issue.records, pr)
            }
            Err(why) => {
                let first = format!("{why:#}").lines().next().unwrap_or_default().to_string();
                Due::Nothing(format!("its position cannot be derived: {first}"))
            }
        };
        self.issues.iter().map(|issue| (issue.number, due(issue))).collect()
    }

    /// The issues read that are among `claims` under a landing worker's
    /// claim (`stage_record::lands`).
    pub fn landing(&self, claims: &[Claim]) -> Vec<u64> {
        let Ok(team) = self.team() else { return vec![] };
        let lands = |issue: &&Flowing| {
            let claim = claims.iter().find(|claim| claim.number == issue.number);
            let workflow = workflow_of(team, &issue.labels).ok();
            claim.zip(workflow).is_some_and(|(claim, workflow)| {
                lands(&history(workflow, &issue.records), &issue.records, &claim.sha)
            })
        };
        self.issues.iter().filter(lands).map(|issue| issue.number).collect()
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
            consulted: false,
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
            consulted: matches!(history.place.position, Position::Consult(..)),
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
