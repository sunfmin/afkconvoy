//! A worker's start in its role (docs/adr/0009): the stage an issue's worker
//! is to be started on, and where a worker of a bound repo starts and with
//! which facts. What the fleet adds to the worker launch command to put it
//! there is its harness's (`harness`). What is added is fixed: nothing here
//! is configurable, and an unbound repo's command never comes this way.

use flux_rs::attrs::*;

use crate::harness::Harness;
use crate::host_watch::WATCH_ENV;
use crate::patrol::PATROL_ENV;
use crate::position::Position;
use crate::stage_record::{StageRecord, history};
use crate::team::{Consult, Role, Stage, Team, Work, Workflow};
use crate::transition::escalated_from;

/// Every flag the fleet adds: what a bound repo's worker launch command must
/// take. They are the default harness's own.
pub const FLAGS: &[&str] = Harness::DEFAULT.flags();

/// The variables a worker's environment carries the commits of its role's
/// repo and of the team's in: the command that declares a verdict reads them.
pub const ROLE_COMMIT_ENV: &str = "AFKCONVOY_ROLE_COMMIT";
pub const TEAM_COMMIT_ENV: &str = "AFKCONVOY_TEAM_COMMIT";
/// The variable it carries the harness the worker is a session of in: the
/// command that declares a verdict reads it too, and the guard. The harness
/// crate's, whose hooks read it.
pub use afkconvoy_harness::HARNESS_ENV;
/// The variable it carries the model the worker runs in, where it was
/// started on one: the command that declares a verdict reads it too.
pub const MODEL_ENV: &str = "AFKCONVOY_MODEL";
/// The variables it carries the issue its worker is on and the worker's role
/// in: the command that takes an issue in reads them.
pub const ISSUE_ENV: &str = "AFKCONVOY_ISSUE";
pub const ROLE_ENV: &str = "AFKCONVOY_ROLE";
/// The variables the guard reads its stage's limits from (`guard::Limits`).
pub const STAGE_ENV: &str = "AFKCONVOY_STAGE";
pub const WRITES_ENV: &str = "AFKCONVOY_WRITES";
pub const CONSULT_ENV: &str = "AFKCONVOY_CONSULT";
pub const BASE_BRANCH_ENV: &str = "AFKCONVOY_BASE_BRANCH";
/// An explicitly selected task destination, protected alongside the repo base.
pub const TARGET_BRANCH_ENV: &str = "AFKCONVOY_TARGET_BRANCH";
pub const ESCALATE_LABEL_ENV: &str = "AFKCONVOY_ESCALATE_LABEL";
pub const LOCK_LABEL_ENV: &str = "AFKCONVOY_LOCK_LABEL";

/// The stage an issue is dispatched to, and who performs it — or, of a
/// consult, the stage that asked and who rules on its `escalate`.
#[derive(Debug, Clone, Copy)]
pub struct Entry<'t> {
    pub workflow: &'t Workflow,
    pub stage: &'t Stage,
    pub work: &'t Work,
    pub role: &'t Role,
}

/// The join an issue is dispatched to once its last stage passed to the
/// landing train, and who performs it.
#[derive(Debug, Clone, Copy)]
pub struct Lander<'t> {
    pub workflow: &'t Workflow,
    /// The workflow's `land:`: the role, and its landing skill.
    pub work: &'t Work,
    pub role: &'t Role,
    /// The issue's open PR: what is gated and joined.
    pub pr: u64,
}

/// The workflow of `team` an issue carrying `labels` follows: the one its
/// flow label names. An error says why it follows none.
#[spec(fn(&Team[@roles], &[String]) -> Result<&Workflow{v: v.roles == roles}, String>)]
pub fn workflow_of<'t>(team: &'t Team, labels: &[String]) -> Result<&'t Workflow, String> {
    let label = match team.flows(labels).as_slice() {
        [] => return Err("it carries no flow label".to_string()),
        [label] => *label,
        several => {
            return Err(format!(
                "it carries {} flow labels ({}): an issue follows one workflow",
                several.len(),
                several.join(", ")
            ));
        }
    };
    team.labelled(label)
        .ok_or_else(|| format!("its flow label {label} names no workflow of the team"))
}

