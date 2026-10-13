//! A team as its files define it, loaded into a value that cannot hold a
//! malformed workflow or patrol (docs/adr/0008, docs/adr/0010). `load` is the only way
//! to a `Team`: it yields one, or every fault with the file and key it is in.
//! Whoever holds a `Team` checks nothing again.

use flux_rs::attrs::*;

use crate::config::{ESCALATE_LABEL, NotADir, RepoDir};
use crate::harness::Harness;

/// The team file, at the top of a team's repo.
pub const TEAM_FILE: &str = "team.md";
/// The directory of a team's repo that holds one file per workflow.
pub const WORKFLOWS: &str = "workflows";
/// The directory of a team's repo that holds one file per patrol.
pub const PATROLS: &str = "patrols";
/// The directory of a role's repo that holds one entry per skill.
pub const SKILLS: &str = ".claude/skills";
/// The file of a team's repo that holds its precedents.
pub const PRECEDENTS: &str = "precedents.md";
/// A role's rules, in its directory: the file every harness loads them from,
/// so that they are written once (docs/adr/0013).
pub const RULES: &str = "CLAUDE.md";
/// What a flow label is made of: this, then the workflow's name.
pub const FLOW_LABEL: &str = "flow:";
/// How much rework an issue may have in total, where its workflow does not say.
pub const MAX_REWORK: u64 = 3;
/// How often one stage may reject in a row, where its workflow does not say.
pub const MAX_CONSECUTIVE_REWORK: u64 = 2;

/// Who performs a stage that no role does.
pub const HUMAN: &str = "human";
/// Where a `pass` leads that ends a workflow in the landing train.
pub const LAND: &str = "land";
/// Where a `pass` leads that ends a workflow in nothing more to do.
pub const DONE: &str = "done";

/// The key of the team file that says who proposes precedents.
const PRECEDENTS_KEY: &str = "precedents";
/// The key of a stage and of a patrol that names the harness its worker
/// runs: a key wherever `model` is one, and nowhere else.
const HARNESS_KEY: &str = "harness";
/// The key under which a stage, or a patrol, names its model.
const MODEL_KEY: &str = "model";

/// The keys of each map a definition holds: what `check`'s help must name.
pub const TEAM_KEYS: [&str; 3] = ["escalate_label", "roles", PRECEDENTS_KEY];
pub const PRECEDENTS_KEYS: [&str; 3] = ["by", "do", "max_open"];
pub const WORKFLOW_KEYS: [&str; 4] = ["max_rework", "max_consecutive_rework", "stages", LAND];
pub const STAGE_KEYS: [&str; 8] =
    ["name", "by", "do", HARNESS_KEY, "model", "writes", "escalate", "on"];
pub const ON_KEYS: [&str; 2] = ["pass", "reject"];
pub const ESCALATE_KEYS: [&str; 2] = ["to", "do"];
pub const LAND_KEYS: [&str; 2] = ["by", "do"];
pub const PATROL_KEYS: [&str; 6] = ["by", "do", "every", HARNESS_KEY, "model", "max_open"];

/// One value of a definition file's yaml, as it was parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Null,
    Bool(bool),
    Int(i64),
    /// A number that is not a whole one. Nothing a definition holds is one.
    Fraction,
    Text(String),
    List(Vec<Node>),
    /// The keys in the order they were written.
    Map(Vec<(String, Node)>),
}

/// Reads a value out of any format serde reads: a map key that is no text is
/// read as the text it was written as.
impl<'de> serde::Deserialize<'de> for Node {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Node, D::Error> {
        deserializer.deserialize_any(NodeVisitor)
    }
}

struct NodeVisitor;

impl<'de> serde::de::Visitor<'de> for NodeVisitor {
    type Value = Node;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a value")
    }

    fn visit_unit<E>(self) -> Result<Node, E> {
        Ok(Node::Null)
    }

    fn visit_none<E>(self) -> Result<Node, E> {
        Ok(Node::Null)
    }

    fn visit_some<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Node, D::Error> {
        deserializer.deserialize_any(NodeVisitor)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Node, E> {
        Ok(Node::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Node, E> {
        Ok(Node::Int(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Node, E> {
        Ok(i64::try_from(value).map_or(Node::Fraction, Node::Int))
    }

    fn visit_f64<E>(self, _: f64) -> Result<Node, E> {
        Ok(Node::Fraction)
    }

    fn visit_str<E>(self, value: &str) -> Result<Node, E> {
        Ok(Node::Text(value.to_string()))
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        let mut items: Vec<Node> = vec![];
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Node::List(items))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let mut entries: Vec<(String, Node)> = vec![];
        while let Some((key, value)) = map.next_entry::<Node, Node>()? {
            let key = match key {
                Node::Text(key) => key,
                Node::Int(key) => key.to_string(),
                Node::Bool(key) => key.to_string(),
                other => {
                    return Err(serde::de::Error::custom(format!("a key is {}", kind(&other))));
                }
            };
            entries.push((key, value));
        }
        Ok(Node::Map(entries))
    }
}

/// One definition file of a team's repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// Its path from the top of the repo.
    pub path: String,
    /// What its markdown says above its first yaml block, with no space
    /// around it: empty when it says nothing there, or has no such block.
    pub prose: String,
    /// Its yaml, or why it has none that can be read.
    pub yaml: Result<Node, String>,
}

/// What was read of a role's directory.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoleDir {
    /// The commit its repo was read at.
    pub commit: String,
    /// The entries of its skills directory.
    pub skills: Vec<String>,
    /// Each symlink in it that leaves it, as `<link> -> <target>`.
    pub links: Vec<String>,
    /// Each `CLAUDE.md` and `.claude/` a directory above it in its repo
    /// holds, as a path from the repo's root.
    pub above: Vec<String>,
    /// The harnesses it holds limits for (`Harness::limited`).
    pub limits: Vec<Harness>,
    /// Each file in it that a harness would load in place of its rules
    /// (`Harness::before_the_rules`), with that harness.
    pub in_place: Vec<(Harness, String)>,
}

/// Everything read of a team, and nothing yet checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    /// The team's directory: where a role named `./path` is, and what no
    /// role's directory may be or contain. `None` for files nothing says the
    /// repo of.
    pub at: Option<RepoDir>,
    pub team: File,
    /// Every entry of the workflows directory.
    pub workflows: Vec<File>,
    /// Every entry of the patrols directory.
    pub patrols: Vec<File>,
    /// Each directory `role_dirs` names, with what was read of it or why it
    /// was not read.
    pub roles: Vec<(RepoDir, Result<RoleDir, Unread>)>,
}

/// Why a role's directory was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unread {
    /// The read was answered, and the answer is no: there is no such repo,
    /// none a role can be read from, or no such directory at its head. A
    /// fault of the team.
    Refused(String),
    /// The read got no answer: nothing is known of the repo, and nothing is
    /// wrong with the team.
    Unreached(String),
}

/// One thing wrong with a team's definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    pub file: String,
    /// The key at fault, as a path from the top of the file's yaml
    /// (`stages[1].on.reject`, counting from 0); empty when it is the file itself.
    pub key: String,
    pub problem: String,
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.key.is_empty() {
            write!(f, "{}: {}", self.file, self.problem)
        } else {
            write!(f, "{}: {}: {}", self.file, self.key, self.problem)
        }
    }
}

/// A loaded team of `roles` roles. Whoever its workflows, its patrols and its
/// precedents have perform a skill is one of them.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(roles: int)]
pub struct Team {
    /// Swapped in for the flow label when the fleet hands an issue to a human.
    pub escalate_label: String,
    #[field(Vec<Role>[roles])]
    pub roles: Vec<Role>,
    #[field(Vec<Workflow{v: v.roles == roles}>)]
    pub workflows: Vec<Workflow>,
    #[field(Vec<Patrol[roles]>)]
    pub patrols: Vec<Patrol>,
    /// Who proposes a human's ruling as a precedent. `None`: the team names
    /// nobody, and none is proposed.
    #[field(Option<Precedents[roles]>)]
    pub precedents: Option<Precedents>,
}

impl Team {
    pub fn role(&self, name: &str) -> Option<&Role> {
        self.roles.iter().find(|role| role.name == name)
    }

    /// The role `work` is a skill of: one of the team's, with nothing to
    /// look for.
    #[spec(fn(&Team[@roles], &Work[roles]) -> &Role)]
    pub fn role_of(&self, work: &Work) -> &Role {
        &self.roles[work.role]
    }

    #[spec(fn(&Team[@roles], name: &str) -> Option<&Workflow{v: v.roles == roles}>)]
    pub fn workflow(&self, name: &str) -> Option<&Workflow> {
        self.workflows.iter().find(|workflow| workflow.name == name)
    }

    pub fn patrol(&self, name: &str) -> Option<&Patrol> {
        self.patrols.iter().find(|patrol| patrol.name == name)
    }

    /// The patrols `named` — a repo's `patrols` — are, in that order, or the
    /// names among them the team has no patrol for.
    pub fn patrols_named(&self, named: &[String]) -> Result<Vec<&Patrol>, Vec<String>> {
        let mut found: Vec<&Patrol> = vec![];
        let mut unknown: Vec<String> = vec![];
        for name in named {
            match self.patrol(name) {
                Some(patrol) => found.push(patrol),
                None => unknown.push(name.clone()),
            }
        }
        if unknown.is_empty() { Ok(found) } else { Err(unknown) }
    }

    /// Every harness a fleet could be asked for, in a repo whose default is
    /// `default` and which runs the patrols `runs`, each with what calls for
    /// it. The default comes first, called for by the repo (`BY_DEFAULT`):
    /// every consult, landing and proposal runs on it, and so does whatever
    /// names none. Any other caller is a stage, or a patrol that runs here,
    /// that names its harness.
    pub fn harnesses(&self, default: Harness, runs: &[String]) -> Vec<(Harness, Vec<String>)> {
        let mut called = vec![(default, vec![BY_DEFAULT.to_string()])];
        for workflow in &self.workflows {
            for stage in workflow.stages() {
                if let Some(harness) = stage.harness {
                    let by =
                        format!("the stage `{}` of the workflow `{}`", stage.name, workflow.name);
                    call(&mut called, harness, by);
                }
            }
        }
        for patrol in &self.patrols {
            if let Some(harness) = patrol.harness.filter(|_| runs.contains(&patrol.name)) {
                call(&mut called, harness, format!("the patrol `{}`", patrol.name));
            }
        }
        called
    }

    /// Holds the team to the harnesses its workers would run on, in a repo
    /// whose default is `default`, on a host where each harness that can say
    /// which models it knows `listed` them (`Harness::models`). A worker on
    /// a harness its role holds no limits for, while the role holds limits
    /// for another, is a fault: a harness is never changed into fewer limits
    /// unseen (docs/adr/0013). So is a model its harness does not list,
    /// where the stage or the patrol names that harness; one that takes the
    /// default is the repo's to answer for, and is handed back.
    pub fn held(&self, default: Harness, listed: &[(Harness, Vec<String>)]) -> Held {
        let mut held = Held::default();
        for run in self.runs() {
            let harness = run.named.unwrap_or(default);
            if let Some(problem) = run.unlimited(harness) {
                held.faults.push(Fault { file: run.file.clone(), key: run.harness_key, problem });
            }
            let Some(model) = run.model else { continue };
            let models =
                listed.iter().find(|(lists, _)| *lists == harness).map(|(_, models)| models);
            match (models, run.named) {
                (Some(models), _) if harness.knows(models, &model) => {}
                (Some(models), Some(_)) => {
                    let problem = format!(
                        "`{model}` is no model {} knows on this host: it lists {}. A model is \
passed as written, and is its harness's own name for it",
                        harness.name(),
                        some(models)
                    );
                    held.faults.push(Fault { file: run.file, key: run.model_key, problem });
                }
                (Some(_), None) => held.unknown.push((model, run.by)),
                (None, None) => {
                    let other = listed
                        .iter()
                        .find(|(lists, models)| *lists != harness && lists.names(models, &model));
                    if let Some((other, _)) = other {
                        held.elsewhere.push((model, run.by, *other));
                    }
                }
                (None, Some(_)) => {}
            }
        }
        held
    }

    /// Every worker the team would have started, whatever the issue.
    fn runs(&self) -> Vec<Run> {
        let mut runs = vec![];
        for workflow in &self.workflows {
            runs.extend(workflow_runs(self, workflow));
        }
        for patrol in &self.patrols {
            runs.push(Run {
                file: format!("{PATROLS}/{}.md", patrol.name),
                harness_key: HARNESS_KEY.to_string(),
                model_key: MODEL_KEY.to_string(),
                by: format!("the patrol `{}`", patrol.name),
                role: self.role_of(&patrol.work).clone(),
                named: patrol.harness,
                model: patrol.model.clone(),
            });
        }
        if let Some(precedents) = &self.precedents {
            runs.push(Run {
                file: TEAM_FILE.to_string(),
                harness_key: under(PRECEDENTS_KEY, "by"),
                model_key: String::new(),
                by: "the proposal of a precedent".to_string(),
                role: self.role_of(&precedents.work).clone(),
                named: None,
                model: None,
            });
        }
        runs
    }
}

/// What holding a team to its harnesses found (`Team::held`).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Held {
    /// What is wrong in the team's own files.
    pub faults: Vec<Fault>,
    /// Each model named with no harness that the default harness does not
    /// list, with what names it: the team is written for another default.
    pub unknown: Vec<(String, String)>,
    /// Each model named with no harness, under a default with no way to say
    /// which models it knows, that another harness lists under that very
    /// name: the model, what names it, and that harness.
    pub elsewhere: Vec<(String, String, Harness)>,
}

