//! A worker's start in its role (docs/adr/0009): the stage an issue's worker
//! is to be started on, what the fleet adds to the worker launch command of a
//! bound repo, and which coding agent the command starts (docs/adr/0013).
//! What is added is fixed for each coding agent: nothing here is
//! configurable, and an unbound repo's command never comes this way.

use flux_rs::attrs::*;

use crate::line::Line;
use crate::patrol::PATROL_ENV;
use crate::position::Position;
use crate::precedent::PROPOSAL_ENV;
use crate::stage_record::{StageRecord, history};
use crate::team::{FLOW_LABEL, Role, SKILLS, Stage, Team, Work, Workflow};
use crate::transition::escalated_from;

/// Claude Code's flag for a directory added to the one it was started in.
pub const ADD_DIR: &str = "--add-dir";
/// Claude Code's flag for which settings it loads.
pub const SETTING_SOURCES: &str = "--setting-sources";
/// Claude Code's flag for the model of the session.
pub const MODEL: &str = "--model";
/// Claude Code's flag for settings added to those it loads.
pub const SETTINGS: &str = "--settings";
/// Every flag the fleet adds to Claude Code.
const CLAUDE_FLAGS: [&str; 4] = [ADD_DIR, SETTING_SOURCES, SETTINGS, MODEL];

/// pi's flag for an extension it is to load, whatever it discovers.
pub const EXTENSION: &str = "--extension";
/// pi's flag for a directory of skills it is to load, whatever it discovers.
pub const SKILL: &str = "--skill";
/// pi's flags that leave out what it discovers on its host and in the
/// directory it starts in: extensions, skills, prompt templates, themes, and
/// the rules — the host user's own among them, which nothing leaves out
/// alone, so the fleet's extension loads the role's (`RULES_OF`).
const PI_DISCOVERS_NOTHING: [&str; 5] = [
    "--no-extensions",
    "--no-skills",
    "--no-prompt-templates",
    "--no-themes",
    "--no-context-files",
];
/// pi's flag that answers its folder-trust question: no. What it would have
/// trusted — a `.pi` directory — is no part of a role.
const PI_TRUSTS_NOTHING: &str = "--no-approve";
/// Every flag the fleet adds to pi.
const PI_FLAGS: [&str; 9] = [
    PI_DISCOVERS_NOTHING[0],
    PI_DISCOVERS_NOTHING[1],
    PI_DISCOVERS_NOTHING[2],
    PI_DISCOVERS_NOTHING[3],
    PI_DISCOVERS_NOTHING[4],
    PI_TRUSTS_NOTHING,
    EXTENSION,
    SKILL,
    MODEL,
];

/// The coding agent a bound repo's worker launch command starts
/// (docs/adr/0013): what the fleet adds to the command to put a worker in
/// its role is that agent's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodingAgent {
    ClaudeCode,
    Pi,
}

impl CodingAgent {
    /// Every coding agent a bound repo's worker can be started with, in the
    /// order a command is asked which it starts.
    pub const ALL: [CodingAgent; 2] = [CodingAgent::ClaudeCode, CodingAgent::Pi];

    /// What a human calls it.
    pub fn name(self) -> &'static str {
        match self {
            CodingAgent::ClaudeCode => "Claude Code",
            CodingAgent::Pi => "pi",
        }
    }

    /// Every flag the fleet adds to it: what a worker launch command that
    /// starts it must take.
    pub fn flags(self) -> &'static [&'static str] {
        match self {
            CodingAgent::ClaudeCode => &CLAUDE_FLAGS,
            CodingAgent::Pi => &PI_FLAGS,
        }
    }
}