/// The workflow of `team` an issue carrying `labels` follows, or — handed to
/// a human, its flow label swapped for the escalate label — followed: the
/// one the latest escalation among its comments (`bodies`, oldest first) says
/// it left.
pub fn followed<'t, 'a>(
    team: &'t Team,
    labels: &[String],
    bodies: impl IntoIterator<Item = &'a str>,
) -> Result<&'t Workflow, String> {
    if !team.flows(labels).is_empty() {
        return workflow_of(team, labels);
    }
    let Some(left) = escalated_from(bodies) else {
        return Err("it carries no flow label, and no escalation of it says which it left".into());
    };
    team.workflow(&left).ok_or_else(|| {
        format!("it was escalated from the workflow {left}, which is no workflow of the team")
    })
}

/// What an issue's place in its workflow calls for while nobody holds it.
#[derive(Debug, Clone)]
pub enum Due<'t> {
    /// A worker is to be started on this stage.
    Worker(Entry<'t>),
    /// A worker is to be started as the consult this stage names, to rule on
    /// its `escalate`.
    Consult(Entry<'t>),
    /// A worker is to be started in the workflow's landing role, to gate the
    /// issue's PR and land in a turn.
    Lander(Lander<'t>),
    /// The issue is to be handed to a human, for this reason.
    Human(String),
    /// Nothing is to be done with it, for this reason.
    Nothing(String),
}

/// What is due on an issue carrying `labels` whose stage records are
/// `records`, oldest first, and whose open PR is `pr`: the stage its position
/// starts a worker on, in the workflow of `team` its flow label names, the
/// consult a stage that declared `escalate` names, or — its last stage having
/// passed to the landing train — the join of that PR.
/// A flow label that names no one workflow, a position that is an
/// escalation, and a landing with no open PR to join hand it to a human; an
/// issue with no flow label is not the fleet's.
pub fn due<'t>(
    team: &'t Team,
    labels: &[String],
    records: &[StageRecord],
    pr: Option<u64>,
) -> Due<'t> {
    let workflow = match workflow_of(team, labels) {
        Ok(workflow) => workflow,
        Err(why) if team.flows(labels).is_empty() => return Due::Nothing(why),
        Err(why) => return Due::Human(why),
    };
    let position = history(workflow, records.to_vec()).place.position;
    if let Some((stage, work)) = position.start() {
        return Due::Worker(Entry { workflow, stage, work, role: team.role_of(work) });
    }
    let waits = |stage: &Stage, on: &str| {
        Due::Nothing(format!("its stage {:?} waits for {on}", stage.name))
    };
    match position {
        Position::Escalating(stage, reason) => Due::Human(reason.said(stage)),
        Position::LandingEscalating => Due::Human(crate::position::LANDING_ESCALATION.to_string()),
        Position::Consult(stage, Consult::Role(work), _) => {
            Due::Consult(Entry { workflow, stage, work, role: team.role_of(work) })
        }
        Position::Perform(stage, _) | Position::Waiting(stage) | Position::Parked(stage) => {
            waits(stage, "a human: no worker performs it")
        }
        Position::ParkedForEvidence(stage) => waits(stage, "new evidence and an explicit send"),
        Position::Landing | Position::LandingParked => match (&workflow.land, pr) {
            (Some(work), Some(pr)) => {
                Due::Lander(Lander { workflow, work, role: team.role_of(work), pr })
            }
            (Some(_), None) => Due::Human(
                "its last stage passed it to the landing train, and no open PR closes it: there \
                 is nothing to join"
                    .to_string(),
            ),
            (None, _) => Due::Human("its workflow names nobody who lands".to_string()),
        },
        Position::Ended => Due::Nothing("its workflow has ended".to_string()),
    }
}