/// One worker a team would have started: what calls for it and where that is
/// written, its role, and the harness and the model it is given.
struct Run {
    file: String,
    /// The key its harness is, or would be, written at — or, of a worker
    /// that always runs on the default, the key that names its role.
    harness_key: String,
    model_key: String,
    /// What calls for it, in words.
    by: String,
    role: Role,
    /// The harness it names. `None`: it takes the default.
    named: Option<Harness>,
    model: Option<String>,
}

impl Run {
    /// Why its role is not right for `harness`, the one it runs on: the role
    /// holds limits, and none for that. A role with no limits at all is
    /// right for every harness, and of one that was not read nothing is
    /// known.
    fn unlimited(&self, harness: Harness) -> Option<String> {
        let role = &self.role;
        if role.limits.is_empty() || role.limits.contains(&harness) {
            return None;
        }
        let holds: Vec<String> = role
            .limits
            .iter()
            .map(|held| format!("`{}` (in its `{}`)", held.key(), held.limits()))
            .collect();
        let said = if self.named.is_some() { "" } else { ", the repo's default" };
        Some(format!(
            "{} runs on `{}`{said}, and its role `{}` holds limits for {} and none for `{}`: a \
worker there would be held to none of them. Write the role's limits for `{}` in its `{}`, in \
that harness's own form, or run this on a harness the role holds limits for",
            self.by,
            harness.key(),
            role.name,
            holds.join(" and "),
            harness.key(),
            harness.key(),
            harness.limits()
        ))
    }
}

/// How many of the models a harness lists are said before the rest are
/// counted.
const SAID: usize = 8;

/// `models`, as a fault says them: the first few, and how many more.
pub fn some(models: &[String]) -> String {
    let said: Vec<&str> = models.iter().take(SAID).map(String::as_str).collect();
    match models.iter().skip(SAID).count() {
        0 => said.join(", "),
        more => format!("{} and {more} more", said.join(", ")),
    }
}

/// The workers `workflow`, one of `team`'s, would have started: one for
/// each stage a role performs, one for each consult, and its landing.
#[spec(fn(&Team[@roles], &Workflow{v: v.roles == roles}) -> Vec<Run>)]
fn workflow_runs(team: &Team, workflow: &Workflow) -> Vec<Run> {
    let mut runs = vec![];
    let file = format!("{WORKFLOWS}/{}.md", workflow.name);
    for stage in workflow.stages() {
        let at = format!("stages[{}]", stage.at());
        let by = format!("the stage `{}` of the workflow `{}`", stage.name, workflow.name);
        if let Performer::Role(work) = &stage.by {
            runs.push(Run {
                file: file.clone(),
                harness_key: under(&at, HARNESS_KEY),
                model_key: under(&at, MODEL_KEY),
                by: by.clone(),
                role: team.role_of(work).clone(),
                named: stage.harness,
                model: stage.model.clone(),
            });
        }
        if let Some(consult) = &stage.consult {
            runs.push(Run {
                file: file.clone(),
                harness_key: under(&under(&at, "escalate"), "to"),
                model_key: String::new(),
                by: format!("the consult of {by}"),
                role: team.role_of(consult).clone(),
                named: None,
                model: None,
            });
        }
    }
    if let Some(land) = &workflow.land {
        runs.push(Run {
            file,
            harness_key: under(LAND, "by"),
            model_key: String::new(),
            by: format!("the landing of the workflow `{}`", workflow.name),
            role: team.role_of(land).clone(),
            named: None,
            model: None,
        });
    }
    runs
}

/// What calls for a repo's default harness, where nothing names one.
pub const BY_DEFAULT: &str = "the repo's default";

/// Adds `by` to what calls for `harness` in `called`.
fn call(called: &mut Vec<(Harness, Vec<String>)>, harness: Harness, by: String) {
    match called.iter_mut().find(|(named, _)| *named == harness) {
        Some((_, callers)) => callers.push(by),
        None => called.push((harness, vec![by])),
    }
}

/// One role of a team, as its directory stood when the team was loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Role {
    /// What the team's workflows call it.
    pub name: String,
    /// Its directory: a whole repo, or a subdirectory of one.
    pub dir: RepoDir,
    /// The head of the repo's default branch when it was read.
    pub commit: String,
    pub skills: Vec<String>,
    /// Each `CLAUDE.md` and `.claude/` above it in its repo
    /// (`RoleDir::above`): what a session opened by hand in it loads and a
    /// worker, given the directory alone, does not.
    pub above: Vec<String>,
    /// The harnesses it holds limits for: the ones it is right for, where it
    /// holds any (`Team::held`).
    pub limits: Vec<Harness>,
    /// Why its repo was not read, when the read got no answer
    /// (`Unread::Unreached`): it then has no commit, and every skill a
    /// workflow or a patrol names of it is taken as it is written. No worker
    /// can be started in it; what names it still loads.
    pub unreached: Option<String>,
}

/// One skill of one role of a team of `roles` roles: what a worker is started
/// to perform. Both exist: the role is kept as its place among the team's
/// roles, which is below `roles` (`Team::role_of`).
#[derive(Debug, PartialEq, Eq)]
#[refined_by(roles: int)]
pub struct Work {
    #[field(usize{v: v < roles})]
    role: usize,
    /// How many roles its team has.
    #[field(usize[roles])]
    of: usize,
    pub skill: String,
}

impl Clone for Work {
    #[spec(fn(&Work[@roles]) -> Work[roles])]
    fn clone(&self) -> Self {
        Work { role: self.role, of: self.of, skill: self.skill.clone() }
    }
}

/// One patrol: a skill of a role, run on a schedule and for no issue. Its
/// name is one a ref and a worktree can carry (`patrol_name`), and its
/// interval is longer than nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[refined_by(roles: int)]
pub struct Patrol {
    /// The name of its file: what a repo's `patrols` names it by.
    pub name: String,
    #[field(Work[roles])]
    pub work: Work,
    /// How long after one run the next is due, in seconds.
    pub every: u64,
    /// The harness it names. `None`: it names none, and takes the default.
    harness: Option<Harness>,
    /// Its harness's own name for the model, passed as written.
    pub model: Option<String>,
    /// How many issues it opened may be open before it is skipped.
    pub max_open: u64,
}

impl Patrol {
    /// The harness its worker runs in a repo whose default is `default`:
    /// the one it names, or that.
    pub fn harness(&self, default: Harness) -> Harness {
        self.harness.unwrap_or(default)
    }
}

/// Who proposes a human's ruling as a precedent — a skill of a role, started
/// once for the ruling — and how many proposals may wait for a human at once.
#[derive(Debug, Clone, PartialEq, Eq)]
#[refined_by(roles: int)]
pub struct Precedents {
    #[field(Work[roles])]
    pub work: Work,
    /// How many proposals may be open on the team's repo before no other is
    /// started.
    pub max_open: u64,
}

/// Who performs a stage: `by_role` when a role does, one of a team of `roles`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[refined_by(by_role: bool, roles: int)]
pub enum Performer {
    /// A decision waited for from a human. No worker is started.
    #[variant(Performer[false, 0])]
    Human,
    #[variant((Work[@roles]) -> Performer[true, roles])]
    Role(Work),
}

/// Where a `pass` leads: `to_stage` when to a stage, the one at the place `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[refined_by(to_stage: bool, to: int)]
pub enum Pass {
    /// The stage at this place among the workflow's stages.
    #[variant((usize[@to]) -> Pass[true, to])]
    Stage(usize),
    /// The landing train.
    #[variant(Pass[false, 0])]
    Land,
    /// Nothing more to do.
    #[variant(Pass[false, 0])]
    Done,
}

/// One stage of a workflow of `n` stages, of a team of `roles` roles. Its
/// place is below `n`, and so is the place of the stage its `pass` leads to,
/// when it leads to one. Its `reject`, if it may reject, leads to a stage
/// before it: every way back through a workflow is a counted rejection. The
/// role that performs it and the one it consults are roles of the team.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(at: int, n: int, roles: int)]
#[invariant(at < n)]
pub struct Stage {
    pub name: String,
    #[field(Performer{v: v.by_role => v.roles == roles})]
    pub by: Performer,
    /// The harness it names. `None`: it names none, and takes the default —
    /// or a human performs it, and it has none.
    harness: Option<Harness>,
    /// Its harness's own name for the model, passed as written.
    pub model: Option<String>,
    /// Whether it may commit. A stage that reviews does not.
    pub writes: bool,
    /// Who rules on its `escalate` verdict before a human is asked.
    #[field(Option<Work[roles]>)]
    pub consult: Option<Work>,
    #[field(usize[at])]
    at: usize,
    /// How many stages its workflow has.
    #[field(usize[n])]
    of: usize,
    /// How many roles its team has.
    #[field(usize[roles])]
    roles: usize,
    #[field(Pass{v: v.to_stage => v.to < n})]
    pass: Pass,
    #[field(Option<usize{v: v < at}>)]
    reject: Option<usize>,
}

impl Stage {
    /// The harness its worker runs in a repo whose default is `default`:
    /// the one it names, or that. `None` of a stage a human performs.
    /// Its consult's is the default whatever it names.
    pub fn harness(&self, default: Harness) -> Option<Harness> {
        match self.by {
            Performer::Human => None,
            Performer::Role(_) => Some(self.harness.unwrap_or(default)),
        }
    }

    /// Its place among its workflow's stages, the first being 0.
    #[spec(fn(&Stage[@s]) -> usize[s.at])]
    pub fn at(&self) -> usize {
        self.at
    }

    /// Where a `pass` leads: to a stage of its workflow, when to a stage.
    #[spec(fn(&Stage[@s]) -> Pass{v: v.to_stage => v.to < s.n})]
    pub fn pass(&self) -> Pass {
        self.pass
    }

    /// The place of the stage a `reject` leads to, a stage of its workflow
    /// before this one; `None` when this stage may not reject.
    #[spec(fn(&Stage[@s]) -> Option<usize{v: v < s.at && v < s.n}>)]
    pub fn reject(&self) -> Option<usize> {
        self.reject
    }
}

/// One workflow. It has a first stage; following `pass` from any of its stages
/// comes to the landing train or to nothing more to do; every stage can be
/// reached from the first; and `land` is set exactly when a `pass` leads there.
/// It has `n` stages, each a stage of a workflow of `n`: where one's `pass` or
/// `reject` leads, there is a stage. It is a workflow of a team of `roles`
/// roles, and so is each of its stages: whoever performs one, is consulted by
/// one or lands is a role of that team.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(n: int, roles: int)]
#[invariant(n > 0)]
pub struct Workflow {
    /// What follows `flow:` in its flow label.
    pub name: String,
    /// What it is for, as whoever hands the fleet work reads it: the prose of
    /// its file. Empty when the file says nothing.
    pub description: String,
    pub max_rework: u64,
    pub max_consecutive_rework: u64,
    #[field(Vec<Stage{v: v.n == n && v.roles == roles}>[n])]
    stages: Vec<Stage>,
    /// Who gates the PR and joins the landing train.
    #[field(Option<Work[roles]>)]
    pub land: Option<Work>,
    /// How many roles its team has.
    #[field(usize[roles])]
    roles: usize,
}

impl Workflow {
    pub fn flow_label(&self) -> String {
        format!("{FLOW_LABEL}{}", self.name)
    }

    /// The stage an issue entering the workflow is at. There is one, by the
    /// workflow's invariant.
    #[spec(fn(&Workflow[@w]) -> &Stage{v: v.n == w.n && v.roles == w.roles})]
    pub fn first(&self) -> &Stage {
        &self.stages[0]
    }

    /// The stage at a place the workflow has: where a `pass` or a `reject`
    /// of one of its stages leads.
    #[spec(fn(&Workflow[@w], at: usize{at < w.n}) -> &Stage{v: v.n == w.n && v.roles == w.roles})]
    pub fn stage(&self, at: usize) -> &Stage {
        &self.stages[at]
    }

    #[spec(fn(&Workflow[@w]) -> &[Stage{v: v.n == w.n && v.roles == w.roles}][w.n])]
    pub fn stages(&self) -> &[Stage] {
        &self.stages
    }

    /// The place among the workflow's stages of the one `name` names; `None`
    /// when the workflow has no such stage.
    #[spec(fn(&Workflow[@w], name: &str) -> Option<usize{v: v < w.n}>)]
    pub fn place_of(&self, name: &str) -> Option<usize> {
        let mut at = 0;
        while at < self.stages.len() {
            if self.stages[at].name == name {
                return Some(at);
            }
            at += 1;
        }
        None
    }
}

/// The one place a `Workflow` is built: one with no stage never gets out, nor
/// one whose stages were made for a workflow of another number of them.
#[spec(fn(String, u64, u64, Vec<Stage{v: v.n == n && v.roles == roles}>, n: usize, Option<Work[roles]>, roles: usize)
    -> Option<Workflow[n, roles]>)]
fn workflow(
    name: String,
    max_rework: u64,
    max_consecutive_rework: u64,
    stages: Vec<Stage>,
    n: usize,
    land: Option<Work>,
    roles: usize,
) -> Option<Workflow> {
    if n == 0 || stages.len() != n {
        return None;
    }
    Some(Workflow {
        name,
        description: String::new(),
        max_rework,
        max_consecutive_rework,
        stages,
        land,
        roles,
    })
}