/// The settings a worker loads: those of the directory it starts in — its
/// role's — and nothing of the host user's.
const SOURCES: &str = "project";
/// The events of Claude Code at which a worker's state changes: what the
/// worker backend hears of, to report it (`Start::report`). With the user's
/// settings left out, nothing else tells it.
const STATE_EVENTS: [&str; 12] = [
    "SessionStart",
    "UserPromptSubmit",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "SubagentStart",
    "SubagentStop",
    "TeammateIdle",
    "PostCompact",
    "Stop",
    "StopFailure",
    "SessionEnd",
];
/// How long Claude Code waits on one report, in seconds: a worker backend
/// that does not answer holds no tool call up.
const REPORT_TIMEOUT: u64 = 10;
/// Has Claude Code load the rules of the directories added to it. Their hooks
/// are not loaded: what a repo must enforce belongs in its gate.
const ADDED_RULES: &str = "CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD";
/// Keeps Claude Code from carrying what one worker learned to the next on
/// this host: a role is what its repo says, the same on every host.
const NO_MEMORY: &str = "CLAUDE_CODE_DISABLE_AUTO_MEMORY";

/// The variables a worker's environment carries the commits of its role's
/// repo and of the team's in: the command that declares a verdict reads them.
pub const ROLE_COMMIT_ENV: &str = "AFKCONVOY_ROLE_COMMIT";
pub const TEAM_COMMIT_ENV: &str = "AFKCONVOY_TEAM_COMMIT";
/// The variables it carries the issue its worker is on and the worker's role
/// in: the command that takes an issue in reads them.
pub const ISSUE_ENV: &str = "AFKCONVOY_ISSUE";
pub const ROLE_ENV: &str = "AFKCONVOY_ROLE";
/// The variables the guard reads its stage's limits from (`guard::Limits`).
pub const STAGE_ENV: &str = "AFKCONVOY_STAGE";
pub const WRITES_ENV: &str = "AFKCONVOY_WRITES";
pub const CONSULT_ENV: &str = "AFKCONVOY_CONSULT";
pub const BASE_BRANCH_ENV: &str = "AFKCONVOY_BASE_BRANCH";
pub const ESCALATE_LABEL_ENV: &str = "AFKCONVOY_ESCALATE_LABEL";
pub const LOCK_LABEL_ENV: &str = "AFKCONVOY_LOCK_LABEL";
/// The variables the fleet's extension for pi reads what Claude Code is told
/// by flags in: the shell line that runs the guard, and the directories
/// added to the role's — the worktree and the team's copy — whose rules it
/// loads beside the role's.
pub const GUARD_ENV: &str = "AFKCONVOY_GUARD";
pub const WORKTREE_ENV: &str = "AFKCONVOY_WORKTREE";
pub const TEAM_DIR_ENV: &str = "AFKCONVOY_TEAM_DIR";

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

/// The names of the workflows an issue's labels name, in the labels' order.
pub fn flows(labels: &[String]) -> Vec<&str> {
    labels.iter().filter_map(|label| label.strip_prefix(FLOW_LABEL)).collect()
}