/// What a worker in its role is on.
#[derive(Debug, Clone, Copy)]
pub enum On<'a> {
    /// A stage of an issue — `land` for the join that follows the last.
    Stage {
        issue: u64,
        /// The name of the workflow the issue follows.
        flow: &'a str,
        stage: &'a str,
    },
    /// A run of the patrol of this name: no issue, and no stage.
    Patrol(&'a str),
    /// The host watch: no issue or stage; its commands have operator authority.
    Watch(&'a str),
}

/// The facts of one start: what its worker, and the commands it runs, read
/// from the environment.
#[derive(Debug, Clone)]
pub struct Facts<'a> {
    pub on: On<'a>,
    pub role: &'a str,
    pub skill: &'a str,
    /// Whether the worker may commit. A patrol's never may.
    pub writes: bool,
    /// Whether the worker is a consult, which writes no code whatever its
    /// stage may.
    pub consult: bool,
    /// The repo's base branch, and the labels that are the fleet's to move
    /// beside the flow and stage labels: the team's escalate label and the
    /// repo's lock label.
    pub base_branch: &'a str,
    pub target_branch: Option<&'a str>,
    pub escalate_label: &'a str,
    pub lock_label: &'a str,
    /// The commits of the role's repo and the team's the stage runs under.
    pub role_commit: &'a str,
    pub team_commit: &'a str,
}

impl Facts<'_> {
    /// The variables a worker's environment carries them in.
    pub fn env(&self) -> Vec<(&'static str, String)> {
        let mut env = match self.on {
            On::Stage { issue, flow, stage } => vec![
                (ISSUE_ENV, issue.to_string()),
                ("AFKCONVOY_FLOW", flow.to_string()),
                (STAGE_ENV, stage.to_string()),
            ],
            On::Patrol(patrol) => vec![(PATROL_ENV, patrol.to_string())],
            On::Watch(worker) => vec![(WATCH_ENV, worker.to_string())],
        };
        env.extend([
            (ROLE_ENV, self.role.to_string()),
            ("AFKCONVOY_SKILL", self.skill.to_string()),
            (WRITES_ENV, self.writes.to_string()),
            (CONSULT_ENV, self.consult.to_string()),
            (BASE_BRANCH_ENV, self.base_branch.to_string()),
            (ESCALATE_LABEL_ENV, self.escalate_label.to_string()),
            (LOCK_LABEL_ENV, self.lock_label.to_string()),
            (ROLE_COMMIT_ENV, self.role_commit.to_string()),
            (TEAM_COMMIT_ENV, self.team_commit.to_string()),
        ]);
        if let Some(target) = self.target_branch {
            env.push((TARGET_BRANCH_ENV, target.to_string()));
        }
        env
    }
}

/// Where one worker starts, and what is added to it.
#[derive(Debug, Clone)]
pub struct Start<'a> {
    /// The fleet's copy of the role's directory: the worker's working directory.
    pub role_dir: &'a str,
    /// The issue's worktree, or the checkout of the base branch a patrol's
    /// worker is given.
    pub worktree: &'a str,
    /// The fleet's copy of the team's directory, without those of its roles.
    pub team_dir: &'a str,
    /// The stage's model, or the patrol's, when its definition names one.
    pub model: Option<&'a str>,
    /// The shell line that runs the guard: this tool's `guard` subcommand.
    pub guard: &'a str,
    /// The directory this host keeps for the worker's harness, holding what
    /// that harness is given of the fleet's own (`Harness::kept`).
    pub kept: &'a str,
    /// Whether the worker is given the host user's own skills and rules
    /// (`Team::host_user`): the directory `kept` is then the one kept with
    /// them (`Kept::host_user`).
    pub host_user: bool,
    /// Native rule paths of the host user's which this harness must exclude
    /// when `host_user` is false, including ones found as project ancestors
    /// (`Harness::host_rule_excludes`).
    pub host_rules: &'a [String],
    /// The DSH profile selected by the team or role.
    pub dsh_profile: crate::team::DshProfile,
    /// The files of the host user's skills that a role's skill of the same
    /// name stands for, where its harness would show both
    /// (`Harness::shadowed_in`): each is turned off.
    pub shadowed: &'a [String],
    /// The rules its harness is told on the line that starts it: nothing, of
    /// a harness that loads them itself (`Harness::told`).
    pub told: Told<'a>,
    pub facts: Facts<'a>,
}