/// `target` as where the stage at `at` may reject to: only a stage before it.
#[spec(fn(target: usize, at: usize) -> Option<usize{v: v < at}>)]
fn earlier(target: usize, at: usize) -> Option<usize> {
    if target < at { Some(target) } else { None }
}

/// `pass` as where a stage of a workflow of `n` stages may pass to: a stage
/// only when it is one of them.
#[spec(fn(Pass, n: usize) -> Option<Pass{v: v.to_stage => v.to < n}>)]
fn among(pass: Pass, n: usize) -> Option<Pass> {
    match pass {
        Pass::Stage(to) if to < n => Some(Pass::Stage(to)),
        Pass::Stage(_) => None,
        Pass::Land => Some(Pass::Land),
        Pass::Done => Some(Pass::Done),
    }
}

/// `work` as a skill of a role of a team of `roles` roles: `None` when it was
/// read under another number of them.
#[spec(fn(Work, roles: usize) -> Option<Work[roles]>)]
fn among_roles(work: Work, roles: usize) -> Option<Work> {
    if work.of == roles { Some(work) } else { None }
}

/// The directories the file of a team at `at` names as its roles, each
/// once: what to read before `load`. It reports nothing — `load` says what
/// is wrong with the file.
pub fn role_dirs(team: &File, at: Option<&RepoDir>) -> Vec<RepoDir> {
    let mut dirs: Vec<RepoDir> = vec![];
    let Ok(Node::Map(entries)) = &team.yaml else { return dirs };
    if let Some(Node::Map(roles)) = get(entries, "roles") {
        for (_, named) in roles {
            if let Node::Text(named) = named
                && let Ok(dir) = role_dir(named, at)
                && !dirs.contains(&dir)
            {
                dirs.push(dir);
            }
        }
    }
    dirs
}

/// The three ways a team's file names a role.
pub const ROLE_FORMS: &str = "owner/repo, owner/repo/path or ./path";

/// The directory `named` names as a role of a team at `at` — `owner/repo`,
/// `owner/repo/path`, or `./path`, a path from the root of the team's own
/// repo (docs/adr/0014) — or why it names none.
fn role_dir(named: &str, at: Option<&RepoDir>) -> Result<RepoDir, String> {
    let read = match named.strip_prefix("./").or(if named == "." { Some("") } else { None }) {
        Some(path) => match at {
            Some(at) => RepoDir::at(at.repo.clone(), path),
            None => {
                return Err(format!(
                    "`{named}` is a path in the team's repo, and nothing says which repo these \
files are of"
                ));
            }
        },
        None if named.starts_with('/') => Err(NotADir::Absolute),
        None => RepoDir::read(named),
    };
    read.map_err(|why| match why {
        NotADir::NoRepo => format!("must be a role's directory, as {ROLE_FORMS}"),
        NotADir::Absolute => format!(
            "`{named}` is an absolute path: a role is named as {ROLE_FORMS}, its path written \
from the root of its repo"
        ),
        NotADir::Leaves => format!(
            "`{named}` has a `..`: a role's path is written from the root of its repo, and \
names nothing outside it"
        ),
    })
}

/// Why a role at `dir` cannot stand beside the team at `at`: its directory
/// is the team's or contains it, or is among the team's own files. A role
/// under the team's directory is the usual layout, and no fault.
fn beside_team(dir: &RepoDir, at: &RepoDir) -> Option<String> {
    if dir == at {
        return Some(format!(
            "its directory is the team's own ({at}): a role is a directory of its own"
        ));
    }
    if dir.holds(at) {
        return Some(format!("its directory {dir} contains the team's ({at})"));
    }
    let own = [WORKFLOWS, PATROLS].into_iter().find(|own| at.join(own).holds(dir))?;
    Some(format!("its directory {dir} is inside the team's {own}/"))
}

/// Why a role at `dir` cannot stand beside the role `other` at `its`: one
/// contains the other. Two names for one directory are no fault.
fn beside_role(dir: &RepoDir, other: &str, its: &RepoDir) -> Option<String> {
    if dir == its {
        None
    } else if dir.holds(its) {
        Some(format!("its directory {dir} contains that of the role `{other}` ({its})"))
    } else if its.holds(dir) {
        Some(format!("its directory {dir} is inside that of the role `{other}` ({its})"))
    } else {
        None
    }
}

/// The name of the workflow the file at `path` defines; `None` when the path
/// is not `workflows/<name>.md`.
pub fn workflow_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix(WORKFLOWS)?.strip_prefix('/')?.strip_suffix(".md")?;
    if name.is_empty() || name.contains('/') { None } else { Some(name) }
}

/// The name of the patrol the file at `path` defines; `None` when the path is
/// not `patrols/<name>.md` with a name made of words of lowercase letters and
/// digits, each starting with a letter, joined by `-`. A ref and a worktree
/// are named for a patrol, and nothing else is let in as one.
pub fn patrol_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix(PATROLS)?.strip_prefix('/')?.strip_suffix(".md")?;
    let word = |word: &str| {
        word.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
            && word.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    };
    if name.len() <= 40 && name.split('-').all(word) { Some(name) } else { None }
}

/// The seconds a duration written as `text` lasts: a whole number from 1 up,
/// then `m`, `h` or `d` — `30m`, `6h`, `1d`. `None` for anything else.
pub fn duration(text: &str) -> Option<u64> {
    let units = [('m', 60), ('h', 3600), ('d', 86400)];
    let (count, unit) =
        units.iter().find_map(|(unit, lasts)| Some((text.strip_suffix(*unit)?, *lasts)))?;
    let count = crate::record::fleet_number(count)?;
    if count == 0 { None } else { count.checked_mul(unit) }
}

/// The team `sources` define, or everything wrong with them.
pub fn load(sources: &Sources) -> Result<Team, Vec<Fault>> {
    let mut faults: Vec<Fault> = vec![];
    let (escalate_label, roles) = team_file(&mut faults, sources);
    let precedents = precedents_key(&mut faults, sources, &roles);
    let mut workflows: Vec<Workflow> = vec![];
    for file in &sources.workflows {
        let mut report = Report { file: &file.path, faults: &mut faults };
        workflows.extend(workflow_file(&mut report, file, &roles));
    }
    let mut patrols: Vec<Patrol> = vec![];
    for file in &sources.patrols {
        let mut report = Report { file: &file.path, faults: &mut faults };
        patrols.extend(patrol_file(&mut report, file, &roles));
    }
    if faults.is_empty() {
        Ok(Team {
            escalate_label,
            roles: roles.loaded,
            workflows,
            patrols,
            precedents: precedents.flatten(),
        })
    } else {
        Err(faults)
    }
}

/// Where the faults of one file go.
struct Report<'a> {
    file: &'a str,
    faults: &'a mut Vec<Fault>,
}

impl Report<'_> {
    fn fault(&mut self, key: &str, problem: String) {
        self.faults.push(Fault { file: self.file.to_string(), key: key.to_string(), problem });
    }
}

/// The path of `key` under the key at `at`.
fn under(at: &str, key: &str) -> String {
    if at.is_empty() { key.to_string() } else { format!("{at}.{key}") }
}

fn kind(node: &Node) -> &'static str {
    match node {
        Node::Null => "nothing",
        Node::Bool(_) => "true or false",
        Node::Int(_) | Node::Fraction => "a number",
        Node::Text(_) => "text",
        Node::List(_) => "a list",
        Node::Map(_) => "a map",
    }
}

fn get<'n>(entries: &'n [(String, Node)], key: &str) -> Option<&'n Node> {
    entries.iter().find(|(name, _)| name == key).map(|(_, node)| node)
}

/// The entries of the map at `at`, every key not in `known` or written twice
/// reported. `None`, reported, when it is no map.
fn map<'n>(
    report: &mut Report,
    at: &str,
    node: &'n Node,
    known: &[&str],
) -> Option<&'n [(String, Node)]> {
    let Node::Map(entries) = node else {
        report.fault(at, format!("must be a map of keys, not {}", kind(node)));
        return None;
    };
    let mut seen: Vec<&str> = vec![];
    for (key, _) in entries {
        if !known.contains(&key.as_str()) {
            let known = known.join(", ");
            report.fault(&under(at, key), format!("unknown key; the keys here are: {known}"));
        } else if seen.contains(&key.as_str()) {
            report.fault(&under(at, key), "written twice".to_string());
        }
        seen.push(key);
    }
    Some(entries)
}

/// The text at `key`, when the map has the key. Anything but text that says
/// something is reported.
fn text<'n>(
    report: &mut Report,
    at: &str,
    entries: &'n [(String, Node)],
    key: &str,
) -> Option<&'n str> {
    match get(entries, key)? {
        Node::Text(text) if !text.trim().is_empty() => Some(text),
        Node::Text(_) => {
            report.fault(&under(at, key), "must not be empty".to_string());
            None
        }
        other => {
            report.fault(&under(at, key), format!("must be text, not {}", kind(other)));
            None
        }
    }
}

/// The harness the map at `at` names, when it has the key. A name that is
/// no harness is reported.
fn harness(report: &mut Report, at: &str, entries: &[(String, Node)]) -> Option<Harness> {
    let named = text(report, at, entries, HARNESS_KEY)?;
    let read = Harness::read(named);
    if read.is_none() {
        report.fault(&under(at, HARNESS_KEY), Harness::unknown(named));
    }
    read
}

/// The text at `key`, its absence reported too.
fn required<'n>(
    report: &mut Report,
    at: &str,
    entries: &'n [(String, Node)],
    key: &str,
) -> Option<&'n str> {
    if get(entries, key).is_none() {
        report.fault(&under(at, key), "required, and it is missing".to_string());
    }
    text(report, at, entries, key)
}

/// The count at `key`, or `default` where the map has no such key. `None`,
/// reported, when it is anything but a whole number from 0 up.
fn count(
    report: &mut Report,
    at: &str,
    entries: &[(String, Node)],
    key: &str,
    default: u64,
) -> Option<u64> {
    let Some(node) = get(entries, key) else { return Some(default) };
    let read = match node {
        Node::Int(n) => u64::try_from(*n).ok(),
        _ => None,
    };
    if read.is_none() {
        let was = match node {
            Node::Int(_) => "a number below 0",
            Node::Fraction => "a number with a fraction",
            other => kind(other),
        };
        report.fault(&under(at, key), format!("must be a whole number from 0 up, not {was}"));
    }
    read
}

/// The `max_open` of the map at `at`: a count, its absence reported too.
fn limit(report: &mut Report, at: &str, entries: &[(String, Node)]) -> Option<u64> {
    if get(entries, "max_open").is_none() {
        report.fault(&under(at, "max_open"), "required, and it is missing".to_string());
        return None;
    }
    count(report, at, entries, "max_open", 0)
}

/// The roles a team file names, `n` of them with their repo read.
#[refined_by(n: int)]
struct Roles<'n> {
    /// Whether the team file could be read at all. Where it could not, a role
    /// a workflow names is neither confirmed nor reported.
    known: bool,
    /// Every name under `roles`.
    names: Vec<&'n str>,
    /// The ones whose repo was read: the roles of the team, once it loads.
    #[field(Vec<Role>[n])]
    loaded: Vec<Role>,
}

impl Roles<'_> {
    /// The place among the roles whose repo was read of the one `name` names.
    #[spec(fn(&Roles[@n], name: &str) -> Option<usize{v: v < n}>)]
    fn place_of(&self, name: &str) -> Option<usize> {
        let mut at = 0;
        while at < self.loaded.len() {
            if self.loaded[at].name == name {
                return Some(at);
            }
            at += 1;
        }
        None
    }
}

