//! A worker's start in its role (docs/adr/0009): the stage an issue's worker
//! is to be started on, what the fleet adds to the worker launch command of a
//! bound repo, and whether the command takes it. What is added is fixed: nothing here is
//! configurable, and an unbound repo's command never comes this way.

use crate::position::Position;
use crate::stage_record::{StageRecord, history};
use crate::team::{FLOW_LABEL, Role, Stage, Team, Work, Workflow};

/// Claude Code's flag for a directory added to the one it was started in.
pub const ADD_DIR: &str = "--add-dir";
/// Claude Code's flag for which settings it loads.
pub const SETTING_SOURCES: &str = "--setting-sources";
/// Claude Code's flag for the model of the session.
pub const MODEL: &str = "--model";
/// Every flag the fleet adds: what a bound repo's worker launch command must take.
pub const FLAGS: [&str; 3] = [ADD_DIR, SETTING_SOURCES, MODEL];

/// The settings a worker loads: those of the directory it starts in — its
/// role's — and nothing of the host user's.
const SOURCES: &str = "project";
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

/// The stage an issue is dispatched to, and who performs it.
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

/// What an issue's place in its workflow calls for while nobody holds it.
#[derive(Debug, Clone)]
pub enum Due<'t> {
    /// A worker is to be started on this stage.
    Worker(Entry<'t>),
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
/// starts a worker on, in the workflow of `team` its flow label names, or —
/// its last stage having passed to the landing train — the join of that PR.
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
    let roleless = |work: &Work| Due::Nothing(format!("the team has no role {:?}", work.role));
    if let Some((stage, work)) = position.start() {
        return match team.role(&work.role) {
            Some(role) => Due::Worker(Entry { workflow, stage, work, role }),
            None => roleless(work),
        };
    }
    let waits = |stage: &Stage, on: &str| {
        Due::Nothing(format!("its stage {:?} waits for {on}", stage.name))
    };
    match position {
        Position::Escalating(stage, reason) => Due::Human(reason.said(stage)),
        Position::Consult(stage, _) => waits(stage, "its consult"),
        Position::Perform(stage, _) | Position::Waiting(stage) | Position::Parked(stage) => {
            waits(stage, "a human: no worker performs it")
        }
        Position::Landing => match (&workflow.land, pr) {
            (Some(work), Some(pr)) => match team.role(&work.role) {
                Some(role) => Due::Lander(Lander { workflow, work, role, pr }),
                None => roleless(work),
            },
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

/// The facts of one stage: what its worker, and the commands it runs, read
/// from the environment.
#[derive(Debug, Clone)]
pub struct Facts<'a> {
    pub issue: u64,
    /// The workflow's name: what follows `flow:` in the issue's flow label.
    pub flow: &'a str,
    pub stage: &'a str,
    pub role: &'a str,
    pub skill: &'a str,
    /// Whether the stage may commit.
    pub writes: bool,
    /// The commits of the role's repo and the team's the stage runs under.
    pub role_commit: &'a str,
    pub team_commit: &'a str,
}

impl Facts<'_> {
    /// The variables a worker's environment carries them in.
    pub fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("AFKCONVOY_ISSUE", self.issue.to_string()),
            ("AFKCONVOY_FLOW", self.flow.to_string()),
            ("AFKCONVOY_STAGE", self.stage.to_string()),
            ("AFKCONVOY_ROLE", self.role.to_string()),
            ("AFKCONVOY_SKILL", self.skill.to_string()),
            ("AFKCONVOY_WRITES", self.writes.to_string()),
            (ROLE_COMMIT_ENV, self.role_commit.to_string()),
            (TEAM_COMMIT_ENV, self.team_commit.to_string()),
        ]
    }
}

/// Where one worker starts, and what is added to it.
#[derive(Debug, Clone)]
pub struct Start<'a> {
    /// The fleet's copy of the role's repo: the worker's working directory.
    pub role_dir: &'a str,
    /// The issue's worktree.
    pub worktree: &'a str,
    /// The fleet's copy of the team's repo.
    pub team_dir: &'a str,
    /// The stage's model, when its workflow names one.
    pub model: Option<&'a str>,
    pub facts: Facts<'a>,
}

/// The line a terminal opened on the issue's worktree runs to start a worker
/// in its role with the worker launch command `command`: change to the role's
/// copy, put the stage's facts in the environment, and start the command with
/// the worktree and the team's copy added, the user's settings left out, and
/// the stage's model.
pub fn in_role(command: &str, start: &Start) -> String {
    let mut line = format!("cd {} && export {ADDED_RULES}=1 {NO_MEMORY}=1", quoted(start.role_dir));
    for (name, value) in start.facts.env() {
        line.push_str(&format!(" {name}={}", quoted(&value)));
    }
    line.push_str(&format!(
        " && {command} {ADD_DIR} {} {ADD_DIR} {} {SETTING_SOURCES} {SOURCES}",
        quoted(start.worktree),
        quoted(start.team_dir)
    ));
    if let Some(model) = start.model {
        line.push_str(&format!(" {MODEL} {}", quoted(model)));
    }
    line
}

/// `text` as one word of a shell line, whatever it holds.
pub fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The shell line whose answer says which flags `command` takes
/// (`takes_flags` reads it).
pub fn flags_question(command: &str) -> String {
    format!("{command} --help")
}

