//! Stage records on GitHub (docs/adr/0010): `verdict`, which writes the one
//! record a stage leaves; `show --issue`, which reads an issue's position off
//! its records; and a rebuild's read of them, from which a tick derives every
//! position it acts on and brings each `stage:` label to it. What a record is and what the records come to is the core's
//! (`afkconvoy_core::stage_record`).

use afkconvoy_core::blocked::BlockedOn;
use afkconvoy_core::claims::Claim;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::intake::{opened_by, taken_in};
use afkconvoy_core::plan::{PullRequest, closing_pr, label_bar};
use afkconvoy_core::position::{Declared, Failure, Position};
use afkconvoy_core::role::{Due, due, followed, workflow_of};
use afkconvoy_core::stage_record::{
    Advance, Author, History, StageRecord, Stated, Unconsulted, advance, history, label_moves,
    lands, stage_records, unconsulted,
};
use afkconvoy_core::team::{FlowLabels, LAND, Team, Workflow};
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::transition::{Relabel, escalated_at, skips_consult};
use afkconvoy_core::waiting::{Back, Waits, back, waits};
use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::claims::Scan;
use crate::copies::Copies;
use crate::fleet::Fleet;
use crate::gh::{Comment, GitHub, Open, OpenIssues, Unreached, unreached};
use crate::git::Repo;
use crate::intake::came_in;
use crate::team::{Kept, keep};
use crate::workflow::position;

/// What a stage declares as it ends.
pub struct Declaring {
    pub issue: u64,
    pub stage: String,
    pub declared: Declared,
    pub blocked_by: BlockedOn,
    /// What the stage produced.
    pub text: String,
    /// What a worker ran under; `None` of a human, who declares under nothing.
    pub ran: Option<Ran>,
    /// Whether the worker is the stage's consult.
    pub consult: bool,
}

/// What the worker that declares ran under, as its environment carries it.
pub struct Ran {
    /// The commits of the role's repo and of the team's.
    pub role_commit: String,
    pub team_commit: String,
    /// The harness it is a session of.
    pub harness: Harness,
    /// The model it runs, where it was started on one.
    pub model: Option<String>,
}

/// `afkconvoy verdict`, and a consult's `afkconvoy rule`: write the stage
/// record of the stage `declaring` ends — or, of a consult, the one whose
/// `escalate` it answers; the fleet is woken as the command ends (`crate::acting`). A worker's is written under the claim the issue is
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
    let claim = crate::claims::on_this_host(
        repo,
        declaring.issue,
        &crate::host_settings::host_name(),
        crate::remote::Fresh::Live,
    )?;
    let Declaring { issue, stage, declared, blocked_by, text, ran, consult } = declaring;
    if stage.trim().is_empty() {
        bail!("--stage names no stage");
    }
    if text.trim().is_empty() {
        bail!("a stage record carries the text its stage produced: there is none");
    }
    if !blocked_by.is_empty() && declared != Declared::Blocked {
        bail!("--blocked-by goes with `blocked` only, not with `{}`", declared.word());
    }
    let Some(Ran { role_commit, team_commit, harness, model }) = ran else {
        if !blocked_by.is_empty() {
            bail!("--blocked-by is a worker's: nobody parks an issue no fleet instance holds");
        }
        let team = crate::team::load(crate::team::From::Config, Some(repo))?.team;
        return crate::steer::declare(repo, &team, issue, &stage, declared, text);
    };
    let Some(claim) = claim else {
        bail!(
            "issue #{issue} is not claimed: a verdict is declared under the claim its stage was \
             dispatched under"
        );
    };
    let record = StageRecord {
        stage,
        stated: Stated::Declared(declared),
        by: Author::Worker {
            consult,
            claim: claim.id.clone(),
            role_commit,
            team_commit,
            harness,
            model,
        },
        blocked_by,
        text,
    };
    let gh = GitHub::of(repo)?;
    let comments = gh.comments(issue)?;
    let standing = stage_records(comments.iter().map(|comment| comment.body.as_str()))
        .into_iter()
        .map(|(_, standing)| standing)
        .find(|standing| standing.stage == record.stage && standing.claim() == record.claim());
    if standing.is_none() {
        gh.comment(issue, &record.comment())?;
    }
    let mut answer = json!({"issue": issue, "stage": record.stage,
        "verdict": record.stated.word(), "claim": claim.id,
        "declared": standing.is_none()});
    if let Some(standing) = standing {
        answer["standing"] = json!(standing.stated.word());
    }
    Ok(answer)
}