/// One set of rules as it was read, and the rules a worker is given from
/// outside the directory it starts in, read for a harness that loads none
/// from there itself (`Harness::told`): the repo's own, the team rules
/// (`Told::added`, from the team's copy) and the role's (`Told::own`, from
/// the file its harness names, `Harness::role_file`).
pub use afkconvoy_harness::{Rules, Told};

impl Start<'_> {
    /// The line a terminal opened on the issue's worktree runs to start this
    /// worker of `harness` in its role with the worker launch command
    /// `command` (`Harness::in_role`): from the role's copy, with the team's
    /// added and the guard as the hook before each tool call, and the stage's
    /// facts, the harness and the model in its environment.
    pub fn in_role(&self, harness: Harness, command: &str) -> String {
        let mut env = self.facts.env();
        env.push((HARNESS_ENV, harness.key().to_string()));
        if let Some(model) = self.model {
            env.push((MODEL_ENV, model.to_string()));
        }
        let start = afkconvoy_harness::Start {
            dir: self.role_dir,
            worktree: self.worktree,
            added: self.team_dir,
            model: self.model,
            hook: self.guard,
            kept: self.kept,
            host_user: self.host_user,
            kept_config: false,
            host_rules: self.host_rules,
            dsh_profile: self.dsh_profile,
            shadowed: self.shadowed,
            told: self.told,
            env,
        };
        harness.in_role(command, &start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RepoDir, RepoLabels};
    use crate::position::Declared;
    use crate::stage_record::{Author, Stated};
    use crate::team::{File, RoleDir, Sources, TEAM_FILE, load};
    use crate::transition::escalation_comment;

    /// A worker's start in its role, put to its harness as a neutral start:
    /// the stage's facts, the harness and the model are the variables of its
    /// environment, in that order, after the harness's own.
    #[test]
    fn a_start_in_a_role_is_put_to_every_harness_with_its_facts_harness_and_model() {
        let start = Start {
            role_dir: "/copies/acme/dev/abc",
            worktree: "/wt/issue 7",
            team_dir: "/copies/acme/team/def",
            model: Some("opus"),
            guard: "/bin/afkconvoy guard",
            kept: "/cache/harness/claude-code",
            host_user: false,
            host_rules: &[],
            dsh_profile: crate::team::DshProfile::Fleet,
            shadowed: &[],
            told: Told::default(),
            facts: Facts {
                on: On::Stage { issue: 7, flow: "feature", stage: "build" },
                role: "dev",
                skill: "implement",
                writes: false,
                consult: false,
                base_branch: "main",
                target_branch: None,
                escalate_label: "ready-for-human",
                lock_label: "fleet-lock",
                role_commit: "abc",
                team_commit: "def",
            },
        };
        let mut env = start.facts.env();
        env.extend([(HARNESS_ENV, String::new()), (MODEL_ENV, "opus".to_string())]);
        for harness in Harness::ALL {
            let at = env.len() - 2;
            env[at].1 = harness.key().to_string();
            let neutral = afkconvoy_harness::Start {
                dir: "/copies/acme/dev/abc",
                worktree: "/wt/issue 7",
                added: "/copies/acme/team/def",
                model: Some("opus"),
                hook: "/bin/afkconvoy guard",
                kept: "/cache/harness/claude-code",
                host_user: false,
                kept_config: false,
                host_rules: &[],
                dsh_profile: crate::team::DshProfile::Fleet,
                shadowed: &[],
                told: Told::default(),
                env: env.clone(),
            };
            assert_eq!(
                start.in_role(harness, "kim --yolo"),
                harness.in_role("kim --yolo", &neutral),
                "{}",
                harness.key()
            );
        }
        // DSH quotes the whole line once more, as one word of its start script.
        for harness in Harness::ALL.into_iter().filter(|harness| *harness != Harness::Dsh) {
            let line = start.in_role(harness, "kim --yolo");
            let facts = " AFKCONVOY_ISSUE='7' AFKCONVOY_FLOW='feature' AFKCONVOY_STAGE='build' \
                 AFKCONVOY_ROLE='dev' AFKCONVOY_SKILL='implement' AFKCONVOY_WRITES='false' \
                 AFKCONVOY_CONSULT='false' AFKCONVOY_BASE_BRANCH='main' \
                 AFKCONVOY_ESCALATE_LABEL='ready-for-human' AFKCONVOY_LOCK_LABEL='fleet-lock' \
                 AFKCONVOY_ROLE_COMMIT='abc' AFKCONVOY_TEAM_COMMIT='def' ";
            let ended = format!(
                "{facts}AFKCONVOY_HARNESS='{}' AFKCONVOY_MODEL='opus' && kim --yolo",
                harness.key()
            );
            assert!(line.contains(&ended), "{}: {line}", harness.key());
        }
    }

    const TEAM: &str = "roles:\n  dev: acme/dev\n  boss: acme/boss\n";
    const FEATURE: &str = "\
stages:
  - name: build
    by: dev
    do: implement
    on: { pass: done }
";
    const SIGNED: &str = "\
stages:
  - name: sign-off
    by: human
    on: { pass: done }
";
    const REVIEW: &str = "\
stages:
  - name: build
    by: dev
    do: implement
    on: { pass: check }
  - name: check
    by: boss
    do: implement
    on: { pass: done, reject: build }
";
    const ASKS: &str = "\
stages:
  - name: build
    by: dev
    do: implement
    escalate: { to: boss, do: implement }
    on: { pass: done }
";

    const LANDS: &str = "\
stages:
  - name: build
    by: dev
    do: implement
    on: { pass: land }
land: { by: boss, do: implement }
";

    fn file(path: &str, yaml: &str) -> File {
        File {
            path: path.to_string(),
            prose: String::new(),
            yaml: serde_yaml::from_str(yaml).map_err(|e| e.to_string()),
        }
    }

    fn team() -> Team {
        let repo = |name: &str| {
            let skills = vec!["implement".to_string()];
            (
                RepoDir::read(name).unwrap(),
                Ok(RoleDir { commit: format!("{name}@1"), skills, ..RoleDir::default() }),
            )
        };
        load(&Sources {
            team: file(TEAM_FILE, TEAM),
            workflows: vec![
                file("workflows/feature.md", FEATURE),
                file("workflows/signed.md", SIGNED),
                file("workflows/review.md", REVIEW),
                file("workflows/lands.md", LANDS),
                file("workflows/asks.md", ASKS),
            ],
            patrols: vec![],
            at: None,
            rules: false,
            roles: vec![repo("acme/dev"), repo("acme/boss")],
            labels: RepoLabels::default(),
        })
        .unwrap()
    }

    fn labels(labels: &[&str]) -> Vec<String> {
        labels.iter().map(|label| label.to_string()).collect()
    }

    fn record(stage: &str, declared: Declared) -> StageRecord {
        StageRecord {
            stage: stage.to_string(),
            stated: Stated::Declared(declared),
            by: Author::Worker {
                consult: false,
                claim: format!("claim of {stage}"),
                role_commit: "r".to_string(),
                team_commit: "t".to_string(),
                harness: Harness::ClaudeCode,
                model: None,
            },
            blocked_by: Default::default(),
            text: "done".to_string(),
        }
    }

    #[test]
    fn an_issue_carrying_the_label_a_workflow_names_enters_at_its_first_stage() {
        use crate::team::tests::{FEATURE, one};
        let team = one(&format!("label: ready-for-agent\n{FEATURE}"));
        let Due::Worker(entry) = due(&team, &labels(&["ready-for-agent"]), &[], None) else {
            panic!("a worker is due")
        };
        assert_eq!((entry.workflow.name.as_str(), entry.stage.name.as_str()), ("feature", "plan"));
        // Its `flow:<name>` it no longer takes: that names no workflow now.
        let why = said(due(&team, &labels(&["flow:feature"]), &[], None));
        assert_eq!(why, "human: its flow label flow:feature names no workflow of the team");
        // …and without `label`, the ready label is an ordinary one.
        let plain = one(FEATURE);
        assert_eq!(
            said(due(&plain, &labels(&["ready-for-agent"]), &[], None)),
            "nothing: it carries no flow label"
        );
    }

    #[test]
    fn an_issue_enters_at_the_first_stage_of_the_workflow_its_flow_label_names() {
        let team = team();
        let Due::Worker(entry) = due(&team, &labels(&["bug", "flow:feature"]), &[], None) else {
            panic!("a worker is due")
        };
        assert_eq!(entry.workflow.name, "feature");
        assert_eq!(entry.stage.name, "build");
        assert_eq!((entry.role.name.as_str(), entry.work.skill.as_str()), ("dev", "implement"));
        assert_eq!(
            (entry.role.dir.repo.as_str(), entry.role.commit.as_str()),
            ("acme/dev", "acme/dev@1")
        );
    }

    /// What is due, in a few words.
    fn said(due: Due) -> String {
        match due {
            Due::Worker(entry) => format!("{} by {}", entry.stage.name, entry.role.name),
            Due::Consult(entry) => {
                format!("consult on {} by {}", entry.stage.name, entry.role.name)
            }
            Due::Lander(lander) => format!("PR #{} joined by {}", lander.pr, lander.role.name),
            Due::Human(why) => format!("human: {why}"),
            Due::Nothing(why) => format!("nothing: {why}"),
        }
    }

    #[test]
    fn the_stage_records_say_which_stage_a_worker_is_due_on() {
        let team = team();
        let flow = labels(&["flow:review"]);
        let after = |records: &[StageRecord]| said(due(&team, &flow, records, None));
        assert_eq!(after(&[]), "build by dev");
        let built = [record("build", Declared::Pass)];
        assert_eq!(after(&built), "check by boss");
        let back = [&built[..], &[record("check", Declared::Reject)]].concat();
        assert_eq!(after(&back), "build by dev");
        // Returned from a park, it is at the stage that parked it.
        assert_eq!(after(&[record("build", Declared::Blocked)]), "build by dev");
        let done = [&built[..], &[record("check", Declared::Pass)]].concat();
        assert_eq!(after(&done), "nothing: its workflow has ended");
        // `build` names no consult: it escalates for the watch.
        assert_eq!(
            after(&[record("build", Declared::Escalate)]),
            "human: the stage `build` declared `escalate`, which it hands to a human"
        );
    }

    #[test]
    fn an_escalate_makes_the_consult_its_stage_names_due_and_a_ruling_the_stage_again() {
        let team = team();
        let flow = labels(&["flow:asks"]);
        let consult = |declared| StageRecord {
            by: Author::Worker {
                consult: true,
                claim: "claim of the consult".to_string(),
                role_commit: "r".to_string(),
                team_commit: "t".to_string(),
                harness: Harness::ClaudeCode,
                model: None,
            },
            ..record("build", declared)
        };
        let asked = [record("build", Declared::Escalate)];
        let Due::Consult(entry) = due(&team, &flow, &asked, None) else {
            panic!("a consult is due")
        };
        assert_eq!(entry.stage.name, "build");
        assert_eq!((entry.role.name.as_str(), entry.work.skill.as_str()), ("boss", "implement"));
        assert_eq!(entry.role.dir.repo.as_str(), "acme/boss");
        let ruled = [&asked[..], &[consult(Declared::Ruling)]].concat();
        assert_eq!(said(due(&team, &flow, &ruled, None)), "build by dev");
        // The chain is one role long: a consult's own `escalate` goes to a human.
        let through = [&asked[..], &[consult(Declared::Escalate)]].concat();
        assert_eq!(
            said(due(&team, &flow, &through, None)),
            "human: the consult of the stage `build` let it through"
        );
    }

    #[test]
    fn a_pass_that_leads_to_land_makes_the_join_of_the_issue_s_pr_due() {
        let team = team();
        let flow = labels(&["flow:lands"]);
        let built = [record("build", Declared::Pass)];
        // An open PR declares nothing while a stage is still to be performed.
        assert_eq!(said(due(&team, &flow, &[], Some(70))), "build by dev");
        let Due::Lander(lander) = due(&team, &flow, &built, Some(70)) else {
            panic!("a landing worker is due")
        };
        assert_eq!((lander.role.name.as_str(), lander.work.skill.as_str()), ("boss", "implement"));
        assert_eq!((lander.role.dir.repo.as_str(), lander.pr), ("acme/boss", 70));
        assert_eq!(lander.workflow.name, "lands");
        // With no PR there is nothing to join: a human's.
        assert_eq!(
            said(due(&team, &flow, &built, None)),
            "human: its last stage passed it to the landing train, and no open PR closes it: \
             there is nothing to join"
        );
    }

    #[test]
    fn an_issue_no_worker_is_due_on_says_why() {
        let team = team();
        let why = |carried: &[&str]| said(due(&team, &labels(carried), &[], None));
        // No flow label: it was never handed to the fleet.
        assert_eq!(why(&["ready-for-agent"]), "nothing: it carries no flow label");
        // A flow label that names no one workflow hands it to a human.
        assert!(
            why(&["flow:feature", "flow:signed"])
                .starts_with("human: it carries 2 flow labels (flow:feature, flow:signed)")
        );
        assert_eq!(
            why(&["flow:chore"]),
            "human: its flow label flow:chore names no workflow of the team"
        );
        assert_eq!(
            why(&["flow:signed"]),
            "nothing: its stage \"sign-off\" waits for a human: no worker performs it"
        );
    }

    #[test]
    fn an_escalated_issue_followed_the_workflow_its_escalation_says_it_left() {
        let team = team();
        let name = |labels: &[&str], bodies: &[&str]| {
            followed(&team, &self::labels(labels), bodies.iter().copied())
                .map(|workflow| workflow.name.clone())
        };
        let left = escalation_comment("c1", Some("review"), "stuck");
        let earlier = escalation_comment("c0", Some("feature"), "stuck before");
        let bodies = [earlier.as_str(), "chat", left.as_str()];
        assert_eq!(name(&["ready-for-human"], &bodies), Ok("review".to_string()));
        // A flow label it carries is what it follows, whatever it left.
        assert_eq!(name(&["flow:signed"], &bodies), Ok("signed".to_string()));
        assert!(name(&["flow:chore"], &bodies).unwrap_err().contains("names no workflow"));
        let why = name(&["bug"], &["chat"]).unwrap_err();
        assert_eq!(why, "it carries no flow label, and no escalation of it says which it left");
        let gone = escalation_comment("c2", Some("chore"), "stuck");
        let why = name(&[], &[gone.as_str()]).unwrap_err();
        assert!(why.starts_with("it was escalated from the workflow chore, which is no"), "{why}");
        // An escalation that says none — an unbound repo's, or of an issue
        // that followed no one workflow — is no answer.
        let silent = escalation_comment("c3", None, "stuck");
        assert!(name(&[], &[left.as_str(), silent.as_str()]).is_err());
    }
}