/// Reads the team file → its escalate label and its roles.
fn team_file<'n>(faults: &mut Vec<Fault>, sources: &'n Sources) -> (String, Roles<'n>) {
    let mut report = Report { file: &sources.team.path, faults };
    let mut names: Vec<&str> = vec![];
    let mut loaded: Vec<Role> = vec![];
    // Every role whose directory stands apart from the team's and the others', read or not.
    let mut placed: Vec<(&str, RepoDir)> = vec![];
    let mut escalate_label = ESCALATE_LABEL.to_string();
    let node = match &sources.team.yaml {
        Ok(node) => node,
        Err(why) => {
            report.fault("", why.clone());
            return (escalate_label, Roles { known: false, names, loaded });
        }
    };
    let Some(entries) = map(&mut report, "", node, &TEAM_KEYS) else {
        return (escalate_label, Roles { known: false, names, loaded });
    };
    if let Some(label) = text(&mut report, "", entries, "escalate_label") {
        escalate_label = label.to_string();
    }
    let named = match get(entries, "roles") {
        None => return (escalate_label, Roles { known: true, names, loaded }),
        Some(Node::Map(named)) => named,
        Some(other) => {
            let problem =
                format!("must be a map of role names to directories, not {}", kind(other));
            report.fault("roles", problem);
            return (escalate_label, Roles { known: true, names, loaded });
        }
    };
    for (name, named) in named {
        let key = under("roles", name);
        if names.contains(&name.as_str()) {
            report.fault(&key, "written twice".to_string());
            continue;
        }
        names.push(name);
        if name == HUMAN {
            report
                .fault(&key, format!("a role cannot be named `{HUMAN}`: `by: {HUMAN}` is a human"));
            continue;
        }
        let dir = match named {
            Node::Text(named) => match role_dir(named, sources.at.as_ref()) {
                Ok(dir) => dir,
                Err(problem) => {
                    report.fault(&key, problem);
                    continue;
                }
            },
            other => {
                let problem =
                    format!("must be a role's directory, as {ROLE_FORMS}, not {}", kind(other));
                report.fault(&key, problem);
                continue;
            }
        };
        let mut apart = true;
        if let Some(problem) = sources.at.as_ref().and_then(|at| beside_team(&dir, at)) {
            report.fault(&key, problem);
            apart = false;
        }
        for (other, its) in &placed {
            if let Some(problem) = beside_role(&dir, other, its) {
                report.fault(&key, problem);
                apart = false;
            }
        }
        // One at fault is held against no other: its fault is said once.
        if !apart {
            continue;
        }
        placed.push((name.as_str(), dir.clone()));
        match sources.roles.iter().find(|(read, _)| *read == dir) {
            Some((_, Ok(read))) if !read.links.is_empty() => {
                let problem = format!(
                    "{dir} holds a symlink that leaves it: {}. A worker is given the role's \
directory alone, where such a link points at nothing",
                    read.links.join(", ")
                );
                report.fault(&key, problem)
            }
            Some((_, Ok(read))) if !read.in_place.is_empty() => {
                for (harness, file) in &read.in_place {
                    let problem = format!(
                        "{dir} holds `{file}`: {} loads that in place of `{RULES}`, the rules \
every harness works under. A role's rules are written once, in `{RULES}`; what one harness needs \
of its own is added to them, and never stands in their place",
                        harness.name()
                    );
                    report.fault(&key, problem)
                }
            }
            Some((_, Ok(read))) => loaded.push(Role {
                name: name.clone(),
                dir,
                commit: read.commit.clone(),
                skills: read.skills.clone(),
                above: read.above.clone(),
                limits: read.limits.clone(),
                unreached: None,
            }),
            Some((_, Err(Unread::Refused(why)))) => {
                report.fault(&key, format!("{dir} cannot be read: {why}"))
            }
            Some((_, Err(Unread::Unreached(why)))) => loaded.push(Role {
                name: name.clone(),
                dir,
                commit: String::new(),
                skills: vec![],
                above: vec![],
                limits: vec![],
                unreached: Some(why.clone()),
            }),
            None => report.fault(&key, format!("{dir} was not read")),
        }
    }
    (escalate_label, Roles { known: true, names, loaded })
}

/// Reads the team file's `precedents` → who proposes a ruling as one;
/// `Some(None)` where the file has no such key, `None` when the key is at
/// fault. What is wrong with the file itself is `team_file`'s to say.
#[spec(fn(&mut Vec<Fault>, &Sources, &Roles[@n]) -> Option<Option<Precedents[n]>>)]
fn precedents_key(
    faults: &mut Vec<Fault>,
    sources: &Sources,
    roles: &Roles,
) -> Option<Option<Precedents>> {
    let mut report = Report { file: &sources.team.path, faults };
    let Ok(Node::Map(entries)) = &sources.team.yaml else { return Some(None) };
    let Some(node) = get(entries, PRECEDENTS_KEY) else { return Some(None) };
    let entries = map(&mut report, PRECEDENTS_KEY, node, &PRECEDENTS_KEYS)?;
    let work = work(&mut report, roles, PRECEDENTS_KEY, entries, ("by", "do"));
    let max_open = limit(&mut report, PRECEDENTS_KEY, entries);
    Some(Some(Precedents { work: work?, max_open: max_open? }))
}

/// The skill `skill_key` names of the role `role_key` names, both under `at`.
/// `None` when either is missing or does not exist — reported, unless the
/// team file already says why the role cannot be had. The role is kept as its
/// place among the roles whose repo was read.
#[spec(fn(&mut Report, &Roles[@n], &str, &[(String, Node)], (&str, &str)) -> Option<Work[n]>)]
fn work(
    report: &mut Report,
    roles: &Roles,
    at: &str,
    entries: &[(String, Node)],
    (role_key, skill_key): (&str, &str),
) -> Option<Work> {
    let role = required(report, at, entries, role_key);
    let skill = required(report, at, entries, skill_key);
    let role = role?;
    if role == HUMAN {
        report.fault(&under(at, role_key), "must be a role: a human performs no skill".to_string());
        return None;
    }
    if !roles.known {
        return None;
    }
    if !roles.names.contains(&role) {
        let named = roles.names.join(", ");
        let problem = format!("`{role}` is not a role of the team; {TEAM_FILE} names: {named}");
        report.fault(&under(at, role_key), problem);
        return None;
    }
    let place = roles.place_of(role)?;
    let loaded = &roles.loaded[place];
    let skill = skill?;
    if loaded.unreached.is_none() && !loaded.skills.iter().any(|has| has == skill) {
        let problem = format!(
            "`{skill}` is not a skill of the role `{role}`: {} has no {SKILLS}/{skill} at {}",
            loaded.dir, loaded.commit
        );
        report.fault(&under(at, skill_key), problem);
        return None;
    }
    Some(Work { role: place, of: roles.loaded.len(), skill: skill.to_string() })
}

/// One stage as its file wrote it, its targets still names.
struct Draft<'n> {
    /// Its key: `stages[<place>]`.
    key: String,
    name: Option<&'n str>,
    by: Option<Performer>,
    harness: Option<Harness>,
    model: Option<String>,
    writes: Option<bool>,
    /// `None` when its `escalate` is at fault; `Some(None)` when it has none.
    consult: Option<Option<Work>>,
    pass: Option<&'n str>,
    reject: Option<&'n str>,
}

/// Reads one stage: everything that can be said of it alone.
fn draft<'n>(report: &mut Report, roles: &Roles, key: String, node: &'n Node) -> Option<Draft<'n>> {
    let entries = map(report, &key, node, &STAGE_KEYS)?;
    let name = required(report, &key, entries, "name");
    if let Some(name) = name
        && [LAND, DONE, HUMAN].contains(&name)
    {
        let problem =
            format!("a stage cannot be named `{name}`: the word means something in every workflow");
        report.fault(&under(&key, "name"), problem);
    }
    let human = matches!(get(entries, "by"), Some(Node::Text(by)) if by == HUMAN);
    let by = if human {
        for worker_key in ["do", HARNESS_KEY, "model", "escalate"] {
            if get(entries, worker_key).is_some() {
                let problem = format!(
                    "a stage `by: {HUMAN}` starts no worker, so it takes no `{worker_key}`"
                );
                report.fault(&under(&key, worker_key), problem);
            }
        }
        Some(Performer::Human)
    } else {
        work(report, roles, &key, entries, ("by", "do")).map(Performer::Role)
    };
    let harness = if human { None } else { harness(report, &key, entries) };
    let model = if human { None } else { text(report, &key, entries, "model") };
    let writes = match get(entries, "writes") {
        None => Some(true),
        Some(Node::Bool(writes)) => Some(*writes),
        Some(other) => {
            let problem = format!("must be true or false, not {}", kind(other));
            report.fault(&under(&key, "writes"), problem);
            None
        }
    };
    let consult = match get(entries, "escalate") {
        Some(node) if !human => {
            let at = under(&key, "escalate");
            let consult = match map(report, &at, node, &ESCALATE_KEYS) {
                Some(entries) => work(report, roles, &at, entries, ("to", "do")),
                None => None,
            };
            consult.map(Some)
        }
        _ => Some(None),
    };
    let (mut pass, mut reject) = (None, None);
    match get(entries, "on") {
        None => report.fault(&under(&key, "on"), "required, and it is missing".to_string()),
        Some(node) => {
            let at = under(&key, "on");
            if let Some(entries) = map(report, &at, node, &ON_KEYS) {
                pass = required(report, &at, entries, "pass");
                reject = text(report, &at, entries, "reject");
            }
        }
    }
    let model = model.map(str::to_string);
    Some(Draft { key, name, by, harness, model, writes, consult, pass, reject })
}

/// Where the verdicts of one stage lead, as far as its file could be read.
#[derive(Clone, Copy)]
struct Edges {
    /// `None` when the `pass` is at fault, and already reported.
    pass: Option<Pass>,
    reject: Option<usize>,
}

/// The place of the first stage named `name`.
fn place(drafts: &[Option<Draft>], name: &str) -> Option<usize> {
    drafts.iter().position(|draft| draft.as_ref().is_some_and(|draft| draft.name == Some(name)))
}

/// The places every stage can be reached from the first through.
fn reached(edges: &[Edges]) -> Vec<usize> {
    let mut found: Vec<usize> = vec![];
    let mut todo: Vec<usize> = vec![0];
    while let Some(at) = todo.pop() {
        if found.contains(&at) {
            continue;
        }
        found.push(at);
        if let Some(from) = edges.get(at) {
            if let Some(Pass::Stage(next)) = from.pass {
                todo.push(next);
            }
            todo.extend(from.reject);
        }
    }
    found
}

/// What following `pass` from one stage comes to.
enum Leads {
    /// The landing train, or nothing more to do.
    ToAnEnd,
    /// Back to a stage already passed through, for ever.
    Round,
    /// A `pass` that is itself at fault.
    Nowhere,
}

/// Follows `pass` from the stage at `from`. A stage has one `pass`, so a walk
/// longer than the stages there are has come round.
fn follow_pass(edges: &[Edges], from: usize) -> Leads {
    let mut at = from;
    for _ in edges {
        match edges.get(at).and_then(|edges| edges.pass) {
            Some(Pass::Stage(next)) => at = next,
            Some(Pass::Land | Pass::Done) => return Leads::ToAnEnd,
            None => return Leads::Nowhere,
        }
    }
    Leads::Round
}

/// Reads a workflow file's `land` → who lands; `Some(None)` where the file has
/// no such key, `None` when the key is at fault.
#[spec(fn(&mut Report, &Roles[@n], &[(String, Node)]) -> Option<Option<Work[n]>>)]
fn land_key(
    report: &mut Report,
    roles: &Roles,
    entries: &[(String, Node)],
) -> Option<Option<Work>> {
    let node = match get(entries, LAND) {
        Some(node) => node,
        None => return Some(None),
    };
    let entries = map(report, LAND, node, &LAND_KEYS)?;
    work(report, roles, LAND, entries, ("by", "do")).map(Some)
}

/// Reads one entry of the workflows directory → the workflow it defines, when
/// nothing is wrong with it.
#[spec(fn(&mut Report, &File, &Roles[@n]) -> Option<Workflow{v: v.roles == n}>)]
fn workflow_file(report: &mut Report, file: &File, roles: &Roles) -> Option<Workflow> {
    let Some(name) = workflow_name(&file.path) else {
        report.fault(
            "",
            format!("a workflow is a file {WORKFLOWS}/<name>.md, and nothing else belongs there"),
        );
        return None;
    };
    let node = match &file.yaml {
        Ok(node) => node,
        Err(why) => {
            report.fault("", why.clone());
            return None;
        }
    };
    let entries = map(report, "", node, &WORKFLOW_KEYS)?;
    let max_rework = count(report, "", entries, "max_rework", MAX_REWORK);
    let max_consecutive_rework =
        count(report, "", entries, "max_consecutive_rework", MAX_CONSECUTIVE_REWORK);
    let land = land_key(report, roles, entries);
    let listed = match get(entries, "stages") {
        Some(Node::List(listed)) if !listed.is_empty() => listed,
        Some(Node::List(_)) => {
            report.fault("stages", "a workflow needs at least one stage".to_string());
            return None;
        }
        Some(other) => {
            report.fault("stages", format!("must be a list of stages, not {}", kind(other)));
            return None;
        }
        None => {
            report.fault("stages", "required, and it is missing".to_string());
            return None;
        }
    };
    let mut drafts: Vec<Option<Draft>> = vec![];
    for (at, node) in listed.iter().enumerate() {
        drafts.push(draft(report, roles, format!("stages[{at}]"), node));
    }

    // What can only be said of the stages together.
    let mut edges: Vec<Edges> = vec![];
    // Whether every stage's name and targets were read: only then is a stage
    // that nothing leads to known to be one.
    let mut whole = true;
    for (at, draft) in drafts.iter().enumerate() {
        let Some(draft) = draft else {
            edges.push(Edges { pass: None, reject: None });
            whole = false;
            continue;
        };
        if let Some(name) = draft.name
            && let Some(first) = place(&drafts, name)
            && first < at
        {
            let problem = format!("`{name}` is already the name of stages[{first}]");
            report.fault(&under(&draft.key, "name"), problem);
            whole = false;
        }
        let pass = match draft.pass {
            None => None,
            Some(LAND) => Some(Pass::Land),
            Some(DONE) => Some(Pass::Done),
            Some(target) => {
                let to = place(&drafts, target).map(Pass::Stage);
                if to.is_none() {
                    let problem = format!(
                        "`{target}` is no stage of this workflow, nor `{LAND}` or `{DONE}`"
                    );
                    report.fault(&under(&draft.key, "on.pass"), problem);
                }
                to
            }
        };
        let reject = match draft.reject {
            None => None,
            Some(target) => match place(&drafts, target) {
                None => {
                    let problem = format!("`{target}` is no stage of this workflow");
                    report.fault(&under(&draft.key, "on.reject"), problem);
                    None
                }
                Some(to) if to >= at => {
                    let which = if to == at { "its own stage" } else { "a later stage" };
                    let problem =
                        format!("`{target}` is {which}: a reject leads only to an earlier stage");
                    report.fault(&under(&draft.key, "on.reject"), problem);
                    None
                }
                Some(to) => Some(to),
            },
        };
        whole = whole && pass.is_some() && reject.is_some() == draft.reject.is_some();
        edges.push(Edges { pass, reject });
    }
    let reached = reached(&edges);
    for (at, draft) in drafts.iter().enumerate() {
        let Some(draft) = draft else { continue };
        if whole && !reached.contains(&at) {
            report.fault(&draft.key, "no path from the first stage reaches this stage".to_string());
        }
        if let Leads::Round = follow_pass(&edges, at) {
            let problem = format!(
                "no path from this stage reaches `{LAND}` or `{DONE}`: following `pass` from it comes round for ever"
            );
            report.fault(&draft.key, problem);
        }
    }
    let lands = edges.iter().position(|edges| matches!(edges.pass, Some(Pass::Land)));
    let every_pass_read = edges.iter().all(|edges| edges.pass.is_some());
    match (lands, get(entries, LAND)) {
        (Some(at), None) => {
            let problem =
                format!("required: stages[{at}] has `pass: {LAND}`, and this says who lands");
            report.fault(LAND, problem);
        }
        (None, Some(_)) if every_pass_read => {
            report.fault(LAND, format!("no `pass` leads to `{LAND}`, so nobody lands"));
        }
        _ => {}
    }

    let roles = roles.loaded.len();
    let mut read = built(name, max_rework?, max_consecutive_rework?, drafts, edges, land?, roles)?;
    read.description = file.prose.clone();
    Some(read)
}