/// Puts each open issue of a bound repo that was handed to a human, and that
/// no consult saw, to its stage's consult — once (`stage_record::unconsulted`):
/// the fleet leaves the record of its escalation, carrying what the comment
/// of it told a human, where its records do not already have it with the
/// consult, and swaps the escalate label back for its flow label → the issues
/// put. Made by an acting tick on its rebuild's read of the team, `flows`;
/// `plan` makes none.
pub fn put_to_consults(fleet: &Fleet, flows: &Flows, open: &OpenIssues) -> Result<Vec<u64>> {
    if fleet.config.bound.is_none() {
        return Ok(vec![]);
    }
    let team = flows.team()?;
    let mut put = vec![];
    for issue in &open.rows {
        if !issue.labels.contains(&team.escalate_label) || !team.flows(&issue.labels).is_empty() {
            continue;
        }
        let comments = open.comments(issue.number);
        let bodies = || comments.iter().map(|comment| comment.body.as_str());
        let Some(at) = escalated_at(bodies()) else { continue };
        if comments.get(at).is_some_and(|comment| skips_consult(&comment.body)) {
            continue;
        }
        let Ok(workflow) = followed(team, &issue.labels, bodies()) else { continue };
        let (records, _) = records_in(comments);
        let Some(unconsulted) = unconsulted(&history(workflow, records)) else { continue };
        if let Unconsulted::Record(stage) = unconsulted {
            let said = comments.get(at).map_or("", |comment| comment.body.as_str());
            // The comment of the escalation, but for its marker.
            let said = said.split_once("-->").map_or(said, |(_, said)| said).trim();
            let record = StageRecord::failed(&stage, Failure::Escalated, said);
            fleet.gh.comment(issue.number, &record.comment())?;
        }
        let back = Relabel::back(&issue.labels, workflow.flow_label(), &team.escalate_label);
        if let Some(label) = &back.add {
            fleet.gh.add_label(issue.number, label)?;
        }
        for label in &back.remove {
            fleet.gh.remove_label(issue.number, label)?;
        }
        put.push(issue.number);
    }
    Ok(put)
}