/// The workflow of `team` an issue carrying `labels` follows: the one its
/// flow label names. An error says why it follows none.
#[spec(fn(&Team[@roles], &[String]) -> Result<&Workflow{v: v.roles == roles}, String>)]
pub fn workflow_of<'t>(team: &'t Team, labels: &[String]) -> Result<&'t Workflow, String> {
    let named = flows(labels);
    let name = match named.as_slice() {
        [] => return Err("it carries no flow label".to_string()),
        [name] => *name,
        several => {
            let labels: Vec<String> =
                several.iter().map(|name| format!("{FLOW_LABEL}{name}")).collect();
            return Err(format!(
                "it carries {} flow labels ({}): an issue follows one workflow",
                labels.len(),
                labels.join(", ")
            ));
        }
    };
    team.workflow(name)
        .ok_or_else(|| format!("its flow label {FLOW_LABEL}{name} names no workflow of the team"))
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
    if !flows(labels).is_empty() {
        return workflow_of(team, labels);
    }
    let Some(left) = escalated_from(bodies) else {
        return Err("it carries no flow label, and no escalation of it says which it left".into());
    };
    team.workflow(&left).ok_or_else(|| {
        format!("it was escalated from {FLOW_LABEL}{left}, which names no workflow of the team")
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
    /// issue's PR and join the landing train.
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
        Err(why) if flows(labels).is_empty() => return Due::Nothing(why),
        Err(why) => return Due::Human(why),
    };
    let position = history(workflow, records).place.position;
    if let Some((stage, work)) = position.start() {
        return Due::Worker(Entry { workflow, stage, work, role: team.role_of(work) });
    }
    let waits = |stage: &Stage, on: &str| {
        Due::Nothing(format!("its stage {:?} waits for {on}", stage.name))
    };
    match position {
        Position::Escalating(stage, reason) => Due::Human(reason.said(stage)),
        Position::Consult(stage, work) => {
            Due::Consult(Entry { workflow, stage, work, role: team.role_of(work) })
        }
        Position::Perform(stage, _) | Position::Waiting(stage) | Position::Parked(stage) => {
            waits(stage, "a human: no worker performs it")
        }
        Position::Landing => match (&workflow.land, pr) {
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
        /// The workflow's name: what follows `flow:` in the issue's flow label.
        flow: &'a str,
        stage: &'a str,
    },
    /// A run of the patrol of this name: no issue, and no stage.
    Patrol(&'a str),
    /// The proposal, as a precedent, of a ruling a human gave this issue: no
    /// stage, and the issue is not its worker's.
    Proposal { issue: u64 },
}

/// The facts of one start: what its worker, and the commands it runs, read
/// from the environment.
#[derive(Debug, Clone)]
pub struct Facts<'a> {
    pub on: On<'a>,
    pub role: &'a str,
    pub skill: &'a str,
    /// Whether the worker may commit. A patrol's never may; a proposal's
    /// does, in its own clone of the team's repo.
    pub writes: bool,
    /// Whether the worker is a consult, which writes no code whatever its
    /// stage may.
    pub consult: bool,
    /// The repo's base branch, and the labels that are the fleet's to move
    /// beside the flow and stage labels: the team's escalate label and the
    /// repo's lock label.
    pub base_branch: &'a str,
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
            On::Proposal { issue } => vec![(PROPOSAL_ENV, issue.to_string())],
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
        env
    }
}

/// Where one worker starts, and what is added to it.
#[derive(Debug, Clone)]
pub struct Start<'a> {
    /// The fleet's copy of the role's repo: the worker's working directory.
    pub role_dir: &'a str,
    /// The issue's worktree, or the checkout of the base branch a patrol's
    /// worker or a proposal's is given.
    pub worktree: &'a str,
    /// The fleet's copy of the team's repo.
    pub team_dir: &'a str,
    /// The stage's model, or the patrol's, when its definition names one.
    pub model: Option<&'a str>,
    /// The shell line that runs the guard: this tool's `guard` subcommand.
    pub guard: &'a str,
    /// The coding agent the worker launch command starts, with what only a
    /// start of that agent is given.
    pub with: With<'a>,
    pub facts: Facts<'a>,
}

/// What a start is given that only one coding agent takes.
#[derive(Debug, Clone, Copy)]
pub enum With<'a> {
    ClaudeCode {
        /// The shell line that tells the worker backend of the worker's
        /// state: it is given what Claude Code says of an event on standard
        /// input.
        report: &'a str,
    },
    Pi {
        /// The fleet's extension for pi, as a file on this host: the guard,
        /// the role's own limits, and the rules of the role and of what is
        /// added to it.
        extension: &'a str,
        /// The extension that tells the worker backend of the worker's
        /// state, as a file on this host. `None` where the backend
        /// installed none: nothing is told.
        report: Option<&'a str>,
    },
}

impl With<'_> {
    pub fn agent(&self) -> CodingAgent {
        match self {
            With::ClaudeCode { .. } => CodingAgent::ClaudeCode,
            With::Pi { .. } => CodingAgent::Pi,
        }
    }
}

/// `text` as a JSON string, quotes and all.
fn json_text(text: &str) -> String {
    let mut json = String::from("\"");
    for c in text.chars() {
        match c {
            '"' | '\\' => {
                json.push('\\');
                json.push(c);
            }
            // A shell line holds no control character worth carrying.
            c if c.is_control() => json.push(' '),
            c => json.push(c),
        }
    }
    json.push('"');
    json
}