/// The workflow `name` of a team of `roles` roles that the drafts of its
/// stages make, each with where its verdicts lead: `None` when one of them
/// has something wrong in it, which was reported as it was read.
#[spec(fn(&str, u64, u64, Vec<Option<Draft>>, Vec<Edges>, Option<Work[roles]>, roles: usize)
    -> Option<Workflow{v: v.roles == roles}>)]
fn built(
    name: &str,
    max_rework: u64,
    max_consecutive_rework: u64,
    drafts: Vec<Option<Draft>>,
    edges: Vec<Edges>,
    land: Option<Work>,
    roles: usize,
) -> Option<Workflow> {
    let n = drafts.len();
    let mut stages: Vec<Stage> = vec![];
    for (at, (draft, edges)) in drafts.into_iter().zip(edges).enumerate() {
        let draft = draft?;
        let reject = match edges.reject {
            Some(to) => Some(earlier(to, at)?),
            None => None,
        };
        // A `reject` that led nowhere was reported above: it is not taken for
        // a stage that may not reject.
        if draft.reject.is_some() && reject.is_none() {
            return None;
        }
        stages.push(stage(draft, at, n, roles, among(edges.pass?, n)?, reject)?);
    }
    workflow(name.to_string(), max_rework, max_consecutive_rework, stages, n, land, roles)
}

/// Reads one entry of the patrols directory → the patrol it defines, when
/// nothing is wrong with it.
#[spec(fn(&mut Report, &File, &Roles[@n]) -> Option<Patrol[n]>)]
fn patrol_file(report: &mut Report, file: &File, roles: &Roles) -> Option<Patrol> {
    let Some(name) = patrol_name(&file.path) else {
        report.fault(
            "",
            format!(
                "a patrol is a file {PATROLS}/<name>.md, its name words of lowercase letters \
                 and digits, each starting with a letter, joined by `-`; nothing else belongs \
                 there"
            ),
        );
        return None;
    };
    let node = match &file.yaml {
        Ok(node) => node,
        Err(why) => {
            report.fault("", why.clone());
            return None;
        }
    };
    let entries = map(report, "", node, &PATROL_KEYS)?;
    let work = work(report, roles, "", entries, ("by", "do"));
    let every = match get(entries, "every") {
        None => {
            report.fault("every", "required, and it is missing".to_string());
            None
        }
        Some(node) => {
            let read = match node {
                Node::Text(text) => duration(text),
                _ => None,
            };
            if read.is_none() {
                let problem = "must be a duration: a whole number from 1 up, then m, h or d — \
                               30m, 6h, 1d";
                report.fault("every", problem.to_string());
            }
            read
        }
    };
    let harness = harness(report, "", entries);
    let model = text(report, "", entries, "model").map(str::to_string);
    let max_open = limit(report, "", entries);
    Some(Patrol {
        name: name.to_string(),
        work: work?,
        every: every?,
        harness,
        model,
        max_open: max_open?,
    })
}

/// The stage a draft with nothing wrong in it is, at the place `at` of a
/// workflow of `n` stages, of a team of `roles` roles: `None` when that is no
/// place of such a workflow, or the draft was read under other roles.
#[spec(fn(Draft, at: usize, n: usize, roles: usize, Pass{v: v.to_stage => v.to < n}, Option<usize{v: v < at}>)
    -> Option<Stage[at, n, roles]>)]
fn stage(
    draft: Draft,
    at: usize,
    n: usize,
    roles: usize,
    pass: Pass,
    reject: Option<usize>,
) -> Option<Stage> {
    if at >= n {
        return None;
    }
    let by = match draft.by? {
        Performer::Human => Performer::Human,
        Performer::Role(work) => Performer::Role(among_roles(work, roles)?),
    };
    let consult = match draft.consult? {
        Some(work) => Some(among_roles(work, roles)?),
        None => None,
    };
    Some(Stage {
        name: draft.name?.to_string(),
        by,
        harness: draft.harness,
        model: draft.model,
        writes: draft.writes?,
        consult,
        at,
        of: n,
        roles,
        pass,
        reject,
    })
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A workflow with no stage.
    #[should_fail]
    fn a_workflow_with_no_stage(name: String) -> Workflow {
        Workflow {
            name,
            description: String::new(),
            max_rework: 0,
            max_consecutive_rework: 0,
            stages: Vec::new(),
            land: None,
            roles: 0,
        }
    }

    /// Whatever stages were listed, taken as a workflow without a look.
    #[should_fail]
    fn a_workflow_of_whatever_was_listed(name: String, stages: Vec<Stage>) -> Workflow {
        Workflow {
            name,
            description: String::new(),
            max_rework: 0,
            max_consecutive_rework: 0,
            stages,
            land: None,
            roles: 0,
        }
    }

    /// A `pass` past the last stage of a workflow.
    #[should_fail]
    #[spec(fn(String, n: usize{n > 0}) -> Stage[0, n, 0])]
    fn a_pass_past_the_last_stage(name: String, n: usize) -> Stage {
        Stage {
            name,
            by: Performer::Human,
            harness: None,
            model: None,
            writes: false,
            consult: None,
            at: 0,
            of: n,
            roles: 0,
            pass: Pass::Stage(n),
            reject: None,
        }
    }

    /// A place read as at the number of stages: one past the last.
    #[should_fail]
    fn a_place_at_the_number_of_stages(workflow: &Workflow) -> &Stage {
        workflow.stage(workflow.stages.len())
    }

    /// A place read as whatever it is, past the stages or not.
    #[should_fail]
    fn a_place_past_the_stages(workflow: &Workflow, at: usize) -> &Stage {
        workflow.stage(at)
    }

    /// A role kept as at the number of roles: one past the last.
    #[should_fail]
    fn a_role_at_the_number_of_roles(skill: String, roles: usize) -> Work {
        Work { role: roles, of: roles, skill }
    }

    /// A role read for whatever skill it is, of a role of this team or past
    /// its roles.
    #[should_fail]
    fn a_role_read_past_the_teams_roles<'t>(team: &'t Team, work: &Work) -> &'t Role {
        team.role_of(work)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const TEAM: &str = "roles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n";
    pub(crate) const FEATURE: &str = "\
stages:
  - name: plan
    by: dev
    do: plan
    on: { pass: build }
  - name: build
    by: dev
    do: implement
    model: opus
    escalate: { to: reviewer, do: rule }
    on: { pass: review }
  - name: review
    by: reviewer
    do: review
    writes: false
    on: { pass: sign-off, reject: build }
  - name: sign-off
    by: human
    on: { pass: land, reject: plan }