/// A consult's `afkconvoy send`: put issue `number`, with its stage's
/// consult, at the stage `stage` of its workflow, with `text` as why — a
/// stage record under the claim the consult holds, as its ruling is, which
/// moves no label: the issue never left the fleet. Its rework counts are
/// not started over (docs/adr/0018).
///
///   {"issue", "stage", "verdict": "sent", "claim", "declared", "woke"}
pub fn consult_send(
    repo: &Repo,
    team: &Team,
    number: u64,
    stage: &str,
    text: String,
    ran: Ran,
) -> Result<Value> {
    if text.trim().is_empty() {
        bail!("a stage record carries why the issue is sent: there is none");
    }
    let gh = GitHub::of(repo)?;
    let issue = gh.issue(number)?;
    let comments = gh.comments(number)?;
    let workflow = workflow_of(team, &issue.labels)
        .map_err(|why| anyhow!("issue #{number} cannot be sent to a stage: {why}"))?;
    if workflow.place_of(stage).is_none() {
        let has: Vec<&str> = workflow.stages().iter().map(|stage| stage.name.as_str()).collect();
        bail!(
            "the workflow `{}` issue #{number} follows has no stage `{stage}`; it has: {}",
            workflow.name,
            has.join(", ")
        );
    }
    let (records, _) = records_in(&comments);
    let read = history(workflow, records);
    if !matches!(read.place.position, Position::Consult(..)) {
        bail!("issue #{number} is not with its stage's consult; nothing was written");
    }
    let Some(claim) = Scan::read(repo)?.claim_of(number).map(|claim| claim.id.clone()) else {
        bail!("issue #{number} is not claimed: a consult sends under the claim it holds");
    };
    let Ran { role_commit, team_commit, harness, model } = ran;
    let by = Author::Worker {
        consult: true,
        claim: claim.clone(),
        role_commit,
        team_commit,
        harness,
        model,
    };
    let record = StageRecord {
        stage: stage.to_string(),
        stated: Stated::Sent,
        by,
        blocked_by: BlockedOn::default(),
        text,
    };
    let standing = read.records.iter().any(|standing| standing.claim() == record.claim());
    if !standing {
        gh.comment(number, &record.comment())?;
    }
    Ok(json!({"issue": number, "stage": stage, "verdict": "sent", "claim": claim,
        "declared": !standing}))
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
///    "role_commit", "team_commit", "harness", "url"}]}
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
    let history = history(workflow, records);
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
    let records: Vec<Value> = history
        .records
        .iter()
        .zip(&urls)
        .zip(&history.readings)
        .map(|((record, url), reading)| {
            let (role_commit, team_commit, harness, model) = match &record.by {
                Author::Worker { role_commit, team_commit, harness, model, .. } => {
                    (Some(role_commit), Some(team_commit), Some(harness), model.as_ref())
                }
                Author::Human | Author::Fleet => (None, None, None, None),
            };
            json!({"stage": record.stage, "verdict": record.stated.word(),
                "reads": reading.word(), "by": record.by.word(), "claim": record.claim(),
                "role_commit": role_commit, "team_commit": team_commit, "harness": harness,
                "model": model,
                "url": url})
        })
        .collect();
    Ok(json!({
        "issue": number,
        "workflow": workflow.name,
        "intake": came_in(team, &issue.labels, &comments),
        "position": position(team, workflow, place.position),
        "follows": follows,
        "rework": {"had": place.rework(), "left": place.rework_left()},
        "consecutive_rework": in_a_row,
        "records": records,
        "refusals": afkconvoy_core::worker::refusals(comments.iter().map(|c| c.body.as_str())),
    }))
}

/// One issue that follows a workflow, as a rebuild read it.
struct Flowing<'t> {
    number: u64,
    labels: Vec<String>,
    /// What its stage records come to.
    stages: Stages<'t>,
    /// The URL of each record's comment.
    urls: Vec<String>,
    /// When each record's comment was written, where GitHub said.
    at: Vec<Option<u64>>,
    /// Whether a worker took it in (`intake::taken_in`).
    taken_in: bool,
    /// The patrol that opened it, when one did (`intake::opened_by`).
    opened_by: Option<String>,
}

/// What the stage records of an issue that carries a flow label come to.
enum Stages<'t> {
    /// It follows this workflow, and its records read as this history.
    Follows(&'t Workflow, History<'t>),
    /// It follows no one workflow, for this reason — or the team did not
    /// load: its records read as nothing.
    Unfollowed(String, Vec<StageRecord>),
}

impl<'t> Stages<'t> {
    /// Reads `records` in the workflow of `team` that `labels` name, where
    /// the team loaded and they name one.
    fn read(team: Option<&'t Team>, labels: &[String], records: Vec<StageRecord>) -> Self {
        let team = team.ok_or_else(|| "the team did not load".to_string());
        match team.and_then(|team| workflow_of(team, labels)) {
            Ok(workflow) => Stages::Follows(workflow, history(workflow, records)),
            Err(why) => Stages::Unfollowed(why, records),
        }
    }

    /// The workflow the issue follows and its history in it, or why it
    /// follows none.
    fn follows(&self) -> Result<(&'t Workflow, &History<'t>), String> {
        match self {
            Stages::Follows(workflow, history) => Ok((workflow, history)),
            Stages::Unfollowed(why, _) => Err(why.clone()),
        }
    }

    /// The history of the issue in the workflow it follows.
    fn history(&self) -> Option<&History<'t>> {
        self.follows().ok().map(|(_, history)| history)
    }

    /// The stage records, oldest first.
    fn records(&self) -> &[StageRecord] {
        match self {
            Stages::Follows(_, history) => &history.records,
            Stages::Unfollowed(_, records) => records,
        }
    }
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
    /// Whether the issue is with a consult. A worker asked after is then the
    /// consult: the claim the stage itself was dispatched under carries its
    /// `escalate`, and is not asked after.
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
/// team as it stands, and the history of each in the workflow it follows,
/// read once. Everything a tick derives of a position, it derives from this
/// one read.
pub struct Flows {
    read: Read,
}