/// The settings added to a worker's: the guard, a hook Claude Code runs
/// before every tool call with the line `guard`, and the report, a hook it
/// runs with the line `report` wherever the worker's state changes. They are
/// added to the role's own, so a role's hooks run beside them and no role is
/// without the guard (docs/adr/0012) — or unheard of by the worker backend,
/// which would read a worker it has no report of off its terminal alone.
fn added_settings(guard: &str, report: &str) -> String {
    let guard = format!("{{\"type\":\"command\",\"command\":{}}}", json_text(guard));
    let report = format!(
        "{{\"type\":\"command\",\"command\":{},\"timeout\":{REPORT_TIMEOUT}}}",
        json_text(report)
    );
    let mut settings = format!("{{\"hooks\":{{\"PreToolUse\":[{{\"hooks\":[{guard},{report}]}}]");
    for event in STATE_EVENTS {
        settings.push_str(&format!(",\"{event}\":[{{\"hooks\":[{report}]}}]"));
    }
    settings.push_str("}}");
    settings
}

/// The line a terminal opened on the issue's worktree runs to start a worker
/// in its role with the worker launch command `command`: change to the role's
/// copy, put the stage's facts in the environment, and start the command with
/// the worktree and the team's copy added, the user's configuration left
/// out, the guard and the report added, and the stage's model — each in the
/// way of the coding agent the command starts.
pub fn in_role(command: &str, start: &Start) -> String {
    let mut line = Line::new().fixed("cd ").word(start.role_dir).fixed(" && export");
    line = match start.with {
        With::ClaudeCode { .. } => {
            line.fixed(" ").fixed(ADDED_RULES).fixed("=1 ").fixed(NO_MEMORY).fixed("=1")
        }
        // What Claude Code is told by flags, pi's extension reads here.
        With::Pi { .. } => {
            let line = line.fixed(" ").fixed(GUARD_ENV).fixed("=").word(start.guard);
            let line = line.fixed(" ").fixed(WORKTREE_ENV).fixed("=").word(start.worktree);
            line.fixed(" ").fixed(TEAM_DIR_ENV).fixed("=").word(start.team_dir)
        }
    };
    for (name, value) in start.facts.env() {
        line = line.fixed(" ").fixed(name).fixed("=").word(&value);
    }
    let mut line = line.fixed(" && ").launch(command);
    match start.with {
        With::ClaudeCode { report } => {
            line = line
                .fixed(" ")
                .fixed(ADD_DIR)
                .fixed(" ")
                .word(start.worktree)
                .fixed(" ")
                .fixed(ADD_DIR)
                .fixed(" ")
                .word(start.team_dir)
                .fixed(" ")
                .fixed(SETTING_SOURCES)
                .fixed(" ")
                .fixed(SOURCES)
                .fixed(" ")
                .fixed(SETTINGS)
                .fixed(" ")
                .word(&added_settings(start.guard, report));
        }
        With::Pi { extension, report } => {
            for flag in PI_DISCOVERS_NOTHING {
                line = line.fixed(" ").fixed(flag);
            }
            line = line.fixed(" ").fixed(PI_TRUSTS_NOTHING);
            for extension in [Some(extension), report].into_iter().flatten() {
                line = line.fixed(" ").fixed(EXTENSION).fixed(" ").word(extension);
            }
            // pi discovers no skills where a role keeps them.
            let skills = format!("{}/{SKILLS}", start.role_dir.trim_end_matches('/'));
            line = line.fixed(" ").fixed(SKILL).fixed(" ").word(&skills);
        }
    }
    if let Some(model) = start.model {
        line = line.fixed(" ").fixed(MODEL).fixed(" ").word(model);
    }
    line.done()
}

/// The shell line whose answer says which flags `command` takes
/// (`starts` reads it).
pub fn flags_question(command: &str) -> String {
    Line::new().launch(command).fixed(" --help").done()
}