land: { by: dev, do: land }
";

    fn file(path: &str, yaml: &str) -> File {
        File {
            path: path.to_string(),
            prose: String::new(),
            yaml: serde_yaml::from_str(yaml).map_err(|e| e.to_string()),
        }
    }

    fn dir(name: &str) -> RepoDir {
        RepoDir::read(name).unwrap()
    }

    fn repo(name: &str, skills: &[&str]) -> (RepoDir, Result<RoleDir, Unread>) {
        let skills = skills.iter().map(|skill| skill.to_string()).collect();
        (dir(name), Ok(RoleDir { commit: format!("{name}@1"), skills, ..RoleDir::default() }))
    }

    fn sources(team: &str, workflows: &[(&str, &str)]) -> Sources {
        Sources {
            at: Some(dir("acme/team")),
            team: file(TEAM_FILE, team),
            workflows: workflows.iter().map(|(path, yaml)| file(path, yaml)).collect(),
            patrols: vec![],
            roles: vec![
                repo("acme/dev", &["plan", "implement", "land"]),
                repo("acme/reviewer", &["review", "rule"]),
            ],
        }
    }

    /// The skill `skill` of the usual team's role at `role`: `dev` is its
    /// first, `reviewer` its second.
    fn work(role: usize, skill: &str) -> Work {
        Work { role, of: 2, skill: skill.to_string() }
    }
    const DEV: usize = 0;
    const REVIEWER: usize = 1;

    /// The usual team with `workflow` as its one workflow, `feature`.
    pub(crate) fn one(workflow: &str) -> Team {
        load(&sources(TEAM, &[("workflows/feature.md", workflow)])).unwrap()
    }

    /// Every fault of a team with one workflow, as (file, key, problem).
    fn faults(team: &str, workflow: &str) -> Vec<(String, String, String)> {
        let faults = load(&sources(team, &[("workflows/feature.md", workflow)])).unwrap_err();
        faults.into_iter().map(|fault| (fault.file, fault.key, fault.problem)).collect()
    }

    /// The one fault of `workflow` under the usual team: its key and problem.
    fn fault(workflow: &str) -> (String, String) {
        let mut faults = faults(TEAM, workflow);
        assert_eq!(faults.len(), 1, "{faults:#?}");
        let (file, key, problem) = faults.remove(0);
        assert_eq!(file, "workflows/feature.md");
        (key, problem)
    }

    /// A workflow of two stages by `dev`, with `second` as the rest of the second.
    fn two(first_on: &str, second: &str) -> String {
        format!(
            "stages:\n  - {{ name: a, by: dev, do: plan, on: {first_on} }}\n  - name: b\n{second}"
        )
    }

    /// The usual team with `patrols` as its patrol files, loaded.
    fn patrolled(patrols: &[(&str, &str)]) -> Result<Team, Vec<(String, String)>> {
        let mut sources = sources(TEAM, &[]);
        sources.patrols = patrols.iter().map(|(path, yaml)| file(path, yaml)).collect();
        load(&sources).map_err(|faults| {
            faults.into_iter().map(|fault| (fault.file, fault.key)).collect::<Vec<_>>()
        })
    }

    #[test]
    fn a_patrol_file_loads_as_it_was_written() {
        let sweep = "by: dev\ndo: plan\nevery: 6h\nmodel: opus\nmax_open: 3\n";
        let nightly = "by: reviewer\ndo: review\nevery: 1d\nmax_open: 0\n";
        let team =
            patrolled(&[("patrols/sweep.md", sweep), ("patrols/nightly-x2.md", nightly)]).unwrap();
        let work = work(DEV, "plan");
        assert_eq!(
            team.patrol("sweep"),
            Some(&Patrol {
                name: "sweep".to_string(),
                work,
                every: 6 * 3600,
                harness: None,
                model: Some("opus".to_string()),
                max_open: 3,
            })
        );
        assert_eq!(team.patrol("nightly-x2").map(|patrol| patrol.every), Some(86400));
        // A repo names the ones it runs, and none the team has not.
        let named = |names: &[&str]| {
            let names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
            team.patrols_named(&names)
                .map(|found| found.iter().map(|patrol| patrol.name.clone()).collect::<Vec<_>>())
        };
        assert_eq!(named(&["nightly-x2", "sweep"]), Ok(vec!["nightly-x2".into(), "sweep".into()]));
        assert_eq!(named(&[]), Ok(vec![]));
        assert_eq!(named(&["sweep", "audit", "mop"]), Err(vec!["audit".into(), "mop".into()]));
    }

    #[test]
    fn a_team_names_who_proposes_precedents_or_has_none_proposed() {
        let without = load(&sources(TEAM, &[])).unwrap();
        assert_eq!(without.precedents, None);
        let named = format!("{TEAM}precedents: {{ by: reviewer, do: rule, max_open: 4 }}\n");
        let with = load(&sources(&named, &[])).unwrap();
        let work = work(REVIEWER, "rule");
        assert_eq!(with.precedents, Some(Precedents { work, max_open: 4 }));

        let keys = |precedents: &str| -> Vec<String> {
            let team = format!("{TEAM}precedents: {precedents}\n");
            let faults = load(&sources(&team, &[])).unwrap_err();
            faults.into_iter().map(|fault| format!("{} {}", fault.file, fault.key)).collect()
        };
        assert_eq!(keys("reviewer"), ["team.md precedents"]);
        assert_eq!(
            keys("{ by: ops, do: mop, when: now }"),
            ["team.md precedents.when", "team.md precedents.by", "team.md precedents.max_open"]
        );
        assert_eq!(keys("{ by: reviewer, do: mop, max_open: -1 }").len(), 2);
        assert_eq!(keys("{ by: human, do: rule, max_open: 1 }"), ["team.md precedents.by"]);
        assert_eq!(keys("{ by: reviewer, max_open: 1 }"), ["team.md precedents.do"]);
    }

    #[test]
    fn a_duration_is_a_count_from_one_up_and_a_unit() {
        assert_eq!(duration("30m"), Some(1800));
        assert_eq!(duration("6h"), Some(21600));
        assert_eq!(duration("1d"), Some(86400));
        for not in ["", "h", "0h", "6", "6s", "1.5h", "-1h", "6 h", "6H", "99999999999999999999d"] {
            assert_eq!(duration(not), None, "{not:?}");
        }
    }

    #[test]
    fn a_patrols_name_is_one_a_ref_and_a_worktree_can_carry() {
        assert_eq!(patrol_name("patrols/sweep.md"), Some("sweep"));
        assert_eq!(patrol_name("patrols/dep-audit2.md"), Some("dep-audit2"));
        // A word that is a number would read as the suffix orca numbers a worktree with.
        for not in ["patrols/sweep-2.md", "patrols/Sweep.md", "patrols/.md", "patrols/a_b.md"] {
            assert_eq!(patrol_name(not), None, "{not}");
        }
        assert_eq!(patrol_name("patrols/sweep.txt"), None);
        assert_eq!(patrol_name(&format!("patrols/{}.md", "a".repeat(41))), None);
    }

    #[test]
    fn every_fault_of_a_patrol_file_is_reported() {
        let faults = patrolled(&[
            ("patrols/a.md", "by: ops\ndo: plan\nevery: 1h\nmax_open: 1\n"),
            ("patrols/b.md", "by: dev\ndo: mop\nevery: soon\n"),
            ("patrols/c.md", "by: dev\ndo: plan\nmax_open: many\nwhen: 1h\n"),
            ("patrols/README.md", "by: dev\n"),
        ])
        .unwrap_err();
        let fault = |file: &str, key: &str| (format!("patrols/{file}"), key.to_string());
        for wanted in [
            fault("a.md", "by"),
            fault("b.md", "do"),
            fault("b.md", "every"),
            fault("b.md", "max_open"),
            fault("c.md", "when"),
            fault("c.md", "every"),
            fault("c.md", "max_open"),
            fault("README.md", ""),
        ] {
            assert!(faults.contains(&wanted), "no fault {wanted:?} among {faults:#?}");
        }
    }

    #[test]
    fn a_well_formed_team_loads_as_it_was_written() {
        let team = load(&sources(TEAM, &[("workflows/feature.md", FEATURE)])).unwrap();
        assert_eq!(team.escalate_label, "ready-for-human");
        let dev = team.role("dev").unwrap();
        assert_eq!((dev.dir.repo.as_str(), dev.commit.as_str()), ("acme/dev", "acme/dev@1"));
        assert_eq!(dev.skills, ["plan", "implement", "land"]);
        let flow = team.workflow("feature").unwrap();
        assert_eq!(flow.flow_label(), "flow:feature");
        assert_eq!(flow.first().name, "plan");
        assert_eq!(flow.land, Some(work(DEV, "land")));
        let [plan, build, review, sign_off] = flow.stages() else { panic!("four stages") };
        assert_eq!((plan.at(), plan.pass(), plan.reject()), (0, Pass::Stage(1), None));
        assert_eq!(build.by, Performer::Role(work(DEV, "implement")));
        assert_eq!(build.model.as_deref(), Some("opus"));
        assert_eq!(build.consult, Some(work(REVIEWER, "rule")));
        let consulted = team.role_of(build.consult.as_ref().unwrap());
        assert_eq!(
            (consulted.name.as_str(), consulted.dir.repo.as_str()),
            ("reviewer", "acme/reviewer")
        );
        assert!(build.writes && !review.writes, "a stage writes unless it says not");
        assert_eq!((review.pass(), review.reject()), (Pass::Stage(3), Some(1)));
        assert_eq!(sign_off.by, Performer::Human);
        assert_eq!((sign_off.at(), sign_off.pass(), sign_off.reject()), (3, Pass::Land, Some(0)));
    }

    #[test]
    fn the_breakers_default_to_3_and_2() {
        let done = "stages:\n  - { name: a, by: dev, do: plan, on: { pass: done } }\n";
        let load_one = |yaml: &str| load(&sources(TEAM, &[("workflows/w.md", yaml)])).unwrap();
        let team = load_one(done);
        let flow = team.workflow("w").unwrap();
        assert_eq!((flow.max_rework, flow.max_consecutive_rework), (3, 2));
        assert_eq!(flow.land, None, "a workflow that ends in nothing more to do lands nothing");
        let team = load_one(&format!("max_rework: 0\nmax_consecutive_rework: 5\n{done}"));
        let flow = team.workflow("w").unwrap();
        assert_eq!((flow.max_rework, flow.max_consecutive_rework), (0, 5));
    }

    #[test]
    fn a_team_file_says_its_escalate_label_and_may_name_no_role() {
        let human = "stages:\n  - { name: a, by: human, on: { pass: done } }\n";
        let team = load(&sources("escalate_label: stuck\n", &[("workflows/w.md", human)])).unwrap();
        assert_eq!(team.escalate_label, "stuck");
        assert!(team.roles.is_empty());
        assert!(load(&sources("{}", &[])).unwrap().workflows.is_empty());
    }

    /// A workflow whose second stage, by `dev`, has `rest` as the rest of it,
    /// and whose `land` is `land`.
    fn landed(rest: &str, land: &str) -> String {
        let second = format!("    by: dev\n    do: plan\n{rest}    on: {{ pass: land }}\n");
        format!("{}land: {land}\n", two("{ pass: b }", &second))
    }

    #[test]
    fn a_stage_and_a_patrol_name_a_harness_or_take_the_default() {
        let team = one(&landed("    harness: claude-code\n", "{ by: dev, do: land }"));
        let workflow = team.workflow("feature").unwrap();
        let (unnamed, named) = (workflow.stage(0), &workflow.stages()[1]);
        assert_eq!(named.harness(Harness::DEFAULT), Some(Harness::ClaudeCode));
        assert_eq!(unnamed.harness(Harness::ClaudeCode), Some(Harness::ClaudeCode));
        // A human runs none.
        let team = one(FEATURE);
        let sign_off = &team.workflow("feature").unwrap().stages()[3];
        assert_eq!(sign_off.harness(Harness::DEFAULT), None);

        let named = "by: dev\ndo: plan\nevery: 6h\nharness: claude-code\nmax_open: 3\n";
        let unnamed = "by: dev\ndo: plan\nevery: 6h\nmax_open: 3\n";
        let team = patrolled(&[("patrols/a.md", named), ("patrols/b.md", unnamed)]).unwrap();
        for patrol in &team.patrols {
            assert_eq!(patrol.harness(Harness::DEFAULT), Harness::ClaudeCode, "{}", patrol.name);
        }
    }

    /// The usual team, `dev` and `reviewer` holding limits for these
    /// harnesses, with `workflow` as its one workflow, `patrol` as its one
    /// patrol, and `reviewer` proposing its precedents.
    fn limited(dev: &[Harness], reviewer: &[Harness], workflow: &str, patrol: &str) -> Team {
        let team = format!("{TEAM}precedents: {{ by: reviewer, do: rule, max_open: 4 }}\n");
        let mut sources = sources(&team, &[("workflows/feature.md", workflow)]);
        sources.patrols = vec![file("patrols/sweep.md", patrol)];
        for (role, limits) in [(DEV, dev), (REVIEWER, reviewer)] {
            sources.roles[role].1.as_mut().unwrap().limits = limits.to_vec();
        }
        load(&sources).unwrap()
    }

    /// Where each fault is, as "file: key".
    fn places(faults: &[Fault]) -> Vec<String> {
        faults.iter().map(|fault| format!("{}: {}", fault.file, fault.key)).collect()
    }

    #[test]
    fn a_worker_on_a_harness_its_role_holds_no_limits_for_while_it_holds_some_is_a_fault() {
        use Harness::{ClaudeCode, Pi};
        let rest = "    harness: pi\n    escalate: { to: reviewer, do: rule }\n";
        let workflow = landed(rest, "{ by: reviewer, do: rule }");
        let patrol = "by: dev\ndo: plan\nevery: 6h\nharness: pi\nmax_open: 3\n";
        let held = |dev: &[Harness], reviewer: &[Harness], default: Harness| {
            let held = limited(dev, reviewer, &workflow, patrol).held(default, &[]);
            assert_eq!((&held.unknown, &held.elsewhere), (&vec![], &vec![]));
            held.faults
        };
        // A role with no limits at all is right for every harness.
        for default in Harness::ALL {
            assert_eq!(held(&[], &[], default), []);
            assert_eq!(held(&[ClaudeCode, Pi], &[Pi, ClaudeCode], default), []);
        }
        // One with limits for one harness is right for that one alone: the
        // stage and the patrol that name another are refused.
        let faults = held(&[ClaudeCode], &[], ClaudeCode);
        assert_eq!(
            places(&faults),
            ["workflows/feature.md: stages[1].harness", "patrols/sweep.md: harness"]
        );
        assert!(
            faults[0].problem.starts_with(
                "the stage `b` of the workflow `feature` runs on `pi`, and its role `dev` holds \
                 limits for `claude-code` (in its `.claude/settings.json`) and none for `pi`: "
            ),
            "{}",
            faults[0].problem
        );
        assert!(faults[0].problem.contains("in its `.pi/extensions`"), "{}", faults[0].problem);
        assert!(faults[1].problem.starts_with("the patrol `sweep` runs on `pi`, and its role"));
        // So is whatever takes a default the role holds none for.
        let faults = held(&[ClaudeCode], &[], Pi);
        assert_eq!(places(&faults)[0], "workflows/feature.md: stages[0].harness");
        assert!(
            faults[0].problem.starts_with(
                "the stage `a` of the workflow `feature` runs on `pi`, the repo's default, and"
            ),
            "{}",
            faults[0].problem
        );
        assert_eq!(faults.len(), 3);
        // A consult, a landing and a proposal run on the default, whatever
        // the stage names.
        assert_eq!(held(&[ClaudeCode, Pi], &[ClaudeCode], ClaudeCode), []);
        let faults = held(&[ClaudeCode, Pi], &[ClaudeCode], Pi);
        assert_eq!(
            places(&faults),
            [
                "workflows/feature.md: stages[1].escalate.to",
                "workflows/feature.md: land.by",
                "team.md: precedents.by"
            ]
        );
        assert!(faults[0].problem.starts_with(
            "the consult of the stage `b` of the workflow `feature` runs on `pi`, the repo's \
             default, and its role `reviewer` holds"
        ));
        assert!(faults[1].problem.starts_with("the landing of the workflow `feature` runs on"));
        assert!(faults[2].problem.starts_with("the proposal of a precedent runs on `pi`"));
    }

    #[test]
    fn a_model_its_harness_does_not_list_is_a_fault_where_it_names_the_harness() {
        use Harness::{ClaudeCode, Pi};
        let workflow = landed("    harness: pi\n    model: opus\n", "{ by: reviewer, do: rule }");
        let patrol =
            |model: &str| format!("by: dev\ndo: plan\nevery: 6h\nmodel: {model}\nmax_open: 3\n");
        let team = |model: &str| limited(&[], &[], &workflow, &patrol(model));
        let lists = vec!["openai-codex/gpt-5.5".to_string(), "zai/glm-5.2".to_string()];
        let listed = [(Pi, lists)];
        // Under a default with no way to say, a model named with no harness
        // is passed as written — and said, where it is another harness's.
        let held = team("zai/glm-5.2").held(ClaudeCode, &listed);
        assert_eq!(places(&held.faults), ["workflows/feature.md: stages[1].model"]);
        assert_eq!(
            held.faults[0].problem,
            "`opus` is no model pi knows on this host: it lists openai-codex/gpt-5.5, \
             zai/glm-5.2. A model is passed as written, and is its harness's own name for it"
        );
        let by = "the patrol `sweep`".to_string();
        assert_eq!(held.elsewhere, [("zai/glm-5.2".to_string(), by.clone(), Pi)]);
        assert_eq!(held.unknown, []);
        let held = team("opus").held(ClaudeCode, &listed);
        assert_eq!((held.faults.len(), held.elsewhere, held.unknown), (1, vec![], vec![]));
        // Under a default that lists its models, one it knows is nothing, and
        // one it does not is the repo's to answer for.
        let held = team("glm").held(Pi, &listed);
        assert_eq!((held.faults.len(), held.elsewhere, held.unknown), (1, vec![], vec![]));
        let held = team("opus").held(Pi, &listed);
        assert_eq!(places(&held.faults), ["workflows/feature.md: stages[1].model"]);
        assert_eq!(held.unknown, [("opus".to_string(), by)]);
        // A harness that was not asked is taken to know what it is given.
        assert_eq!(team("opus").held(Pi, &[]), Held::default());

        let many: Vec<String> = (0..10).map(|n| format!("p/m{n}")).collect();
        assert_eq!(some(&many), "p/m0, p/m1, p/m2, p/m3, p/m4, p/m5, p/m6, p/m7 and 2 more");
    }

    #[test]
    fn a_file_a_harness_would_load_in_place_of_a_roles_rules_is_a_fault_naming_it() {
        let mut sources = sources(TEAM, &[]);
        let read = sources.roles[DEV].1.as_mut().unwrap();
        read.in_place = vec![
            (Harness::Pi, "AGENTS.md".to_string()),
            (Harness::Pi, "AGENTS.override.md".to_string()),
        ];
        let faults = load(&sources).unwrap_err();
        assert_eq!(places(&faults), ["team.md: roles.dev", "team.md: roles.dev"]);
        assert!(
            faults[0].problem.starts_with(
                "acme/dev holds `AGENTS.md`: pi loads that in place of `CLAUDE.md`, the rules \
                 every harness works under."
            ),
            "{}",
            faults[0].problem
        );
        assert!(faults[1].problem.starts_with("acme/dev holds `AGENTS.override.md`: "));
    }

    #[test]
    fn a_fleet_could_be_asked_for_the_default_and_for_each_harness_named() {
        let runs = |names: &[&str]| names.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        // Nothing names one: the default alone, called for by the repo.
        let by_default = vec![(Harness::ClaudeCode, vec![BY_DEFAULT.to_string()])];
        assert_eq!(one(FEATURE).harnesses(Harness::DEFAULT, &[]), by_default);

        let named = "by: dev\ndo: plan\nevery: 6h\nharness: claude-code\nmax_open: 3\n";
        let unnamed = "by: dev\ndo: plan\nevery: 6h\nmax_open: 3\n";
        let workflow = landed("    harness: claude-code\n", "{ by: dev, do: land }");
        let mut sources = sources(TEAM, &[("workflows/feature.md", &workflow)]);
        sources.patrols =
            [("patrols/a.md", named), ("patrols/b.md", unnamed), ("patrols/c.md", named)]
                .iter()
                .map(|(path, yaml)| file(path, yaml))
                .collect();
        let team = load(&sources).unwrap();
        // A stage that names one calls for it, and so does a patrol that
        // names one and runs here: `c` does not run, and `b` names none.
        let called = team.harnesses(Harness::DEFAULT, &runs(&["b", "a"]));
        assert_eq!(
            called,
            [(
                Harness::ClaudeCode,
                vec![
                    BY_DEFAULT.to_string(),
                    "the stage `b` of the workflow `feature`".to_string(),
                    "the patrol `a`".to_string(),
                ]
            )]
        );
    }

    #[test]
    fn a_harness_is_named_only_where_a_model_is_and_only_by_its_key() {
        let usual = "{ by: dev, do: land }";
        for none in ["codex", "Claude Code", "claude"] {
            let (key, problem) = fault(&landed(&format!("    harness: {none}\n"), usual));
            assert_eq!(key, "stages[1].harness");
            assert!(problem.contains(none) && problem.contains("claude-code"), "{problem}");
        }
        assert_eq!(fault(&landed("    harness: [claude-code]\n", usual)).0, "stages[1].harness");
        // On a consult and on `land:` it is no key.
        let consulted = "    escalate: { to: reviewer, do: rule, harness: claude-code }\n";
        assert_eq!(fault(&landed(consulted, usual)).0, "stages[1].escalate.harness");
        let land = "{ by: dev, do: land, harness: claude-code }";
        assert_eq!(fault(&landed("", land)).0, "land.harness");
        // Nor of a workflow, nor on `precedents:`.
        assert_eq!(fault(&format!("harness: claude-code\n{FEATURE}")).0, "harness");
        let precedents = "precedents: { by: dev, do: plan, max_open: 1, harness: claude-code }\n";
        let faults = faults(&format!("{TEAM}{precedents}"), FEATURE);
        assert_eq!(faults.len(), 1, "{faults:#?}");
        assert_eq!((faults[0].0.as_str(), faults[0].1.as_str()), ("team.md", "precedents.harness"));
        // A stage a human performs starts no worker.
        let human =
            two("{ pass: b }", "    by: human\n    harness: claude-code\n    on: { pass: done }\n");
        assert_eq!(fault(&human).0, "stages[1].harness");
        let patrol = "by: dev\ndo: plan\nevery: 6h\nharness: codex\nmax_open: 3\n";
        let faults = patrolled(&[("patrols/a.md", patrol)]).unwrap_err();
        assert_eq!(faults, [("patrols/a.md".to_string(), "harness".to_string())]);
    }

    #[test]
    fn an_unknown_key_is_reported_wherever_it_is() {
        let (key, problem) = fault(&format!("retries: 2\n{FEATURE}"));
        assert_eq!(key, "retries");
        assert!(problem.contains("unknown key") && problem.contains("max_rework"), "{problem}");
        let (key, _) = fault(&two(
            "{ pass: b }",
            "    by: dev\n    do: plan\n    on: { pass: done }\n    skill: x\n",
        ));
        assert_eq!(key, "stages[1].skill");
        let (key, _) = fault(&two(
            "{ pass: b, fail: a }",
            "    by: dev\n    do: plan\n    on: { pass: done }\n",
        ));
        assert_eq!(key, "stages[0].on.fail");
        let (key, _) = fault(&two(
            "{ pass: b }",
            "    by: dev\n    do: plan\n    escalate: { to: reviewer, do: rule, then: a }\n    on: { pass: done }\n",
        ));
        assert_eq!(key, "stages[1].escalate.then");
        let faults = faults(&format!("{TEAM}mascots: {{}}\n"), FEATURE);
        assert_eq!(faults.len(), 1, "{faults:#?}");
        assert_eq!((faults[0].0.as_str(), faults[0].1.as_str()), ("team.md", "mascots"));
    }

    #[test]
    fn a_wrong_type_is_reported_with_what_was_found() {
        for (yaml, key, found) in [
            ("max_rework: many", "max_rework", "text"),
            ("max_rework: -1", "max_rework", "below 0"),
            ("max_consecutive_rework: 1.5", "max_consecutive_rework", "fraction"),
        ] {
            let (at, problem) = fault(&format!("{yaml}\n{FEATURE}"));
            assert_eq!(at, key);
            assert!(problem.contains(found), "{problem}");
        }
        let rest =
            |line: &str| format!("    by: dev\n    do: plan\n    on: {{ pass: done }}\n{line}");
        for (line, key) in [
            ("    writes: no thanks\n", "stages[1].writes"),
            ("    model: [opus]\n", "stages[1].model"),
            ("    model:\n", "stages[1].model"),
            ("    escalate: reviewer\n", "stages[1].escalate"),
        ] {
            assert_eq!(fault(&two("{ pass: b }", &rest(line))).0, key);
        }
        assert_eq!(
            fault(&two("b", "    by: dev\n    do: plan\n    on: { pass: done }\n")).0,
            "stages[0].on"
        );
        assert_eq!(fault("stages: plan").0, "stages");
        assert_eq!(fault("stages: [plan]").0, "stages[0]");
        assert_eq!(fault("[a, b]").0, "");
        assert_eq!(fault(&FEATURE.replace("land: { by: dev, do: land }", "land: dev")).0, "land");
        let faults =
            faults("roles: [dev]", "stages:\n  - { name: a, by: human, on: { pass: done } }");
        assert_eq!((faults[0].0.as_str(), faults[0].1.as_str()), ("team.md", "roles"));
    }

    #[test]
    fn stage_names_are_unique_and_never_a_word_every_workflow_uses() {
        let (key, problem) = fault(
            &two("{ pass: done }", "    by: dev\n    do: plan\n    on: { pass: done }\n")
                .replace("name: b", "name: a"),
        );
        assert_eq!(key, "stages[1].name");
        assert!(problem.contains("already the name of stages[0]"), "{problem}");
        for word in ["land", "done", "human"] {
            let yaml = format!(
                "stages:\n  - {{ name: {word}, by: dev, do: plan, on: {{ pass: done }} }}\n"
            );
            let (key, problem) = fault(&yaml);
            assert_eq!(key, "stages[0].name");
            assert!(problem.contains(word), "{problem}");
        }
    }

    #[test]
    fn a_target_must_be_a_stage_and_a_reject_must_lead_back() {
        let second = "    by: dev\n    do: plan\n    on: { pass: done }\n";
        let (key, problem) = fault(&two("{ pass: c }", second));
        // Nothing leads to `b` once the first stage's pass is at fault, and that
        // is not reported on top of it: where the pass was meant to lead is unknown.
        assert_eq!(key, "stages[0].on.pass");
        assert!(problem.contains("`c` is no stage"), "{problem}");
        let rejects = |to: &str| {
            format!("    by: dev\n    do: plan\n    on: {{ pass: done, reject: {to} }}\n")
        };
        let (key, problem) = fault(&two("{ pass: b }", &rejects("nowhere")));
        assert_eq!(key, "stages[1].on.reject");
        assert!(problem.contains("no stage"), "{problem}");
        let (key, problem) = fault(&two("{ pass: b }", &rejects("b")));
        assert_eq!(key, "stages[1].on.reject");
        assert!(problem.contains("its own stage"), "{problem}");
        let (key, problem) = fault(&two("{ pass: b, reject: b }", second));
        assert_eq!(key, "stages[0].on.reject");
        assert!(problem.contains("a later stage"), "{problem}");
        // `land` and `done` are where a pass may lead, never a reject.
        assert_eq!(fault(&two("{ pass: b }", &rejects("done"))).0, "stages[1].on.reject");
        let back = two("{ pass: b }", &rejects("a"));
        assert!(load(&sources(TEAM, &[("workflows/w.md", &back)])).is_ok(), "leads back");
    }

    #[test]
    fn every_stage_is_reached_and_reaches_an_end() {
        let second = "    by: dev\n    do: plan\n    on: { pass: done }\n";
        let (key, problem) = fault(&two("{ pass: done }", second));
        assert_eq!(key, "stages[1]");
        assert!(problem.contains("no path from the first stage"), "{problem}");
        // A stage only a reject leads to is reached.
        let back = "\
stages:
  - { name: a, by: dev, do: plan, on: { pass: c } }
  - { name: b, by: dev, do: plan, on: { pass: c } }
  - { name: c, by: dev, do: plan, on: { pass: done, reject: b } }
";
        assert!(load(&sources(TEAM, &[("workflows/w.md", back)])).is_ok());

        let round = "\
stages:
  - { name: a, by: dev, do: plan, on: { pass: b } }
  - { name: b, by: dev, do: plan, on: { pass: c } }
  - { name: c, by: dev, do: plan, on: { pass: b, reject: a } }
";
        let found = faults(TEAM, round);
        let keys: Vec<&str> = found.iter().map(|(_, key, _)| key.as_str()).collect();
        assert_eq!(keys, ["stages[0]", "stages[1]", "stages[2]"], "{found:#?}");
        assert!(found.iter().all(|(_, _, problem)| problem.contains("reaches `land` or `done`")));
    }

    #[test]
    fn land_is_set_exactly_when_a_pass_leads_there() {
        let lands = "stages:\n  - { name: a, by: dev, do: plan, on: { pass: land } }\n";
        let (key, problem) = fault(lands);
        assert_eq!(key, "land");
        assert!(problem.contains("stages[0] has `pass: land`"), "{problem}");
        let ends = "stages:\n  - { name: a, by: dev, do: plan, on: { pass: done } }\nland: { by: dev, do: land }\n";
        let (key, problem) = fault(ends);
        assert_eq!(key, "land");
        assert!(problem.contains("no `pass` leads to `land`"), "{problem}");
        assert_eq!(fault(&format!("{lands}land: {{ by: human, do: land }}\n")).0, "land.by");
        assert_eq!(fault(&format!("{lands}land: {{ by: dev }}\n")).0, "land.do");
    }

    #[test]
    fn a_role_and_its_skill_must_exist() {
        let stage =
            |rest: &str| format!("stages:\n  - {{ name: a, {rest}, on: {{ pass: done }} }}\n");
        let (key, problem) = fault(&stage("by: tester, do: test"));
        assert_eq!(key, "stages[0].by");
        assert!(
            problem.contains("`tester` is not a role") && problem.contains("dev, reviewer"),
            "{problem}"
        );
        let (key, problem) = fault(&stage("by: reviewer, do: implement"));
        assert_eq!(key, "stages[0].do");
        assert!(
            problem.contains("acme/reviewer has no .claude/skills/implement at acme/reviewer@1"),
            "{problem}"
        );
        let (key, problem) = fault(&stage("by: dev"));
        assert_eq!(key, "stages[0].do");
        assert!(problem.contains("required"), "{problem}");
        assert_eq!(fault(&stage("do: plan")).0, "stages[0].by");
        assert_eq!(
            fault(&stage("by: dev, do: plan, escalate: { to: boss, do: rule }")).0,
            "stages[0].escalate.to"
        );
        assert_eq!(
            fault(&stage("by: dev, do: plan, escalate: { to: reviewer, do: plan }")).0,
            "stages[0].escalate.do"
        );
        assert_eq!(
            fault(&stage("by: dev, do: plan, escalate: { to: human, do: rule }")).0,
            "stages[0].escalate.to"
        );
        // A human is started as no worker.
        assert_eq!(fault(&stage("by: human, do: plan")).0, "stages[0].do");
        assert_eq!(fault(&stage("by: human, model: opus")).0, "stages[0].model");
    }

    #[test]
    fn a_role_whose_repo_cannot_be_had_is_reported_once_in_the_team_file() {
        let workflow = "stages:\n  - { name: a, by: ops, do: deploy, on: { pass: done } }\n";
        let unread = |team: &str, problem: &str| {
            let mut found = faults(team, workflow);
            assert_eq!(found.len(), 1, "{found:#?}");
            let (file, key, said) = found.remove(0);
            assert_eq!((file.as_str(), key.as_str()), ("team.md", "roles.ops"));
            assert!(said.contains(problem), "{said}");
        };
        unread("roles: { ops: acme/ops }", "acme/ops was not read");
        unread("roles: { ops: ops }", "owner/repo");
        unread("roles: { ops: 7 }", "not a number");
        let mut sources = sources("roles: { ops: acme/ops }", &[("workflows/w.md", workflow)]);
        let refused = Unread::Refused("no such repository".to_string());
        sources.roles.push((dir("acme/ops"), Err(refused)));
        let found = load(&sources).unwrap_err();
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(
            found[0].to_string(),
            "team.md: roles.ops: acme/ops cannot be read: no such repository"
        );
        // a read that got no answer says nothing of the team: the role is
        // loaded with nothing known of it, and what names it loads too
        let unreached = Unread::Unreached("gh api failed".to_string());
        *sources.roles.last_mut().unwrap() = (dir("acme/ops"), Err(unreached));
        sources.workflows.push(file(
            "workflows/ops.md",
            "stages:\n  - { name: a, by: ops, do: anything, on: { pass: done } }",
        ));
        let team = load(&sources).unwrap();
        let ops = team.roles.iter().find(|role| role.name == "ops").unwrap();
        assert_eq!(ops.unreached.as_deref(), Some("gh api failed"));
        assert_eq!((ops.commit.as_str(), ops.skills.len()), ("", 0));
        assert_eq!(team.workflows.len(), 2);
        let human = faults(
            "roles: { human: acme/dev }",
            "stages:\n  - { name: a, by: human, on: { pass: done } }",
        );
        assert_eq!(human[0].1, "roles.human");
    }

    #[test]
    fn every_fault_of_a_team_is_reported_in_one_run() {
        let one = "\
max_rework: lots
stages:
  - { name: a, by: dev, do: plan, on: { pass: b }, colour: red }
  - { name: a, by: tester, do: test, on: { pass: land, reject: z } }
  - { name: c, by: reviewer, do: implement, on: { pass: done } }
";
        let two = "stages: []\n";
        let mut sources = sources(
            "roles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n  ops: nowhere\nextra: 1\n",
            &[("workflows/one.md", one), ("workflows/two.md", two), ("workflows/notes.txt", "x")],
        );
        sources.workflows.push(File {
            path: "workflows/bad.md".into(),
            prose: String::new(),
            yaml: Err("has no ```yaml block".into()),
        });
        let found = load(&sources).unwrap_err();
        let found: Vec<String> =
            found.iter().map(|fault| format!("{}: {}", fault.file, fault.key)).collect();
        assert_eq!(
            found,
            [
                "team.md: extra",
                "team.md: roles.ops",
                "workflows/one.md: max_rework",
                "workflows/one.md: stages[0].colour",
                "workflows/one.md: stages[1].by",
                "workflows/one.md: stages[2].do",
                "workflows/one.md: stages[0].on.pass",
                "workflows/one.md: stages[1].name",
                "workflows/one.md: stages[1].on.reject",
                "workflows/one.md: land",
                "workflows/two.md: stages",
                "workflows/notes.txt: ",
                "workflows/bad.md: ",
            ]
        );
    }

    #[test]
    fn a_key_written_twice_or_a_file_that_is_no_yaml_is_a_fault() {
        let (key, problem) = fault(&format!("max_rework: 1\nmax_rework: 2\n{FEATURE}"));
        assert_eq!((key.as_str(), problem.as_str()), ("max_rework", "written twice"));
        let broken = faults(TEAM, "stages: [");
        assert_eq!((broken[0].0.as_str(), broken[0].1.as_str()), ("workflows/feature.md", ""));
        let unreadable = faults("roles: {", FEATURE);
        assert_eq!(
            unreadable.len(),
            1,
            "a role is not reported where the team file cannot be read: {unreadable:#?}"
        );
        assert_eq!(unreadable[0].0, "team.md");
    }

    #[test]
    fn the_directories_to_read_are_the_ones_the_team_file_names() {
        let team = file(
            TEAM_FILE,
            "roles: { dev: acme/dev, qa: acme/dev, ops: nowhere, n: 3, r: acme/r/roles/r, \
             own: ./roles/own, same: acme/team/roles/own/ }",
        );
        let at = dir("acme/team/teams/a");
        let dirs: Vec<String> =
            role_dirs(&team, Some(&at)).iter().map(RepoDir::to_string).collect();
        // A `./path` is read from the root of the team's repo, not from its directory.
        assert_eq!(dirs, ["acme/dev", "acme/r/roles/r", "acme/team/roles/own"]);
        let dirs: Vec<String> = role_dirs(&team, None).iter().map(RepoDir::to_string).collect();
        assert_eq!(dirs, ["acme/dev", "acme/r/roles/r", "acme/team/roles/own"]);
        assert!(role_dirs(&file(TEAM_FILE, "roles: [a/b]"), None).is_empty());
        assert!(role_dirs(&file(TEAM_FILE, "roles: {"), None).is_empty());
    }

    type Pairs = Vec<(String, String)>;

    /// A team at `at` whose file is `team`, with every directory it names
    /// read as holding the skill `plan` → its roles as (name, directory), or
    /// its faults as (key, problem).
    fn placed(at: &str, team: &str) -> Result<Pairs, Pairs> {
        let at = dir(at);
        let team = file(TEAM_FILE, team);
        let read = |dir: RepoDir| {
            let commit = format!("{}@1", dir.repo);
            (dir, Ok(RoleDir { commit, skills: vec!["plan".to_string()], ..RoleDir::default() }))
        };
        let roles = role_dirs(&team, Some(&at)).into_iter().map(read).collect();
        let sources = Sources { at: Some(at), team, workflows: vec![], patrols: vec![], roles };
        match load(&sources) {
            Ok(team) => {
                Ok(team.roles.iter().map(|r| (r.name.clone(), r.dir.to_string())).collect())
            }
            Err(faults) => Err(faults.into_iter().map(|f| (f.key, f.problem)).collect()),
        }
    }

    /// The one fault of the role `x` of a team at `at`, named `named` beside
    /// a role `fellow` at `./roles/fellow`.
    fn misplaced(at: &str, named: &str) -> String {
        let team = format!("roles: {{ fellow: ./roles/fellow, x: {named} }}");
        let mut faults = placed(at, &team).unwrap_err();
        assert_eq!(faults.len(), 1, "{faults:#?}");
        let (key, problem) = faults.remove(0);
        assert_eq!(key, "roles.x");
        problem
    }

    #[test]
    fn a_role_is_named_by_repo_by_repo_and_path_or_by_a_path_in_the_teams_repo() {
        let team = "roles: { fellow: ./roles/fellow, boss: acme/roles/boss/, dev: acme/dev }";
        let named = |name: &str, dir: &str| (name.to_string(), dir.to_string());
        let roles = [
            named("fellow", "acme/hospital/roles/fellow"),
            named("boss", "acme/roles/boss"),
            named("dev", "acme/dev"),
        ];
        assert_eq!(placed("acme/hospital", team).unwrap(), roles);
        // A `./path` is read from the root of the team's repo, not from the
        // team's directory: a team in a subdirectory names the same role.
        assert_eq!(placed("acme/hospital/teams/night", team).unwrap(), roles);
        let whole = load(&sources(TEAM, &[])).unwrap();
        assert_eq!(
            (whole.roles[0].dir.to_string(), whole.roles[0].dir.path()),
            ("acme/dev".to_string(), "")
        );
    }

    #[test]
    fn two_names_for_one_directory_are_two_roles() {
        let team = "roles: { fellow: ./roles/fellow, night: acme/hospital/roles/fellow }";
        let roles = placed("acme/hospital", team).unwrap();
        assert_eq!(roles[0].1, roles[1].1);
        assert_eq!((roles[0].0.as_str(), roles[1].0.as_str()), ("fellow", "night"));
    }

    #[test]
    fn a_path_that_leaves_its_repo_or_starts_at_no_repos_root_is_a_fault() {
        for leaves in ["./roles/../../x", "acme/roles/../x", "./.."] {
            let problem = misplaced("acme/hospital", leaves);
            assert!(problem.contains("has a `..`"), "{leaves}: {problem}");
        }
        for absolute in ["/roles/x", "acme/roles//x"] {
            let problem = misplaced("acme/hospital", absolute);
            assert!(problem.contains("is an absolute path"), "{absolute}: {problem}");
        }
        for formless in ["x", "../x", "acme", "roles/"] {
            let problem = misplaced("acme/hospital", formless);
            assert_eq!(
                problem, "must be a role's directory, as owner/repo, owner/repo/path or ./path",
                "{formless}"
            );
        }
        assert_eq!(
            misplaced("acme/hospital", "3"),
            "must be a role's directory, as owner/repo, owner/repo/path or ./path, not a number"
        );
    }

    #[test]
    fn a_roles_directory_is_not_the_teams_and_does_not_contain_it() {
        assert_eq!(
            misplaced("acme/hospital/teams/night", "./teams/night"),
            "its directory is the team's own (acme/hospital/teams/night): a role is a directory \
             of its own"
        );
        assert_eq!(
            misplaced("acme/hospital/teams/night", "acme/hospital/teams"),
            "its directory acme/hospital/teams contains the team's (acme/hospital/teams/night)"
        );
        assert_eq!(
            placed("acme/hospital/teams/night", "roles: { x: ./ }").unwrap_err()[0].1,
            "its directory acme/hospital contains the team's (acme/hospital/teams/night)"
        );
        // Under the team's directory is where a team keeps its roles; among
        // the team's own files is nowhere for one.
        assert!(placed("acme/hospital", "roles: { x: ./roles/x }").is_ok());
        assert_eq!(
            misplaced("acme/hospital", "./workflows/x"),
            "its directory acme/hospital/workflows/x is inside the team's workflows/"
        );
        assert_eq!(
            misplaced("acme/hospital/teams/night", "./teams/night/patrols"),
            "its directory acme/hospital/teams/night/patrols is inside the team's patrols/"
        );
        // Another repo's directory of the same path is another directory.
        assert!(placed("acme/hospital", "roles: { x: acme/other }").is_ok());
    }

    #[test]
    fn a_roles_directory_neither_contains_another_roles_nor_is_inside_one() {
        assert_eq!(
            misplaced("acme/hospital", "./roles/fellow/night"),
            "its directory acme/hospital/roles/fellow/night is inside that of the role `fellow` \
             (acme/hospital/roles/fellow)"
        );
        assert_eq!(
            misplaced("acme/hospital", "./roles"),
            "its directory acme/hospital/roles contains that of the role `fellow` \
             (acme/hospital/roles/fellow)"
        );
        assert!(
            placed("acme/hospital", "roles: { a: ./roles/fellow, b: ./roles/fellow-2 }").is_ok()
        );
    }

    #[test]
    fn a_path_in_the_teams_repo_needs_the_repo_to_be_known() {
        let team = file(TEAM_FILE, "roles: { fellow: ./roles/fellow }");
        let sources = Sources { at: None, team, workflows: vec![], patrols: vec![], roles: vec![] };
        let faults = load(&sources).unwrap_err();
        assert_eq!(faults[0].key, "roles.fellow");
        assert!(faults[0].problem.contains("nothing says which repo"), "{}", faults[0].problem);
    }

    #[test]
    fn a_symlink_that_leaves_a_role_is_a_fault_and_rules_above_it_are_kept_to_be_said() {
        let at = dir("acme/hospital");
        let team = file(TEAM_FILE, "roles: { fellow: ./roles/fellow }");
        let read = |links: &[&str], above: &[&str]| {
            let all = |list: &[&str]| list.iter().map(|one| one.to_string()).collect();
            let read = RoleDir {
                commit: "c1".to_string(),
                skills: vec![],
                links: all(links),
                above: all(above),
                ..RoleDir::default()
            };
            let roles = vec![(dir("acme/hospital/roles/fellow"), Ok(read))];
            load(&Sources {
                at: Some(at.clone()),
                team: team.clone(),
                workflows: vec![],
                patrols: vec![],
                roles,
            })
        };
        let faults = read(&["hooks/guard.sh -> ../../shared/guard.sh"], &[]).unwrap_err();
        assert_eq!(faults[0].key, "roles.fellow");
        assert!(
            faults[0].problem.starts_with(
                "acme/hospital/roles/fellow holds a symlink that leaves it: hooks/guard.sh -> \
                 ../../shared/guard.sh."
            ),
            "{}",
            faults[0].problem
        );
        let loaded = read(&[], &["CLAUDE.md", "roles/.claude/"]).unwrap();
        assert_eq!(loaded.roles[0].above, ["CLAUDE.md", "roles/.claude/"]);
    }

    #[test]
    fn a_directory_that_is_not_at_the_head_of_its_repo_is_a_fault() {
        let team = file(TEAM_FILE, "roles: { fellow: ./roles/fellow }");
        let refused = Unread::Refused("it is no directory at the head of acme/hospital".into());
        let roles = vec![(dir("acme/hospital/roles/fellow"), Err(refused))];
        let at = Some(dir("acme/hospital"));
        let faults =
            load(&Sources { at, team, workflows: vec![], patrols: vec![], roles }).unwrap_err();
        assert_eq!(
            (faults[0].key.as_str(), faults[0].problem.as_str()),
            (
                "roles.fellow",
                "acme/hospital/roles/fellow cannot be read: it is no directory at the head of \
                 acme/hospital"
            )
        );
    }

    #[test]
    fn a_workflow_is_named_by_its_file() {
        assert_eq!(workflow_name("workflows/feature.md"), Some("feature"));
        assert_eq!(workflow_name("workflows/small-fix.md"), Some("small-fix"));
        for path in
            ["workflows/.md", "workflows/a/b.md", "workflows/a.yaml", "team.md", "workflowsx/a.md"]
        {
            assert_eq!(workflow_name(path), None, "{path}");
        }
    }
}