/// The issues read: each one's history borrows the workflow it follows from
/// the team.
type Issues<'t> = Vec<Flowing<'t>>;

self_cell::self_cell!(
    /// The team, or why it does not load — `None` in an unbound repo, where
    /// nothing was read — and the issues read in it.
    struct Read {
        owner: Option<Result<Kept, Unkept>>,
        #[covariant]
        dependent: Issues,
    }
);

/// Why a rebuild has no team.
enum Unkept {
    /// GitHub could not be reached for it: nothing is said of the team.
    Unreached(Unreached),
    /// Anything else, as it was said: the team has a fault, or cannot be copied.
    Unloaded(String),
}

impl Flows {
    /// Reads the team `fleet`'s repo is bound to and, among the comments of
    /// `open`, the stage records of each issue that carries a flow label. The
    /// team is read first: which labels are flow labels is its workflows' to
    /// say. A team that does not load is not an error here: the issues it
    /// leaves without a position are said so, one by one.
    pub fn read(fleet: &Fleet, open: &OpenIssues) -> Result<Flows> {
        let Some(bound) = fleet.config.bound.as_ref() else {
            return Ok(Flows { read: Read::new(None, |_| vec![]) });
        };
        let labels = fleet.config.labels();
        let team = Copies::of_host()
            .and_then(|copies| keep(&fleet.gh, &copies, &bound.team, &labels))
            .map_err(|error| match unreached(&error) {
                Some(read) => Unkept::Unreached(read.clone()),
                None => Unkept::Unloaded(format!("{error:#}")),
            });
        let read = Read::new(Some(team), |team| {
            let team = match team {
                Some(Ok(kept)) => Some(&kept.team),
                _ => None,
            };
            let flow_labels = team.map(Team::flow_labels).unwrap_or_default();
            let flowing = open.rows.iter().filter(|issue| {
                !afkconvoy_core::plan::is_repair(&issue.labels)
                    && !flow_labels.among(&issue.labels).is_empty()
            });
            let mut issues = vec![];
            for issue in flowing {
                let comments = open.comments(issue.number);
                let (records, said) = records_in(comments);
                let urls = said.iter().map(|comment| comment.url.clone()).collect();
                let at = said.iter().map(|comment| comment.at).collect();
                let bodies = || comments.iter().map(|comment| comment.body.as_str());
                let taken_in =
                    team.is_some_and(|team| taken_in(team, &issue.labels, bodies()).is_some());
                let opened_by = opened_by(bodies());
                let (number, labels) = (issue.number, issue.labels.clone());
                let stages = Stages::read(team, &labels, records);
                issues.push(Flowing { number, labels, stages, urls, at, taken_in, opened_by });
            }
            issues
        });
        Ok(Flows { read })
    }

    /// The issues read.
    fn issues(&self) -> &[Flowing<'_>] {
        self.read.borrow_dependent()
    }