/// The coding agent the worker launch command `command` starts, given what
/// the worker's shell printed for `flags_question` (`None`: it failed, or
/// printed nothing): the first whose every flag the answer names. An error
/// is a command that takes no agent's, naming the reason: used anyway, every
/// worker would start outside its role, or not at all.
pub fn starts(command: &str, answer: Option<&str>) -> Result<CodingAgent, String> {
    let refused = |why: String| {
        Err(format!(
            "this repo is bound to a team, whose workers are started in their roles, and the \
             worker launch command {command:?} cannot do that: {why}. A bound repo's worker \
             launch command must end in Claude Code or pi taking flags"
        ))
    };
    let question = flags_question(command);
    let Some(answer) = answer.filter(|answer| !answer.trim().is_empty()) else {
        return refused(format!("`{question}` failed in a login shell, or said nothing"));
    };
    let mut missing: Vec<String> = vec![];
    for agent in CodingAgent::ALL {
        let lacks: Vec<&str> =
            agent.flags().iter().copied().filter(|flag| !answer.contains(flag)).collect();
        if lacks.is_empty() {
            return Ok(agent);
        }
        missing.push(format!("of {}'s flags, no {}", agent.name(), lacks.join(", no ")));
    }
    refused(format!("`{question}` names, {}", missing.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line::quoted;
    use crate::position::Declared;
    use crate::stage_record::{Author, Stated};
    use crate::team::{File, RoleRepo, Sources, TEAM_FILE, load};
    use crate::transition::escalation_comment;

    const TEAM: &str = "roles:\n  dev: acme/dev\n  boss: acme/boss\n";
    const FEATURE: &str = "\
stages:
  - name: build
    by: dev
    do: implement
    model: opus
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
            (name.to_string(), Ok(RoleRepo { commit: format!("{name}@1"), skills }))
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
            repos: vec![repo("acme/dev"), repo("acme/boss")],
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
            },
            blocked_by: vec![],
            text: "done".to_string(),
        }
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
            (entry.role.repo.as_str(), entry.role.commit.as_str()),
            ("acme/dev", "acme/dev@1")
        );
        assert_eq!(entry.stage.model.as_deref(), Some("opus"));
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
        assert_eq!(
            after(&[record("build", Declared::Escalate)]),
            "human: the stage `build` declared `escalate`, and it names no consult"
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
            },
            ..record("build", declared)
        };
        let asked = [record("build", Declared::Escalate)];
        let Due::Consult(entry) = due(&team, &flow, &asked, None) else {
            panic!("a consult is due")
        };
        assert_eq!(entry.stage.name, "build");
        assert_eq!((entry.role.name.as_str(), entry.work.skill.as_str()), ("boss", "implement"));
        assert_eq!(entry.role.repo.as_str(), "acme/boss");
        let ruled = [&asked[..], &[consult(Declared::Ruling)]].concat();
        assert_eq!(said(due(&team, &flow, &ruled, None)), "build by dev");
        // The chain is one role long: a consult's own `escalate` goes to a human.
        let through = [&asked[..], &[consult(Declared::Escalate)]].concat();
        assert_eq!(
            said(due(&team, &flow, &through, None)),
            "human: the consult of the stage `build` let its `escalate` through"
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
        assert_eq!((lander.role.repo.as_str(), lander.pr), ("acme/boss", 70));
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
        assert!(why.starts_with("it was escalated from flow:chore, which names no"), "{why}");
        // An escalation that says none — an unbound repo's, or of an issue
        // that followed no one workflow — is no answer.
        let silent = escalation_comment("c3", None, "stuck");
        assert!(name(&[], &[left.as_str(), silent.as_str()]).is_err());
    }

    /// The settings that add the guard `/bin/afkconvoy guard` and the report
    /// `tell "$HOME"`.
    fn added() -> String {
        let report = r#"{"type":"command","command":"tell \"$HOME\"","timeout":10}"#;
        let mut added = format!(
            r#"{{"hooks":{{"PreToolUse":[{{"hooks":[{{"type":"command","command":"/bin/afkconvoy guard"}},{report}]}}]"#
        );
        for event in STATE_EVENTS {
            added.push_str(&format!(r#","{event}":[{{"hooks":[{report}]}}]"#));
        }
        added + "}}"
    }

    fn start<'a>(model: Option<&'a str>) -> Start<'a> {
        Start {
            role_dir: "/copies/acme/dev/abc",
            worktree: "/wt/issue 7",
            team_dir: "/copies/acme/team/def",
            model,
            guard: "/bin/afkconvoy guard",
            with: With::ClaudeCode { report: r#"tell "$HOME""# },
            facts: Facts {
                on: On::Stage { issue: 7, flow: "feature", stage: "build" },
                role: "dev",
                skill: "implement",
                writes: false,
                consult: false,
                base_branch: "main",
                escalate_label: "ready-for-human",
                lock_label: "fleet-lock",
                role_commit: "abc",
                team_commit: "def",
            },
        }
    }

    #[test]
    fn the_line_changes_to_the_role_first_and_adds_a_fixed_set() {
        let line = in_role("kim --yolo", &start(Some("opus")));
        assert_eq!(
            line,
            "cd '/copies/acme/dev/abc' && export CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1 \
             CLAUDE_CODE_DISABLE_AUTO_MEMORY=1 AFKCONVOY_ISSUE='7' AFKCONVOY_FLOW='feature' \
             AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' AFKCONVOY_SKILL='implement' \
             AFKCONVOY_WRITES='false' AFKCONVOY_CONSULT='false' AFKCONVOY_BASE_BRANCH='main' \
             AFKCONVOY_ESCALATE_LABEL='ready-for-human' AFKCONVOY_LOCK_LABEL='fleet-lock' \
             AFKCONVOY_ROLE_COMMIT='abc' AFKCONVOY_TEAM_COMMIT='def' && \
             kim --yolo --add-dir '/wt/issue 7' --add-dir '/copies/acme/team/def' \
             --setting-sources project --settings '"
                .to_string()
                + &added()
                + "' --model 'opus'"
        );
    }

    #[test]
    fn a_stage_with_no_model_names_none() {
        let line = in_role("claude", &start(None));
        assert!(
            line.ends_with(&format!("--setting-sources project --settings '{}'", added())),
            "{line}"
        );
        assert!(!line.contains("--model"), "{line}");
    }

    #[test]
    fn nothing_a_path_or_a_name_holds_is_read_by_the_shell() {
        let odd = Start { role_dir: "/it's here/$HOME", ..start(Some("a b;c")) };
        let line = in_role("claude", &odd);
        assert!(line.starts_with("cd '/it'\\''s here/$HOME' && "), "{line}");
        assert!(line.ends_with("--model 'a b;c'"), "{line}");
        // Nor by Claude Code, of the line that runs the guard.
        let odd = Start { guard: "/it's \"here\"/afk\\convoy guard", ..start(None) };
        let With::ClaudeCode { report } = odd.with else { panic!("a start of Claude Code") };
        let settings = added_settings(odd.guard, report);
        assert!(settings.contains(r#""command":"/it's \"here\"/afk\\convoy guard""#), "{settings}");
        assert!(settings.contains(r#""command":"tell \"$HOME\"","timeout":10"#), "{settings}");
        assert!(in_role("claude", &odd).ends_with(&format!("{SETTINGS} {}", quoted(&settings))));
    }

    const HELP: &str = "Usage: claude [options]\n  --add-dir <directories...>\n  --model <model>\n  \
                        --setting-sources <sources>\n  --settings <file-or-json>\n";
    const PI_HELP: &str = "Usage:\n  pi [options]\n  --model <pattern>\n  --extension, -e <path>\n  \
                           --no-extensions, -ne\n  --skill <path>\n  --no-skills, -ns\n  \
                           --no-prompt-templates, -np\n  --no-themes\n  --no-context-files, -nc\n  \
                           --approve, -a\n  --no-approve, -na\n";

    fn pi<'a>(model: Option<&'a str>, report: Option<&'a str>) -> Start<'a> {
        Start { with: With::Pi { extension: "/wt/.git/ext's.ts", report }, ..start(model) }
    }

    #[test]
    fn a_start_of_pi_adds_what_pi_takes_and_hands_its_extension_the_rest() {
        let line = in_role("pi", &pi(Some("opus"), Some("/home/kim/.pi/orca.ts")));
        assert_eq!(
            line,
            "cd '/copies/acme/dev/abc' && export AFKCONVOY_GUARD='/bin/afkconvoy guard' \
             AFKCONVOY_WORKTREE='/wt/issue 7' AFKCONVOY_TEAM_DIR='/copies/acme/team/def' \
             AFKCONVOY_ISSUE='7' AFKCONVOY_FLOW='feature' \
             AFKCONVOY_STAGE='build' AFKCONVOY_ROLE='dev' AFKCONVOY_SKILL='implement' \
             AFKCONVOY_WRITES='false' AFKCONVOY_CONSULT='false' AFKCONVOY_BASE_BRANCH='main' \
             AFKCONVOY_ESCALATE_LABEL='ready-for-human' AFKCONVOY_LOCK_LABEL='fleet-lock' \
             AFKCONVOY_ROLE_COMMIT='abc' AFKCONVOY_TEAM_COMMIT='def' && \
             pi --no-extensions --no-skills --no-prompt-templates --no-themes --no-context-files \
             --no-approve --extension '/wt/.git/ext'\\''s.ts' --extension '/home/kim/.pi/orca.ts' \
             --skill '/copies/acme/dev/abc/.claude/skills' --model 'opus'"
        );
        // Nothing of Claude Code's is in it.
        for claude in [ADD_DIR, SETTINGS, ADDED_RULES, NO_MEMORY] {
            assert!(!line.contains(claude), "{claude}: {line}");
        }
    }

    #[test]
    fn a_start_of_pi_with_no_report_and_no_model_names_neither() {
        let line = in_role("pi", &pi(None, None));
        assert!(
            line.ends_with(
                "--no-approve --extension '/wt/.git/ext'\\''s.ts' --skill \
                 '/copies/acme/dev/abc/.claude/skills'"
            ),
            "{line}"
        );
    }

    #[test]
    fn every_flag_added_to_a_coding_agent_is_one_its_command_is_asked_for() {
        let starts = [start(Some("opus")), pi(Some("opus"), Some("/orca.ts"))];
        for start in starts {
            let agent = start.with.agent();
            let line = in_role("agent", &start);
            let added = line.split(" && agent ").nth(1).unwrap();
            let flags = added.split(' ').filter(|word| word.starts_with("--"));
            let mut flags: Vec<&str> = flags.collect();
            flags.sort_unstable();
            flags.dedup();
            let mut asked = agent.flags().to_vec();
            asked.sort_unstable();
            assert_eq!(flags, asked, "{}", agent.name());
        }
    }

    #[test]
    fn a_command_is_told_by_the_flags_it_takes_which_coding_agent_it_starts() {
        assert_eq!(flags_question("kim --yolo"), "kim --yolo --help");
        assert_eq!(starts("kim --yolo", Some(HELP)), Ok(CodingAgent::ClaudeCode));
        assert_eq!(starts("glm", Some(PI_HELP)), Ok(CodingAgent::Pi));
    }

    #[test]
    fn a_command_that_takes_no_coding_agent_s_flags_is_refused_naming_the_reason() {
        let old = HELP.replace("--setting-sources", "--settings").replace("--add-dir", "--dir");
        let why = starts("agent", Some(&old)).unwrap_err();
        assert!(
            why.contains(
                "`agent --help` names, of Claude Code's flags, no --add-dir, no \
                 --setting-sources; of pi's flags, no --no-extensions, no --no-skills"
            ),
            "{why}"
        );
        assert!(why.contains("bound to a team") && why.contains("\"agent\""), "{why}");
        // A pi too old to leave its discovery out is not pi enough.
        let old = PI_HELP.replace("  --no-context-files, -nc\n", "");
        let why = starts("pi", Some(&old)).unwrap_err();
        assert!(why.contains("of pi's flags, no --no-context-files."), "{why}");
        for silent in [None, Some(""), Some(" \n")] {
            let why = starts("agent ; true", silent).unwrap_err();
            assert!(why.contains("`agent ; true --help` failed in a login shell"), "{why}");
        }
    }
}