/// Whether the worker launch command `command` takes what a bound repo adds
/// to it, given what the worker's shell printed for `flags_question` (`None`:
/// it failed, or printed nothing). An error names the reason: used anyway,
/// every worker would start outside its role, or not at all.
pub fn takes_flags(command: &str, answer: Option<&str>) -> Result<(), String> {
    let refused = |why: String| {
        Err(format!(
            "this repo is bound to a team, whose workers are started in their roles, and the \
             worker launch command {command:?} cannot do that: {why}. A bound repo's worker \
             launch command must end in Claude Code taking flags"
        ))
    };
    let question = flags_question(command);
    let Some(answer) = answer.filter(|answer| !answer.trim().is_empty()) else {
        return refused(format!("`{question}` failed in a login shell, or said nothing"));
    };
    let missing: Vec<&str> = FLAGS.iter().copied().filter(|flag| !answer.contains(flag)).collect();
    if missing.is_empty() {
        Ok(())
    } else {
        refused(format!("`{question}` names no {}", missing.join(", no ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::Declared;
    use crate::team::{File, RoleRepo, Sources, TEAM_FILE, load};

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

    const LANDS: &str = "\
stages:
  - name: build
    by: dev
    do: implement
    on: { pass: land }
land: { by: boss, do: implement }
";

    fn file(path: &str, yaml: &str) -> File {
        File { path: path.to_string(), yaml: serde_yaml::from_str(yaml).map_err(|e| e.to_string()) }
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
            ],
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
            declared,
            claim: format!("claim of {stage}"),
            role_commit: "r".to_string(),
            team_commit: "t".to_string(),
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
        assert_eq!((entry.work.role.as_str(), entry.work.skill.as_str()), ("dev", "implement"));
        assert_eq!(
            (entry.role.repo.as_str(), entry.role.commit.as_str()),
            ("acme/dev", "acme/dev@1")
        );
        assert_eq!(entry.stage.model.as_deref(), Some("opus"));
    }

    /// What is due, in a few words.
    fn said(due: Due) -> String {
        match due {
            Due::Worker(entry) => format!("{} by {}", entry.stage.name, entry.work.role),
            Due::Lander(lander) => format!("PR #{} joined by {}", lander.pr, lander.work.role),
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
    fn a_pass_that_leads_to_land_makes_the_join_of_the_issue_s_pr_due() {
        let team = team();
        let flow = labels(&["flow:lands"]);
        let built = [record("build", Declared::Pass)];
        // An open PR declares nothing while a stage is still to be performed.
        assert_eq!(said(due(&team, &flow, &[], Some(70))), "build by dev");
        let Due::Lander(lander) = due(&team, &flow, &built, Some(70)) else {
            panic!("a landing worker is due")
        };
        assert_eq!((lander.work.role.as_str(), lander.work.skill.as_str()), ("boss", "implement"));
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

    fn start<'a>(model: Option<&'a str>) -> Start<'a> {
        Start {
            role_dir: "/copies/acme/dev/abc",
            worktree: "/wt/issue 7",
            team_dir: "/copies/acme/team/def",
            model,
            facts: Facts {
                issue: 7,
                flow: "feature",
                stage: "build",
                role: "dev",
                skill: "implement",
                writes: false,
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
             AFKCONVOY_WRITES='false' AFKCONVOY_ROLE_COMMIT='abc' AFKCONVOY_TEAM_COMMIT='def' && \
             kim --yolo --add-dir '/wt/issue 7' --add-dir '/copies/acme/team/def' \
             --setting-sources project --model 'opus'"
        );
    }

    #[test]
    fn a_stage_with_no_model_names_none() {
        let line = in_role("claude", &start(None));
        assert!(line.ends_with("--setting-sources project"), "{line}");
        assert!(!line.contains("--model"), "{line}");
    }

    #[test]
    fn nothing_a_path_or_a_name_holds_is_read_by_the_shell() {
        let odd = Start { role_dir: "/it's here/$HOME", ..start(Some("a b;c")) };
        let line = in_role("claude", &odd);
        assert!(line.starts_with("cd '/it'\\''s here/$HOME' && "), "{line}");
        assert!(line.ends_with("--model 'a b;c'"), "{line}");
    }

    const HELP: &str = "Usage: claude [options]\n  --add-dir <directories...>\n  --model <model>\n  \
                        --setting-sources <sources>\n";

    #[test]
    fn a_command_that_takes_the_flags_is_not_refused() {
        assert_eq!(flags_question("kim --yolo"), "kim --yolo --help");
        assert_eq!(takes_flags("kim --yolo", Some(HELP)), Ok(()));
    }

    #[test]
    fn a_command_that_does_not_take_the_flags_is_refused_naming_the_reason() {
        let old = HELP.replace("--setting-sources", "--settings").replace("--add-dir", "--dir");
        let why = takes_flags("agent", Some(&old)).unwrap_err();
        assert!(why.contains("`agent --help` names no --add-dir, no --setting-sources."), "{why}");
        assert!(why.contains("bound to a team") && why.contains("\"agent\""), "{why}");
        for silent in [None, Some(""), Some(" \n")] {
            let why = takes_flags("agent ; true", silent).unwrap_err();
            assert!(why.contains("`agent ; true --help` failed in a login shell"), "{why}");
        }
    }
}