    /// Issue `number`, where it was read.
    fn issue(&self, number: u64) -> Option<&Flowing<'_>> {
        self.issues().iter().find(|issue| issue.number == number)
    }

    /// The flow labels of the team as this rebuild read it: none where it
    /// read none, or the team did not load.
    pub fn flow_labels(&self) -> FlowLabels {
        self.team().map(Team::flow_labels).unwrap_or_default()
    }

    /// The team as this rebuild read it.
    pub fn team(&self) -> Result<&Team> {
        Ok(&self.kept()?.team)
    }

    /// The team as this rebuild read it, with the copies it was read from.
    pub fn kept(&self) -> Result<&Kept> {
        match self.read.borrow_owner() {
            Some(Ok(kept)) => Ok(kept),
            Some(Err(Unkept::Unreached(read))) => Err(read.clone().into()),
            Some(Err(Unkept::Unloaded(why))) => Err(anyhow!("{why}")),
            None => Err(anyhow!("no issue of this repo follows a workflow")),
        }
    }

    /// The patrol that opened each issue read that one opened.
    pub fn opened(&self) -> Vec<String> {
        self.issues().iter().filter_map(|issue| issue.opened_by.clone()).collect()
    }

    /// Where each issue read is in the workflow it follows, with that
    /// workflow. An issue that follows no one workflow has no position.
    pub fn positions(&self) -> Vec<(u64, &Workflow, Position<'_>)> {
        let mut placed = vec![];
        for issue in self.issues() {
            if let Ok((workflow, history)) = issue.stages.follows() {
                placed.push((issue.number, workflow, history.place.position));
            }
        }
        placed
    }

    /// Where each issue read stands. None where the team did not load: what
    /// its issues follow cannot be said.
    pub fn stood(&self) -> Vec<Stood> {
        let Ok(team) = self.team() else { return vec![] };
        let stood = |issue: &Flowing<'_>| {
            let flow_labels: Vec<String> =
                team.flows(&issue.labels).into_iter().map(str::to_string).collect();
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
            let (workflow, history) = match issue.stages.follows() {
                Ok(follows) => follows,
                Err(why) => return Stood { workflow: Err(why), ..stood },
            };
            let place = &history.place;
            stood.workflow = Ok(workflow.name.clone());
            let (stage, work) = match place.position {
                Position::Landing | Position::LandingParked => {
                    (Some(LAND.to_string()), workflow.land.as_ref())
                }
                Position::LandingEscalating => (Some(LAND.to_string()), None),
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
        self.issues().iter().map(stood).collect()
    }

    /// How many of the issues read — the open ones that follow a workflow — a
    /// worker took in: what the repo's `intake_limit` is held against.
    pub fn taken_in(&self) -> usize {
        self.issues().iter().filter(|issue| issue.taken_in).count()
    }

    /// What is due on each issue read (`role::due`), for the frontier. `prs`
    /// are the open PRs: the one that closes an issue is what its landing
    /// worker joins.
    pub fn due(&self, prs: &[PullRequest]) -> Vec<(u64, Due<'_>)> {
        let due = |issue: &Flowing<'_>| match self.team() {
            Ok(team) => {
                let pr = closing_pr(prs, issue.number).map(|pr| pr.number);
                due(team, &issue.labels, issue.stages.records(), pr)
            }
            Err(why) => {
                let first = format!("{why:#}").lines().next().unwrap_or_default().to_string();
                Due::Nothing(format!("its position cannot be derived: {first}"))
            }
        };
        self.issues().iter().map(|issue| (issue.number, due(issue))).collect()
    }

    /// The issues read that have a stage record: a worker was started on
    /// each before.
    pub fn begun(&self) -> Vec<u64> {
        let begun = |issue: &&Flowing<'_>| !issue.stages.records().is_empty();
        self.issues().iter().filter(begun).map(|issue| issue.number).collect()
    }

    /// The stage issue `number` is at — `land` once its last one passed to
    /// the landing train. `None` once its workflow has ended, and for an
    /// issue with no position.
    pub fn at(&self, number: u64) -> Option<String> {
        match self.issue(number)?.stages.history()?.place.position {
            at if at.landing() => Some(LAND.to_string()),
            at => at.stage().map(|stage| stage.name.clone()),
        }
    }

    /// Whether a stage declared a verdict on issue `number` under the claim
    /// `claim` (`History::verdict_under`): what is declared under a claim is
    /// carried out as it is read, and ends it.
    pub fn declared(&self, number: u64, claim: &str) -> bool {
        let history = self.issue(number).and_then(|issue| issue.stages.history());
        history.is_some_and(|history| history.verdict_under(claim).is_some())
    }

    /// The issues read that are among `claims` under a landing worker's
    /// claim (`stage_record::lands`).
    pub fn landing(&self, claims: &[Claim]) -> Vec<u64> {
        let lands = |issue: &&Flowing<'_>| {
            let claim = claims.iter().find(|claim| claim.number == issue.number);
            let history = issue.stages.history();
            claim.zip(history).is_some_and(|(claim, history)| lands(history, &claim.id))
        };
        self.issues().iter().filter(lands).map(|issue| issue.number).collect()
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
        let Some(issue) = self.issue(number) else {
            return Ok(unfollowed("it carries no flow label".to_string()));
        };
        self.team()?;
        let history = match issue.stages.follows() {
            Ok((_, history)) => history,
            Err(why) => return Ok(unfollowed(why)),
        };
        let verdict = history.verdict_under(claim);
        let waits_at = match history.place.position {
            Position::Waiting(stage) => Some(stage.name.clone()),
            _ => None,
        };
        Ok(Called {
            waits_at,
            consulted: matches!(history.place.position, Position::Consult(..)),
            advance: advance(history, claim),
            stage: self.at(number),
            said: verdict.and_then(|at| issue.urls.get(at).cloned()),
            rounds: history.rounds_said(&issue.urls),
        })
    }

    /// The stage of issue `number` whose consult a hand-off for `reason`
    /// goes to (`Place::consults`); `None` where it goes to a human.
    pub fn consults(&self, number: u64, failure: Failure) -> Option<String> {
        let (workflow, read) = self.issue(number)?.stages.follows().ok()?;
        // The stage it is at, or the one whose verdict the failure refutes.
        let refuted = read.records.iter().rev().find(|record| {
            matches!(record.by, Author::Worker { consult: false, .. })
                && matches!(record.stated, Stated::Declared(_))
        });
        let at = read.place.position.stage().map(|stage| stage.name.clone());
        [at, refuted.map(|record| record.stage.clone())].into_iter().flatten().find(|stage| {
            let mut records = read.records.clone();
            records.push(StageRecord::failed(stage, failure, ""));
            matches!(history(workflow, records).place.position, Position::Consult(..))
        })
    }

    /// The `escalate` on issue `number` a fleet lock may explain
    /// (`History::stage_escalate`) → the stage that declared it, and when,
    /// where GitHub said. `None` where the issue's last word is no such
    /// `escalate`.
    pub fn stage_escalate(&self, number: u64) -> Option<(String, Option<u64>)> {
        let issue = self.issue(number)?;
        let history = issue.stages.history()?;
        let at = history.stage_escalate()?;
        Some((history.records.get(at)?.stage.clone(), issue.at.get(at).copied().flatten()))
    }

    /// Whether `record` is the last stage record issue `number` was read
    /// with: what was left once is not left again.
    pub fn ends_with(&self, number: u64, record: &StageRecord) -> bool {
        self.issue(number).is_some_and(|issue| issue.stages.records().last() == Some(record))
    }

    /// Every round of the rework a breaker stopped on issue `number`, as the
    /// human it is handed to is told it; `None` where no breaker did.
    pub fn rounds(&self, number: u64) -> Option<String> {
        let issue = self.issue(number)?;
        issue.stages.history()?.rounds_said(&issue.urls)
    }

    /// Why the stage records of issue `number` hand it to a human, in the few
    /// words a tick's line has room for: the stage it stopped at, where it
    /// is at one, and what stopped it there.
    pub fn brief(&self, number: u64) -> String {
        let history = self.issue(number).and_then(|issue| issue.stages.history());
        let Some(history) = history else {
            return "it follows no one workflow".to_string();
        };
        match history.place.position {
            Position::Escalating(stage, reason) => format!("{}: {}", stage.name, reason.brief()),
            Position::Landing => "nothing to land in a turn with".to_string(),
            Position::LandingEscalating => "land: declared escalate, to a human".to_string(),
            at => match at.stage() {
                Some(stage) => format!("{}: its stage cannot be carried on", stage.name),
                None => "its workflow has ended".to_string(),
            },
        }
    }

    /// The open issues nobody is to start a worker on that wait for a human,
    /// each with what for and since when (`plan::Observed::waiting`): those
    /// read here by what is due on them, and — among `open` — the escalated
    /// ones, which carry no flow label and were not. An escalated issue is
    /// one that carries the escalate label and the comment of an escalation:
    /// the label alone may be a human's own.
    pub fn waiting(&self, fleet: &Fleet, open: &Open) -> Vec<(u64, Waits, Option<u64>)> {
        let mut found = vec![];
        let team = self.team().ok();
        for issue in self.issues() {
            let pr = closing_pr(&open.prs, issue.number).map(|pr| pr.number);
            let records = issue.stages.records();
            let waits = team.and_then(|team| waits(team, &issue.labels, records, pr));
            if let Some(waits) = waits {
                found.push((issue.number, waits, issue.at.last().copied().flatten()));
            }
        }
        let config = &fleet.config;
        let escalate = match (&config.bound, team) {
            (None, _) => Some(config.escalate_label.as_str()),
            (Some(_), Some(team)) => Some(team.escalate_label.as_str()),
            // What the team calls its escalate label was not read.
            (Some(_), None) => None,
        };
        for issue in &open.issues.rows {
            let comments = open.issues.comments(issue.number);
            let bodies = || comments.iter().map(|comment| comment.body.as_str());
            let handed = escalate.is_some_and(|label| issue.labels.iter().any(|l| l == label))
                && label_bar(&issue.labels, config, &self.flow_labels()).is_some();
            let Some(at) = escalated_at(bodies()).filter(|_| handed) else { continue };
            let back = match (team, escalate) {
                (Some(team), Some(_)) if config.bound.is_some() => {
                    let workflow = followed(team, &issue.labels, bodies()).ok();
                    back(team, workflow, &records_in(comments).0)
                }
                _ => Back::Ready {
                    ready: config.ready_label.clone(),
                    escalate: config.escalate_label.clone(),
                },
            };
            let since = comments.get(at).and_then(|comment| comment.at);
            found.push((issue.number, Waits::Escalated(back), since));
        }
        found
    }

    /// Brings the `stage:` label of issue `number` to the position its stage
    /// records give it → whether a label was moved. The labels it carries
    /// are read only to say what to write. An issue whose labels name no one
    /// workflow of the team has no position, and is left as it is.
    pub fn bring(&self, fleet: &Fleet, number: u64) -> Result<Outcome> {
        let Some(issue) = self.issue(number) else {
            return Ok(Outcome::Nothing);
        };
        self.team()?;
        let Some(history) = issue.stages.history() else {
            return Ok(Outcome::Nothing);
        };
        let position = history.place.position;
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

/// What `afkconvoy verdict --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

Once, as the last thing a stage does: it declares the verdict that ends the stage, with the text the stage produced. A worker's brief gives the line with `--issue` and `--stage` filled in; add the verdict and the text:

```
afkconvoy verdict pass --issue 7 --stage build --text-file /tmp/build.md
```

The five verdicts:

- `pass`: the stage did its work: the issue goes where the workflow says
- `reject`: the work of an earlier stage is turned down; the text says why. Only a stage whose workflow gives it a `reject` may declare it
- `blocked`: the issue waits on other issues: name them with `--blocked-by`. One that can go on only once the next version of this repo is installed — what it needs has landed but is not released — names `cut` there (`--blocked-by cut`) instead of polling for it: the fleet releases the claim and puts the label `convoy-waits-on-cut` on the issue, which keeps it off the frontier, so the fleet can go idle and cut. Once a cut has published and installed its version it takes that label off every issue carrying it, and each returns at the stage it left; a failed cut leaves them waiting. Where the repo names no cut steps, or nothing has landed past its newest version tag and no cut is running, no cut will come, and the issue escalates instead. A stage that splits its issue declares it on the issues it opened and took in (`afkconvoy intake --help`), and runs again when they have all closed. So does a stage whose gate is red for a defect outside its issue, on the defect's issue: one it opened and took in, or the open one that already recorded it
- `already-satisfied`: the issue needs no change: it is already done
- `escalate`: the stage cannot decide; the text says what has to be decided

Opening a PR declares nothing: a stage that wrote one still declares `pass` — after `afkconvoy gate` says green on what it pushed, as its brief tells it.

At Landing, a worker declares with `--stage land`: `blocked` names open issues the fleet can wait for and parks the claim, keeping work as `afkconvoy park` does. Once every blocker closes it returns to Landing, without repeating a stage. `escalate` hands it to a human with an answer line, keeping its PR and branch; a human's `afkconvoy rule` returns the issue to Landing. Other worker verdicts do not advance Landing.

A consult also declares `close` or `park`, with its reason in the text. Close ends the issue; park leaves it open for new evidence, off the frontier until explicitly brought back (see `afkconvoy help humans`). Neither adds rework or resets its counts. `escalate` lets through to a human what the consult was asked to rule on. Its ruling uses `afkconvoy rule --help`.

A human runs it too, for a stage a workflow gives them (`by: human`): the issue waits there, unclaimed, until they declare. `afkconvoy show --issue 7` names the stage:

```
afkconvoy verdict pass --issue 7 --stage sign-off --text "Approved."
```

## Who runs it

A worker, in its own worktree: it starts in its role's directory, so the line its brief gives changes to the worktree first. The fleet put the commits of its role and of its team, and the harness it is a session of, in its environment, and the command reads them from there.

Or a human, in a checkout of the bound repo, with neither commit named and none in the environment: that is what makes the record a human's.

The issue's claim is checked live first. If another host holds it, the command refuses: `the claim on #N is held by instance X on host H since <ts>: this worker's work was taken; stop`. No stage record is written. Ownership uses the host's name from its host settings; a new instance on the same host keeps its workers.

## What it reads and writes

It reads the claim on the issue off the remote and the issue's comments, and writes one comment on the issue: the stage record. The record opens with a marker that carries the stage, the verdict, the claim it was declared under, the commits of the role and of the team and the harness the worker ran on — and nothing of how it was started — and the text follows it. Nothing but this command writes that format.

A human's record is written under no claim, and only on an issue nobody holds. It says a human left it and carries no commit. Unlike a worker's it is checked first, against the team the config names and the issue's records: the issue must be at the stage named, and the verdict one that may be declared there.

The text is `--text`, or the contents of `--text-file`: one of the two, and not empty. Then it wakes the fleet, as `afkconvoy wake` does.

It moves nothing. Where the issue goes is read by the fleet off its records and its workflow at the next tick: see `afkconvoy show --help`.

## What it answers

{"issue", "stage", "verdict", "claim", "declared", "woke"} {"issue", "stage", "verdict", "by": "human", "declared": true, "woke"} — a human's

- declared: true: the record was written
- declared: false: this stage already has a record under this claim: nothing was written, and `standing` is the verdict that record declared
- `woke`: whether a `run` on this host was told, as `afkconvoy wake` answers

Errors, as {"error": …} with a non-zero exit: a word that is none of the five verdicts; no text, or an empty one; `--blocked-by` with another verdict than `blocked`; a worker's on an issue that is not claimed; one of `--role-commit` and `--team-commit` (or AFKCONVOY_ROLE_COMMIT and AFKCONVOY_TEAM_COMMIT) without the other; a `--harness` (or AFKCONVOY_HARNESS) that names no harness, or one with no commit; "is not in a git checkout", or git's own words for a checkout it refuses.

A human's is refused, with nothing written, when the issue "is claimed" — its stage is its worker's, and a worker's record names both commits; when it is at another stage, or has left its stages; when the verdict "cannot be declared" there; with `--blocked-by`; and when the issue carries no one flow label.

## What trips you up

- One stage declares once under one claim. A second run changes nothing, whatever it says: the first record stands.
- A record is a verdict only for the stage the claim was dispatched to. One that names another stage is written and counts for nothing: the worker is then treated as having declared no verdict.
- It does not check that the verdict is one the stage may declare. A `reject` from a stage whose workflow gives it none is no valid verdict, and the issue escalates.
- A ruling is not a verdict, and is not declared here: see `afkconvoy rule --help`.
- An `escalate` starts a consult only where the stage names one. Otherwise it escalates for the host's watch to answer. A named consult rules — and the stage runs again under the ruling — or declares `escalate` itself. That one always goes to a human: the chain is one role long.
- Whether a record is a consult's is read from AFKCONVOY_CONSULT, which the fleet put in the worker's environment (or `--consult`). Anything a consult declares but `escalate`, `close` or `park` is no valid verdict, and the issue escalates.
- With no commit named a record is a human's. A worker whose environment lost its commits is refused, as its issue is claimed.
- A human's `already-satisfied` ends the workflow and closes nothing, and a human's `blocked` parks nothing: no claim is there to do either."#;
