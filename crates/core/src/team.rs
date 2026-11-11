//! A team as its files define it, loaded into a value that cannot hold a
//! malformed workflow or patrol (docs/adr/0008, docs/adr/0010). `load` is the only way
//! to a `Team`: it yields one, or every fault with the file and key it is in.
//! Whoever holds a `Team` checks nothing again.

use flux_rs::attrs::*;

use crate::config::{ESCALATE_LABEL, NotADir, RepoDir, RepoLabels};
use crate::harness::Harness;

/// The team file, at the top of a team's repo.
pub const TEAM_FILE: &str = "team.md";
/// The directory of a team's repo that holds one file per workflow.
pub const WORKFLOWS: &str = "workflows";
/// The directory of a team's repo that holds one file per patrol.
pub const PATROLS: &str = "patrols";
/// The directory of a role's repo that holds one entry per skill, the file
/// of a skill's directory that says what the skill is, and a directory's
/// rules — in a role's directory the role's, and in a team's the team rules:
/// where every harness reads them (docs/adr/0013), the harness crate's.
pub use afkconvoy_harness::start::{RULES, SKILL_FILE, SKILLS};
/// The keys of a skill's frontmatter every harness reads: what `skill-file`,
/// the help topic, must name.
pub const SKILL_KEYS: [&str; 2] = ["name", "description"];
/// What a workflow's flow label is made of where nothing names another: this,
/// then the workflow's name. A label made so is a flow label though it names
/// no workflow of the team.
pub const FLOW_LABEL: &str = "flow:";
/// How much rework an issue may have in total, where its workflow does not say.
pub const MAX_REWORK: u64 = 3;
/// How often one stage may reject in a row, where its workflow does not say.
pub const MAX_CONSECUTIVE_REWORK: u64 = 2;

/// The labels a team's workflows take: what, among an issue's labels, hands
/// it to one. Where no team was read it holds none, and only a label written
/// as one — `flow:…` — is taken for a flow label.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlowLabels(Vec<String>);

impl FlowLabels {
    /// The flow labels among `labels`, in their order: each a workflow's, or
    /// written as one (`flow:…`) though it names none.
    pub fn among<'l>(&self, labels: &'l [String]) -> Vec<&'l str> {
        let mut among = vec![];
        for label in labels {
            if label.starts_with(FLOW_LABEL) || self.0.contains(label) {
                among.push(label.as_str());
            }
        }
        among
    }

    /// Whether `label` is one of the team's workflows' flow labels.
    pub fn takes(&self, label: &str) -> bool {
        self.0.iter().any(|taken| taken.eq_ignore_ascii_case(label))
    }
}

/// Who performs a stage that no role does.
pub const HUMAN: &str = "human";
/// Where a `pass` leads that ends a workflow in the landing train.
pub const LAND: &str = "land";
/// Where a `pass` leads that ends a workflow in nothing more to do.
pub const DONE: &str = "done";

/// The key of the team file that says whether a worker of the team is given
/// the host user's skills and rules (`Team::host_user`).
const HOST_USER_KEY: &str = "host_user";
const DSH_PROFILE_KEY: &str = "dsh_profile";

/// The profile a DSH worker loads, independent of host skills and rules.
pub use afkconvoy_harness::DshProfile;

/// The key of a role named as a map that names its directory.
const DIR_KEY: &str = "dir";
/// The key of a roster's entry that names a harness its role may run.
const HARNESS_KEY: &str = "harness";
/// The key of a roster's entry that names a model of that harness.
const MODEL_KEY: &str = "model";
/// The keys a stage and a patrol once named their worker's harness and
/// model under, which the roster says now (docs/adr/0020): refused, with
/// what to write instead.
const RETIRED_KEYS: [&str; 2] = [HARNESS_KEY, MODEL_KEY];
/// The key of the team file that holds its roster.
pub const ROSTER_KEY: &str = "roster";
/// The key of a workflow's file that names its flow label.
const LABEL_KEY: &str = "label";

/// The keys of each map a definition holds: what `check`'s help must name.
pub const TEAM_KEYS: [&str; 6] =
    ["escalate_label", "roles", ROSTER_KEY, HOST_USER_KEY, DSH_PROFILE_KEY, "split"];
/// The keys of a role named as a map, beside the directory that names it.
pub const ROLE_KEYS: [&str; 3] = [DIR_KEY, HOST_USER_KEY, DSH_PROFILE_KEY];
/// The keys of a role's entry in the roster.
pub const ROSTER_ENTRY_KEYS: [&str; 2] = ["cast", "run"];
/// The keys of one harness and model of a role's entry in the roster.
pub const RUN_KEYS: [&str; 3] = [HARNESS_KEY, MODEL_KEY, "good_at"];
pub const WORKFLOW_KEYS: [&str; 5] =
    [LABEL_KEY, "max_rework", "max_consecutive_rework", "stages", LAND];
pub const STAGE_KEYS: [&str; 6] = ["name", "by", "do", "writes", "escalate", "on"];
pub const ON_KEYS: [&str; 2] = ["pass", "reject"];
pub const ESCALATE_KEYS: [&str; 2] = ["to", "do"];
pub const LAND_KEYS: [&str; 2] = ["by", "do"];
pub const PATROL_KEYS: [&str; 6] =
    ["by", "do", "every", "max_open", "max_runs_a_day", "max_landed_a_day"];

/// One harness a role's workers may run, by its team's roster, on one of
/// its models or on its command as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allowed {
    pub harness: Harness,
    /// Its harness's own name for the model. `None`: any model the host kit
    /// holds for the harness, or its command as written where it holds none.
    pub model: Option<String>,
    /// What the model is good at, as research says.
    pub good_at: Option<String>,
}

/// A role's entry in its team's roster (docs/adr/0020): the harnesses and
/// models its workers may run, in order of preference. It always allows one
/// at least. A `cast` key is read as text and ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub run: Vec<Allowed>,
}

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
    /// The skills it holds where a harness finds them by itself
    /// (`Harness::skills`), for each harness it holds such a directory for.
    pub own_skills: Vec<(Harness, Vec<String>)>,
    /// Each of its skills whose file is not in the form of a skill's file,
    /// with why (`malformed`).
    pub malformed: Vec<(String, String)>,
}

/// Why the file of the skill `skill` is not in the form every harness reads
/// a skill in, `frontmatter` being the yaml it opens with, or why it has
/// none that can be read. `None`: it holds a `name` that is the skill's — its
/// directory's, and what a `do` says — and a `description`. Any other key is
/// a harness's own.
pub fn malformed(skill: &str, frontmatter: &Result<Node, String>) -> Option<String> {
    let entries = match frontmatter {
        Ok(Node::Map(entries)) => entries,
        Ok(other) => return Some(format!("opens with frontmatter that is {}", kind(other))),
        Err(why) => return Some(why.clone()),
    };
    match get(entries, "name") {
        Some(Node::Text(name)) if name == skill => {}
        Some(Node::Text(name)) => {
            return Some(format!("says its `name` is `{name}`, which its directory's is not"));
        }
        Some(other) => return Some(format!("says its `name` is {}", kind(other))),
        None => return Some("says no `name`".to_string()),
    }
    match get(entries, "description") {
        Some(Node::Text(description)) if !description.trim().is_empty() => None,
        Some(Node::Text(_)) | None => Some("says no `description`".to_string()),
        Some(other) => Some(format!("says its `description` is {}", kind(other))),
    }
}

/// Everything read of a team, and nothing yet checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    /// The team's directory: where a role named `./path` is, and what no
    /// role's directory may be or contain. `None` for files nothing says the
    /// repo of.
    pub at: Option<RepoDir>,
    pub team: File,
    /// Whether the team's directory holds a rules file (`RULES`).
    pub rules: bool,
    /// Every entry of the workflows directory.
    pub workflows: Vec<File>,
    /// Every entry of the patrols directory.
    pub patrols: Vec<File>,
    /// Each directory `role_dirs` names, with what was read of it or why it
    /// was not read.
    pub roles: Vec<(RepoDir, Result<RoleDir, Unread>)>,
    /// The labels of the repo the team is read for: no flow label is its lock
    /// label or an epic label.
    pub labels: RepoLabels,
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

/// A loaded team of `roles` roles. Whoever its workflows and patrols have
/// perform a skill is one of them.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(roles: int)]
pub struct Team {
    /// Swapped in for the flow label when the fleet hands an issue to a human.
    pub escalate_label: String,
    /// Whether it has team rules: a `RULES` in its directory, which every
    /// worker of the team is given with that directory.
    pub rules: bool,
    /// Whether a worker of the team is given the host user's own skills and
    /// rules beside its role's (docs/adr/0017): `true` unless the team file
    /// says `host_user: false`, which keeps a role the same on every host.
    /// The default of every role, which a role may set for itself
    /// (`Role::host_user`).
    pub host_user: bool,
    /// The team's default or the role's own DSH profile choice.
    pub dsh_profile: DshProfile,
    /// Whether stage briefs offer splitting an issue: true unless `split: false`.
    pub split: bool,
    #[field(Vec<Role>[roles])]
    pub roles: Vec<Role>,
    #[field(Vec<Workflow{v: v.roles == roles}>)]
    pub workflows: Vec<Workflow>,
    #[field(Vec<Patrol[roles]>)]
    pub patrols: Vec<Patrol>,
}

impl Team {
    /// The labels its workflows take, each one's flow label.
    pub fn flow_labels(&self) -> FlowLabels {
        FlowLabels(self.workflows.iter().map(|workflow| workflow.label.clone()).collect())
    }

    /// The flow labels among `labels`, in their order.
    pub fn flows<'l>(&self, labels: &'l [String]) -> Vec<&'l str> {
        self.flow_labels().among(labels)
    }

    /// The workflow whose flow label is `label`.
    #[spec(fn(&Team[@roles], label: &str) -> Option<&Workflow{v: v.roles == roles}>)]
    pub fn labelled(&self, label: &str) -> Option<&Workflow> {
        self.workflows.iter().find(|workflow| workflow.label == label)
    }

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

    /// The harnesses a worker of `role` may run on a host whose host kit
    /// holds `kit`, in order: its roster entry's, or the host kit's where
    /// the roster names none for it.
    pub fn allowed(role: &Role, kit: &[Harness]) -> Vec<Harness> {
        let Some(entry) = &role.roster else { return kit.to_vec() };
        let mut allowed: Vec<Harness> = vec![];
        for held in &entry.run {
            if !allowed.contains(&held.harness) {
                allowed.push(held.harness);
            }
        }
        allowed
    }

    /// Holds the team to the harnesses its workers may run, on a host whose
    /// host kit holds `kit` (`Team::allowed`): a skill a worker is asked to
    /// perform whose file is not in the form every harness reads one in is a
    /// fault, and a harness that would not show a worker its skill is said.
    /// A harness a role holds no limits for is a fault of the roster where
    /// it allows it (`unlimited`), and `run`'s to refuse where the host kit
    /// gives it.
    pub fn held(&self, kit: &[Harness]) -> Held {
        let mut held = Held::default();
        for run in self.runs() {
            for harness in Team::allowed(&run.role, kit) {
                if let Some(note) = run.unfound(harness) {
                    held.notes.push(note);
                }
            }
            if let Some(problem) = run.malformed() {
                held.faults.push(Fault { file: run.file, key: run.skill_key, problem });
            }
        }
        held
    }

    /// Every worker the team would have started, whatever the issue, as
    /// (its role's name, what calls for it in words, the skill it performs):
    /// what a drafter of the roster is told of each role.
    pub fn duties(&self) -> Vec<(String, String, String)> {
        self.runs().into_iter().map(|run| (run.role.name, run.by, run.skill)).collect()
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
                skill_key: "do".to_string(),
                by: format!("the patrol `{}`", patrol.name),
                role: self.role_of(&patrol.work).clone(),
                skill: patrol.work.skill.clone(),
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
    /// What a human is told and is no fault: each worker whose role holds
    /// skills where a harness it may run finds them by itself, the one it is
    /// asked to perform not among them.
    pub notes: Vec<String>,
}

/// One worker a team would have started: what calls for it and where that is
/// written, its role, and the skill it is asked to perform.
struct Run {
    file: String,
    /// The key its skill is written at.
    skill_key: String,
    /// What calls for it, in words.
    by: String,
    role: Role,
    /// The skill it is asked to perform.
    skill: String,
}

impl Run {
    /// Why the skill it is asked to perform is not sure to be shown to a
    /// worker: its file is not in the form every harness reads a skill in
    /// (`malformed`).
    fn malformed(&self) -> Option<String> {
        let role = &self.role;
        let (_, why) = role.malformed.iter().find(|(skill, _)| *skill == self.skill)?;
        Some(format!(
            "`{}`, the skill {} has the role `{}` perform, has a file no harness is sure to show \
a worker: {}/{SKILLS}/{}/{SKILL_FILE} {why}. A skill's file opens with frontmatter that holds \
`{}`, the name of its directory, and `{}`: `afkconvoy help skill-file` says its form",
            self.skill, self.by, role.name, role.dir, self.skill, SKILL_KEYS[0], SKILL_KEYS[1]
        ))
    }

    /// Why a worker of it on `harness` would not be shown the skill it is
    /// asked to perform: its role holds a directory `harness` finds skills
    /// in by itself, which the fleet leaves as it is, and the skill is not
    /// there. `None` of a role that holds no such directory: the fleet has
    /// that harness find the role's skills.
    fn unfound(&self, harness: Harness) -> Option<String> {
        let role = &self.role;
        let (_, own) = role.own_skills.iter().find(|(holds, _)| *holds == harness)?;
        if own.contains(&self.skill) {
            return None;
        }
        let dirs: Vec<String> = harness.skills().iter().map(|dir| format!("`{dir}/`")).collect();
        Some(format!(
            "{} runs on `{}`, and its role `{}` holds skills of its own for {} ({}), `{}` not \
among them: {} finds skills there and not in `{SKILLS}/`, and the fleet leaves what a role holds \
there as it is, so a worker would not be shown the skill it is asked to perform. Hold the skill \
there too, or hold neither directory: the fleet then has {} find every skill of `{SKILLS}/`",
            self.by,
            harness.key(),
            role.name,
            harness.name(),
            dirs.join(" or "),
            self.skill,
            harness.name(),
            harness.name()
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
                skill_key: under(&at, "do"),
                by: by.clone(),
                role: team.role_of(work).clone(),
                skill: work.skill.clone(),
            });
        }
        if let Some(Consult::Role(consult)) = &stage.consult {
            runs.push(Run {
                file: file.clone(),
                skill_key: under(&under(&at, "escalate"), "do"),
                by: format!("the consult of {by}"),
                role: team.role_of(consult).clone(),
                skill: consult.skill.clone(),
            });
        }
    }
    if let Some(land) = &workflow.land {
        runs.push(Run {
            file,
            skill_key: under(LAND, "do"),
            by: format!("the landing of the workflow `{}`", workflow.name),
            role: team.role_of(land).clone(),
            skill: land.skill.clone(),
        });
    }
    runs
}

/// One role of a team, as its directory stood when the team was loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Role {
    /// What the team's workflows call it.
    pub name: String,
    /// Its directory: a whole repo, or a subdirectory of one.
    pub dir: RepoDir,
    /// Whether its worker is given the host user's own skills and rules
    /// beside its role's: what the team file says of it, `{ dir, host_user }`,
    /// and the team's (`Team::host_user`) where it says nothing.
    pub host_user: bool,
    /// The team's default or the role's own DSH profile choice.
    pub dsh_profile: DshProfile,
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
    /// The skills it holds where a harness finds them by itself, for each
    /// harness it holds such a directory for (`RoleDir::own_skills`): what a
    /// worker on that harness is shown, in place of the role's skills.
    pub own_skills: Vec<(Harness, Vec<String>)>,
    /// Each of its skills whose file is not in the form of a skill's file,
    /// with why (`RoleDir::malformed`).
    pub malformed: Vec<(String, String)>,
    /// Its entry in the team's roster. `None`: the roster names none for it,
    /// and it ranges over the host kit whole.
    pub roster: Option<Entry>,
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
    /// How many issues it opened may be open before it is skipped.
    pub max_open: u64,
    /// How many of its runs the last day may hold before it is skipped, when
    /// it says; fewer than its `every` lets it.
    pub max_runs_a_day: Option<u64>,
    /// How many issues it opened may have completed in the last rolling day.
    pub max_landed_a_day: Option<u64>,
}

/// Who a stage hands an issue to before a human (docs/adr/0018): `by_role`
/// when a role it names, one of a team of `roles`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[refined_by(by_role: bool, roles: int)]
pub enum Consult {
    #[variant((Work[@roles]) -> Consult[true, roles])]
    Role(Work),
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
    /// Whether it may commit. A stage that reviews does not.
    pub writes: bool,
    /// Who is asked before a human whenever the stage would hand the issue
    /// to one: the role it names. With no named consult, it escalates for the watch.
    /// `None` where it says `escalate: { to: human }`, and of a stage a
    /// human performs.
    #[field(Option<Consult{v: v.by_role => v.roles == roles}>)]
    pub consult: Option<Consult>,
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
    #[spec(fn(&Stage[@s]) -> Option<usize{v: v < s.at}>)]
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
    /// The name of its file: `workflows/<name>.md`.
    pub name: String,
    /// The label an issue carries that follows it: what its file's `label`
    /// names, and else `flow:<name>`.
    label: String,
    /// What it is for, as whoever hands the fleet work reads it: the prose of
    /// its file. Empty when the file says nothing.
    pub description: String,
    pub max_rework: u64,
    pub max_consecutive_rework: u64,
    #[field(Vec<Stage{v: v.n == n && v.roles == roles}>[n])]
    stages: Vec<Stage>,
    /// Who gates the PR and lands in a turn.
    #[field(Option<Work[roles]>)]
    pub land: Option<Work>,
    /// How many roles its team has.
    #[field(usize[roles])]
    roles: usize,
}

impl Workflow {
    pub fn flow_label(&self) -> &str {
        &self.label
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
        label: String::new(),
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
            if let Some(named) = role_named(named)
                && let Ok(dir) = role_dir(named, at)
                && !dirs.contains(&dir)
            {
                dirs.push(dir);
            }
        }
    }
    dirs
}

/// The directory a role's entry of a team file names: the entry itself, or
/// its `dir` where it is a map. `None` where it names none as text.
fn role_named(entry: &Node) -> Option<&str> {
    match entry {
        Node::Text(named) => Some(named),
        Node::Map(entries) => match get(entries, DIR_KEY) {
            Some(Node::Text(named)) => Some(named),
            _ => None,
        },
        _ => None,
    }
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

/// The most characters a patrol's name may have.
pub const PATROL_NAME_MOST: usize = 40;

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
    if name.len() <= PATROL_NAME_MOST && name.split('-').all(word) { Some(name) } else { None }
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
    let host_user = team_bool_key(&mut faults, sources, HOST_USER_KEY);
    let mut report = Report { file: &sources.team.path, faults: &mut faults };
    let dsh_profile = match &sources.team.yaml {
        Ok(Node::Map(entries)) => profile_key(
            &mut report,
            DSH_PROFILE_KEY,
            get(entries, DSH_PROFILE_KEY),
            DshProfile::Fleet,
        ),
        _ => DshProfile::Fleet,
    };
    let split = team_bool_key(&mut faults, sources, "split");
    let (escalate_label, mut roles) = team_file(&mut faults, sources, host_user, dsh_profile);
    roster_key(&mut faults, sources, &mut roles);
    let mut workflows: Vec<Workflow> = vec![];
    for file in &sources.workflows {
        let mut report = Report { file: &file.path, faults: &mut faults };
        let label = label_key(&mut report, file);
        if let Some(mut read) = workflow_file(&mut report, file, &roles) {
            read.label = label.unwrap_or_else(|| format!("{FLOW_LABEL}{}", read.name));
            workflows.push(read);
        }
    }
    flow_labels(&mut faults, sources, &escalate_label, &workflows);
    let mut patrols: Vec<Patrol> = vec![];
    for file in &sources.patrols {
        let mut report = Report { file: &file.path, faults: &mut faults };
        patrols.extend(patrol_file(&mut report, file, &roles));
    }
    if faults.is_empty() {
        Ok(Team {
            escalate_label,
            rules: sources.rules,
            host_user,
            dsh_profile,
            split,
            roles: roles.loaded,
            workflows,
            patrols,
        })
    } else {
        Err(faults)
    }
}

/// Reads a workflow's file's `label` → the flow label it names; `None` where
/// it names none, or the key is at fault. What is wrong with the file itself
/// is `workflow_file`'s to say.
fn label_key(report: &mut Report, file: &File) -> Option<String> {
    let Ok(Node::Map(entries)) = &file.yaml else { return None };
    text(report, "", entries, LABEL_KEY).map(str::to_string)
}

/// Reports a flow label two of `workflows` take, and one that is the
/// escalate label `escalate`, the lock label or an epic label: under `label`
/// where the file names it, and of the file where it is `flow:<name>`.
fn flow_labels(faults: &mut Vec<Fault>, sources: &Sources, escalate: &str, workflows: &[Workflow]) {
    let labels = &sources.labels;
    // Each label taken so far, with the file that took it.
    let mut taken: Vec<(String, String)> = vec![];
    for workflow in workflows {
        let file = format!("{WORKFLOWS}/{}.md", workflow.name);
        let mut report = Report { file: &file, faults: &mut *faults };
        let label = workflow.label.clone();
        let written = label != format!("{FLOW_LABEL}{}", workflow.name);
        let key = if written { LABEL_KEY } else { "" };
        let its = if written { format!("`{label}`") } else { format!("its flow label `{label}`") };
        if label == escalate {
            report.fault(key, format!("{its} is the team's escalate label"));
        } else if label == labels.lock {
            report.fault(key, format!("{its} is the repo's lock label"));
        } else if labels.epics.contains(&label) {
            report.fault(key, format!("{its} is an epic label of the repo"));
        }
        match taken.iter().find(|(held, _)| *held == label) {
            Some((_, by)) => report.fault(key, format!("{its} is already the flow label of {by}")),
            None => taken.push((label, file)),
        }
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
            let problem = if report.file == TEAM_FILE && at.is_empty() && key == "precedents" {
                "retired: migrate effective rules from precedents.md into their owning ADR, team or role rules, preserve source links, then remove the precedents key; rulings remain on their issues".to_string()
            } else {
                format!("unknown key; the keys here are: {known}")
            };
            report.fault(&under(at, key), problem);
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

/// Reads the team file → its escalate label and its roles, each given the
/// host user's skills and rules as `host_user` says unless it says otherwise.
fn team_file<'n>(
    faults: &mut Vec<Fault>,
    sources: &'n Sources,
    host_user: bool,
    dsh_profile: DshProfile,
) -> (String, Roles<'n>) {
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
        let Some((named, host_user, dsh_profile)) =
            role_entry(&mut report, &key, named, host_user, dsh_profile)
        else {
            continue;
        };
        let dir = match role_dir(named, sources.at.as_ref()) {
            Ok(dir) => dir,
            Err(problem) => {
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
                // A file is one fault, whichever harnesses would load it.
                let mut files: Vec<(&str, Vec<&str>)> = vec![];
                for (harness, file) in &read.in_place {
                    match files.iter_mut().find(|(held, _)| *held == file.as_str()) {
                        Some((_, harnesses)) => harnesses.push(harness.name()),
                        None => files.push((file.as_str(), vec![harness.name()])),
                    }
                }
                for (file, harnesses) in files {
                    let load = if harnesses.len() == 1 { "loads" } else { "load" };
                    let problem = format!(
                        "{dir} holds `{file}`: {} {load} that in place of `{RULES}`, the rules \
every harness works under. A role's rules are written once, in `{RULES}`; what one harness needs \
of its own is added to them, and never stands in their place",
                        harnesses.join(" and ")
                    );
                    report.fault(&key, problem)
                }
            }
            Some((_, Ok(read))) => loaded.push(Role {
                name: name.clone(),
                dir,
                host_user,
                dsh_profile,
                commit: read.commit.clone(),
                skills: read.skills.clone(),
                above: read.above.clone(),
                limits: read.limits.clone(),
                own_skills: read.own_skills.clone(),
                malformed: read.malformed.clone(),
                roster: None,
                unreached: None,
            }),
            Some((_, Err(Unread::Refused(why)))) => {
                report.fault(&key, format!("{dir} cannot be read: {why}"))
            }
            Some((_, Err(Unread::Unreached(why)))) => loaded.push(Role {
                name: name.clone(),
                dir,
                host_user,
                dsh_profile,
                commit: String::new(),
                skills: vec![],
                above: vec![],
                limits: vec![],
                own_skills: vec![],
                malformed: vec![],
                roster: None,
                unreached: Some(why.clone()),
            }),
            None => report.fault(&key, format!("{dir} was not read")),
        }
    }
    (escalate_label, Roles { known: true, names, loaded })
}

/// Reads the team file's `roster` into each role it gives an entry: a map
/// of role names, each to its `run`, a list of the harnesses
/// and models the role's workers may run, in order of preference. What is
/// wrong with the file itself is `team_file`'s to say.
fn roster_key(faults: &mut Vec<Fault>, sources: &Sources, roles: &mut Roles) {
    let mut report = Report { file: &sources.team.path, faults };
    let Ok(Node::Map(entries)) = &sources.team.yaml else { return };
    let Some(node) = get(entries, ROSTER_KEY) else { return };
    let Node::Map(named) = node else {
        let problem = format!(
            "must be a map of role names, each to the harnesses and models its workers may run, \
not {}",
            kind(node)
        );
        report.fault(ROSTER_KEY, problem);
        return;
    };
    let mut seen: Vec<&str> = vec![];
    for (name, node) in named {
        let key = under(ROSTER_KEY, name);
        if seen.contains(&name.as_str()) {
            report.fault(&key, "written twice".to_string());
            continue;
        }
        seen.push(name);
        if roles.known && !roles.names.contains(&name.as_str()) {
            let problem = format!(
                "`{name}` is not a role of the team; {TEAM_FILE} names: {}",
                roles.names.join(", ")
            );
            report.fault(&key, problem);
            continue;
        }
        let Some(entry) = roster_entry(&mut report, &key, node) else { continue };
        let Some(role) = roles.loaded.iter_mut().find(|role| role.name == *name) else {
            continue;
        };
        for (at, allowed) in entry.run.iter().enumerate() {
            if let Some(problem) = unlimited(role, allowed.harness) {
                report.fault(&under(&format!("{key}.run[{at}]"), HARNESS_KEY), problem);
            }
        }
        role.roster = Some(entry);
    }
}

/// Reads one role's entry of the roster at `key`. `None` when it is at
/// fault, reported.
fn roster_entry(report: &mut Report, key: &str, node: &Node) -> Option<Entry> {
    let entries = map(report, key, node, &ROSTER_ENTRY_KEYS)?;
    let _ignored = text(report, key, entries, "cast");
    let at = under(key, "run");
    let listed = match get(entries, "run") {
        Some(Node::List(listed)) if !listed.is_empty() => listed,
        Some(Node::List(_)) => {
            report.fault(&at, "a role's entry allows one harness at least".to_string());
            return None;
        }
        Some(other) => {
            let problem = format!("must be a list of harnesses and models, not {}", kind(other));
            report.fault(&at, problem);
            return None;
        }
        None => {
            report.fault(&at, "required, and it is missing".to_string());
            return None;
        }
    };
    let mut run: Vec<Allowed> = vec![];
    let mut whole = true;
    for (place, node) in listed.iter().enumerate() {
        let key = format!("{at}[{place}]");
        let Some(entries) = map(report, &key, node, &RUN_KEYS) else {
            whole = false;
            continue;
        };
        let named = required(report, &key, entries, HARNESS_KEY);
        let model = text(report, &key, entries, MODEL_KEY).map(str::to_string);
        let good_at = text(report, &key, entries, "good_at").map(str::to_string);
        let Some(named) = named else {
            whole = false;
            continue;
        };
        let Some(harness) = Harness::read(named) else {
            report.fault(&under(&key, HARNESS_KEY), Harness::unknown(named));
            whole = false;
            continue;
        };
        if run.iter().any(|held| held.harness == harness && held.model == model) {
            report.fault(&key, "written twice: it names what an earlier one does".to_string());
            whole = false;
            continue;
        }
        run.push(Allowed { harness, model, good_at });
    }
    whole.then_some(Entry { run })
}

/// Why `role` is not right for `harness`, which its roster entry allows:
/// the role holds limits, and none for that. A role with no limits at all is
/// right for every harness, and of one that was not read nothing is known.
pub fn unlimited(role: &Role, harness: Harness) -> Option<String> {
    if role.limits.is_empty() || role.limits.contains(&harness) {
        return None;
    }
    let holds: Vec<String> = role
        .limits
        .iter()
        .map(|held| format!("`{}` (in its `{}`)", held.key(), held.limits()))
        .collect();
    Some(format!(
        "the role `{}` may run `{}`, and holds limits for {} and none for `{}`: a worker there \
would be held to none of them. Write the role's limits for `{}` in its `{}`, in that harness's \
own form, or allow it only harnesses it holds limits for",
        role.name,
        harness.key(),
        holds.join(" and "),
        harness.key(),
        harness.key(),
        harness.limits()
    ))
}

/// Reports each key of `entries`, at `at`, that names a worker's harness or
/// model, which a stage or a patrol names no longer: the line to delete,
/// and the roster entry of `role` — the one it was written for, where it
/// names one — to write in its place.
fn retired(report: &mut Report, at: &str, entries: &[(String, Node)], role: Option<&str>) {
    let written = |key: &str| match get(entries, key) {
        Some(Node::Text(text)) => Some(text.as_str()),
        _ => None,
    };
    let (harness, model) = (written(HARNESS_KEY), written(MODEL_KEY));
    let mut allowed = format!("{HARNESS_KEY}: {}", harness.unwrap_or("<harness>"));
    if let Some(model) = model {
        allowed.push_str(&format!(", {MODEL_KEY}: {model}"));
    }
    let role = role.unwrap_or("<role>");
    for key in RETIRED_KEYS {
        let Some(node) = get(entries, key) else { continue };
        let line = match node {
            Node::Text(text) => format!("{key}: {text}"),
            _ => key.to_string(),
        };
        let problem = format!(
            "names its worker's {key}, which a stage and a patrol name no more (docs/adr/0020): \
delete the line `{line}`, and say what the role `{role}` may run in {TEAM_FILE}'s `{ROSTER_KEY}`: \
`{ROSTER_KEY}: {{ {role}: {{ run: [{{ {allowed} }}] }} }}`"
        );
        report.fault(&under(at, key), problem);
    }
}

/// Reads the entry `key` of a team file's `roles` → the directory it names
/// and whether its worker is given the host user's skills and rules: `team`,
/// the team's, unless it is a map that says. `None` when it is at fault,
/// reported. Where its directory is right is `role_dir`'s to say.
fn role_entry<'n>(
    report: &mut Report,
    key: &str,
    entry: &'n Node,
    team: bool,
    profile: DshProfile,
) -> Option<(&'n str, bool, DshProfile)> {
    let entries = match entry {
        Node::Text(named) => return Some((named, team, profile)),
        Node::Map(_) => map(report, key, entry, &ROLE_KEYS)?,
        other => {
            let problem = format!(
                "must be a role's directory, as {ROLE_FORMS}, or a map of its `{DIR_KEY}` and \
`{HOST_USER_KEY}` and optional `{DSH_PROFILE_KEY}`, not {}",
                kind(other)
            );
            report.fault(key, problem);
            return None;
        }
    };
    let host_user = match get(entries, HOST_USER_KEY) {
        None => Some(team),
        Some(Node::Bool(host_user)) => Some(*host_user),
        Some(other) => {
            let problem = format!("must be true or false, not {}", kind(other));
            report.fault(&under(key, HOST_USER_KEY), problem);
            None
        }
    };
    let named = match get(entries, DIR_KEY) {
        Some(Node::Text(named)) => Some(named.as_str()),
        None => {
            report.fault(&under(key, DIR_KEY), "required, and it is missing".to_string());
            None
        }
        Some(other) => {
            let problem =
                format!("must be a role's directory, as {ROLE_FORMS}, not {}", kind(other));
            report.fault(&under(key, DIR_KEY), problem);
            None
        }
    };
    let profile =
        profile_key(report, &under(key, DSH_PROFILE_KEY), get(entries, DSH_PROFILE_KEY), profile);
    Some((named?, host_user?, profile))
}

/// Reads a profile choice, reporting every value outside the two named choices.
fn profile_key(
    report: &mut Report,
    key: &str,
    node: Option<&Node>,
    default: DshProfile,
) -> DshProfile {
    match node {
        None => default,
        Some(Node::Text(value)) if value == "fleet" => DshProfile::Fleet,
        Some(Node::Text(value)) if value == "host-user" => DshProfile::HostUser,
        Some(_) => {
            report.fault(key, "must be fleet or host-user".to_string());
            default
        }
    }
}

/// A boolean team setting, true where unsaid. A wrong type is reported.
fn team_bool_key(faults: &mut Vec<Fault>, sources: &Sources, key: &str) -> bool {
    let mut report = Report { file: &sources.team.path, faults };
    let Ok(Node::Map(entries)) = &sources.team.yaml else { return true };
    match get(entries, key) {
        None => true,
        Some(Node::Bool(host_user)) => *host_user,
        Some(other) => {
            report.fault(key, format!("must be true or false, not {}", kind(other)));
            true
        }
    }
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
    writes: Option<bool>,
    /// `None` when its `escalate` is at fault; `Some(None)` when it goes
    /// to a human.
    consult: Option<Option<Consult>>,
    pass: Option<&'n str>,
    reject: Option<&'n str>,
}

/// Reads one stage: everything that can be said of it alone.
fn draft<'n>(report: &mut Report, roles: &Roles, key: String, node: &'n Node) -> Option<Draft<'n>> {
    let known: Vec<&str> = STAGE_KEYS.iter().chain(&RETIRED_KEYS).copied().collect();
    let entries = map(report, &key, node, &known)?;
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
        for worker_key in ["do", "escalate"] {
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
    let by_role = match get(entries, "by") {
        Some(Node::Text(by)) if !human => Some(by.as_str()),
        _ => None,
    };
    retired(report, &key, entries, by_role);
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
        _ if human => Some(None),
        None => Some(None),
        Some(node) => {
            let at = under(&key, "escalate");
            match map(report, &at, node, &ESCALATE_KEYS) {
                Some(entries) if matches!(get(entries, "to"), Some(Node::Text(to)) if to == HUMAN) => {
                    match get(entries, "do") {
                        Some(_) => {
                            let problem = format!("`to: {HUMAN}` performs no skill");
                            report.fault(&under(&at, "do"), problem);
                            None
                        }
                        None => Some(None),
                    }
                }
                Some(entries) => {
                    work(report, roles, &at, entries, ("to", "do")).map(Consult::Role).map(Some)
                }
                None => None,
            }
        }
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
    Some(Draft { key, name, by, writes, consult, pass, reject })
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
    let known: Vec<&str> = PATROL_KEYS.iter().chain(&RETIRED_KEYS).copied().collect();
    let entries = map(report, "", node, &known)?;
    let by_role = match get(entries, "by") {
        Some(Node::Text(by)) => Some(by.as_str()),
        _ => None,
    };
    retired(report, "", entries, by_role);
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
    let max_open = limit(report, "", entries);
    let max_runs_a_day = runs_a_day(report, entries, every);
    let max_landed_a_day = daily_limit(report, entries, "max_landed_a_day");
    Some(Patrol {
        name: name.to_string(),
        work: work?,
        every: every?,
        max_open: max_open?,
        max_runs_a_day: max_runs_a_day?,
        max_landed_a_day: max_landed_a_day?,
    })
}

/// A patrol's `max_runs_a_day`, under its interval `every` where that was
/// read: a whole number from 1 up, and fewer runs than `every` lets a day
/// hold — as many or more would bound nothing. `Some(None)` where it says
/// none.
fn runs_a_day(
    report: &mut Report,
    entries: &[(String, Node)],
    every: Option<u64>,
) -> Option<Option<u64>> {
    let Some(read) = daily_limit(report, entries, "max_runs_a_day")? else { return Some(None) };
    if let Some(every) = every {
        let most = crate::patrol::runs_a_day(every);
        if read >= most {
            report.fault(
                "max_runs_a_day",
                format!(
                    "{read} bounds nothing: `every` already lets the patrol run at most {most} \
                     time(s) a day. Say fewer, or leave the key out"
                ),
            );
            return None;
        }
    }
    Some(Some(read))
}

/// An optional daily bound: omitted, or a whole number from one up.
fn daily_limit(report: &mut Report, entries: &[(String, Node)], key: &str) -> Option<Option<u64>> {
    let Some(node) = get(entries, key) else { return Some(None) };
    let read = match node {
        Node::Int(n) => u64::try_from(*n).ok().filter(|n| *n > 0),
        _ => None,
    };
    match read {
        Some(read) => Some(Some(read)),
        None => {
            let was = match node {
                Node::Int(_) => "a number below 1",
                Node::Fraction => "a number with a fraction",
                other => kind(other),
            };
            report.fault(key, format!("must be a whole number from 1 up, not {was}"));
            None
        }
    }
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
        Some(Consult::Role(work)) => Some(Consult::Role(among_roles(work, roles)?)),
        None => None,
    };
    Some(Stage {
        name: draft.name?.to_string(),
        by,
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
            label: String::new(),
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
            label: String::new(),
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
    escalate: { to: human }
    on: { pass: build }
  - name: build
    by: dev
    do: implement
    escalate: { to: reviewer, do: rule }
    on: { pass: review }
  - name: review
    by: reviewer
    do: review
    writes: false
    escalate: { to: human }
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
            rules: false,
            workflows: workflows.iter().map(|(path, yaml)| file(path, yaml)).collect(),
            patrols: vec![],
            roles: vec![
                repo("acme/dev", &["plan", "implement", "land"]),
                repo("acme/reviewer", &["review", "rule"]),
            ],
            labels: RepoLabels::default(),
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

    /// The usual team with a workflow `FEATURE` under each of `names`.
    pub(crate) fn named(names: &[&str]) -> Team {
        let paths: Vec<String> =
            names.iter().map(|name| format!("{WORKFLOWS}/{name}.md")).collect();
        let workflows: Vec<(&str, &str)> =
            paths.iter().map(|path| (path.as_str(), FEATURE)).collect();
        load(&sources(TEAM, &workflows)).unwrap()
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
        let sweep = "by: dev\ndo: plan\nevery: 6h\nmax_open: 3\nmax_runs_a_day: 2\n";
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
                max_open: 3,
                max_runs_a_day: Some(2),
                max_landed_a_day: None,
            })
        );
        assert_eq!(team.patrol("nightly-x2").map(|patrol| patrol.every), Some(86400));
        assert_eq!(team.patrol("nightly-x2").and_then(|patrol| patrol.max_runs_a_day), None);
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
        let Some(Consult::Role(consult)) = &build.consult else { panic!("a named consult") };
        assert_eq!(*consult, work(REVIEWER, "rule"));
        let consulted = team.role_of(consult);
        assert_eq!(
            (consulted.name.as_str(), consulted.dir.repo.as_str()),
            ("reviewer", "acme/reviewer")
        );
        assert!(build.writes && !review.writes, "a stage writes unless it says not");
        assert_eq!((review.pass(), review.reject()), (Pass::Stage(3), Some(1)));
        assert_eq!(sign_off.by, Performer::Human);
        // `plan` goes to a human; a human's stage has no consult.
        assert_eq!((&plan.consult, &sign_off.consult), (&None, &None));
        assert_eq!((sign_off.at(), sign_off.pass(), sign_off.reject()), (3, Pass::Land, Some(0)));
    }

    #[test]
    fn a_workflow_s_file_names_its_flow_label_or_takes_flow_and_its_name() {
        let named = format!("label: ready-for-agent\n{FEATURE}");
        let team = load(&sources(
            TEAM,
            &[("workflows/feature.md", &named), ("workflows/fix.md", FEATURE)],
        ))
        .unwrap();
        assert_eq!(team.workflow("feature").unwrap().flow_label(), "ready-for-agent");
        assert_eq!(team.workflow("fix").unwrap().flow_label(), "flow:fix");
        assert_eq!(team.labelled("ready-for-agent").unwrap().name, "feature");
        assert!(team.labelled("flow:feature").is_none());
        let carried = ["bug", "ready-for-agent", "flow:chore", "flow:fix"].map(String::from);
        // One a workflow takes, and one written as one though it names none.
        assert_eq!(team.flows(&carried), ["ready-for-agent", "flow:chore", "flow:fix"]);
        assert_eq!(FlowLabels::default().among(&carried), ["flow:chore", "flow:fix"]);
    }

    #[test]
    fn a_flow_label_two_workflows_take_or_the_fleet_s_own_is_a_fault() {
        let labelled = |label: &str| format!("label: {label}\n{FEATURE}");
        let faults = |team: &str, workflows: &[(&str, &str)]| {
            let faults = load(&sources(team, workflows)).unwrap_err();
            faults.into_iter().map(|f| (f.file, f.key, f.problem)).collect::<Vec<_>>()
        };
        let fault = |file: &str, key: &str, problem: &str| {
            (file.to_string(), key.to_string(), problem.to_string())
        };
        let shared = labelled("flow:fix");
        assert_eq!(
            faults(TEAM, &[("workflows/fix.md", FEATURE), ("workflows/feature.md", &shared)]),
            [fault(
                "workflows/feature.md",
                "label",
                "`flow:fix` is already the flow label of workflows/fix.md"
            )]
        );
        // Where the one that names it comes first, the other is the one at fault.
        assert_eq!(
            faults(TEAM, &[("workflows/feature.md", &shared), ("workflows/fix.md", FEATURE)]),
            [fault(
                "workflows/fix.md",
                "",
                "its flow label `flow:fix` is already the flow label of workflows/feature.md"
            )]
        );
        let one = |label: &str| faults(TEAM, &[("workflows/feature.md", &labelled(label))]);
        assert_eq!(
            one("ready-for-human"),
            [fault(
                "workflows/feature.md",
                "label",
                "`ready-for-human` is the team's escalate label"
            )]
        );
        let stuck = format!("escalate_label: stuck\n{TEAM}");
        let escalating = faults(&stuck, &[("workflows/feature.md", &labelled("stuck"))]);
        assert_eq!(escalating[0].2, "`stuck` is the team's escalate label");
        assert_eq!(
            one("fleet-lock"),
            [fault("workflows/feature.md", "label", "`fleet-lock` is the repo's lock label")]
        );
        assert_eq!(
            one("prd"),
            [fault("workflows/feature.md", "label", "`prd` is an epic label of the repo")]
        );
        assert_eq!(one("''")[0].2, "must not be empty");
        // The repo's own labels are the ones it is held against.
        let mut stopped = sources(TEAM, &[("workflows/feature.md", &labelled("stop"))]);
        stopped.labels.lock = "stop".to_string();
        assert_eq!(load(&stopped).unwrap_err()[0].problem, "`stop` is the repo's lock label");
        assert!(load(&sources(TEAM, &[("workflows/feature.md", &labelled("stop"))])).is_ok());
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

    #[test]
    fn a_team_offers_splitting_unless_it_turns_it_off() {
        let feature = [("workflows/feature.md", FEATURE)];
        assert!(load(&sources(TEAM, &feature)).unwrap().split);
        let said = |value: &str| load(&sources(&format!("{TEAM}split: {value}\n"), &feature));
        assert!(said("true").unwrap().split);
        assert!(!said("false").unwrap().split);
        for value in ["no", "0", "[false]", "null"] {
            let faults = said(value).unwrap_err();
            let [fault] = &faults[..] else { panic!("one fault: {faults:#?}") };
            assert_eq!((fault.file.as_str(), fault.key.as_str()), (TEAM_FILE, "split"));
            assert!(fault.problem.starts_with("must be true or false, not "));
        }
    }

    #[test]
    fn a_team_gives_its_workers_the_host_user_s_skills_and_rules_unless_it_says_not() {
        let feature = [("workflows/feature.md", FEATURE)];
        assert!(load(&sources(TEAM, &feature)).unwrap().host_user);
        let said = |value: &str| load(&sources(&format!("{TEAM}host_user: {value}\n"), &feature));
        assert!(said("true").unwrap().host_user);
        assert!(!said("false").unwrap().host_user);
        for value in ["no", "0", "[false]"] {
            let faults = said(value).unwrap_err();
            let [fault] = &faults[..] else { panic!("one fault: {faults:#?}") };
            assert_eq!((fault.file.as_str(), fault.key.as_str()), (TEAM_FILE, "host_user"));
            assert!(fault.problem.starts_with("must be true or false, not "), "{}", fault.problem);
        }
    }

    #[test]
    fn a_role_may_set_its_own_host_user_and_one_named_by_its_directory_takes_the_team_s() {
        let feature = [("workflows/feature.md", FEATURE)];
        let host_user = |team: &str| -> Vec<(String, bool)> {
            let team = load(&sources(team, &feature)).unwrap();
            team.roles.iter().map(|role| (role.name.clone(), role.host_user)).collect()
        };
        let own = |value: &str, team: &str| {
            format!(
                "roles:\n  dev: acme/dev\n  reviewer: {{ dir: acme/reviewer, {value} }}\n{team}"
            )
        };
        let said = |dev: bool, reviewer: bool| {
            vec![("dev".to_string(), dev), ("reviewer".to_string(), reviewer)]
        };
        assert_eq!(host_user(&own("host_user: false", "")), said(true, false));
        assert_eq!(host_user(&own("host_user: true", "host_user: false\n")), said(false, true));
        assert_eq!(host_user(&own("host_user: false", "host_user: false\n")), said(false, false));
        // A map that names its directory alone takes the team's too.
        assert_eq!(
            host_user("roles:\n  dev: acme/dev\n  reviewer: { dir: acme/reviewer }\n"),
            said(true, true)
        );
        let fault = |team: &str| {
            let faults = load(&sources(team, &feature)).unwrap_err();
            let [fault] = &faults[..] else { panic!("one fault: {faults:#?}") };
            (fault.key.clone(), fault.problem.clone())
        };
        let (key, problem) = fault(&own("host_user: \"no\"", ""));
        assert_eq!(key, "roles.reviewer.host_user");
        assert_eq!(problem, "must be true or false, not text");
        let (key, problem) = fault(&own("host_user: false, model: opus", ""));
        assert_eq!(key, "roles.reviewer.model");
        assert!(problem.starts_with("unknown key; the keys here are: dir, host_user"), "{problem}");
        let (key, problem) = fault("roles:\n  dev: acme/dev\n  reviewer: { host_user: false }\n");
        assert_eq!(
            (key.as_str(), problem.as_str()),
            ("roles.reviewer.dir", "required, and it is missing")
        );
        let (key, _) = fault("roles:\n  dev: acme/dev\n  reviewer: [acme/reviewer]\n");
        assert_eq!(key, "roles.reviewer");
    }

    #[test]
    fn dsh_profiles_inherit_and_override_the_team_and_reject_other_values_at_each_key() {
        let feature = [("workflows/feature.md", FEATURE)];
        let own = |value: &str, team: &str| {
            format!(
                "roles:\n  dev: acme/dev\n  reviewer: {{ dir: acme/reviewer, dsh_profile: {value} }}\n{team}"
            )
        };
        for (key, inherited, value, expected) in [
            ("", DshProfile::Fleet, "host-user", DshProfile::HostUser),
            ("dsh_profile: host-user\n", DshProfile::HostUser, "fleet", DshProfile::Fleet),
        ] {
            let team = load(&sources(&own(value, key), &feature)).unwrap();
            assert_eq!(team.dsh_profile, inherited);
            assert_eq!(team.roles[DEV].dsh_profile, inherited);
            assert_eq!(team.roles[REVIEWER].dsh_profile, expected);
        }
        for value in ["true", "0", "null", "[fleet]", "{}", "other", "''"] {
            for (text, key) in [
                (format!("{TEAM}dsh_profile: {value}\n"), "dsh_profile"),
                (own(value, ""), "roles.reviewer.dsh_profile"),
            ] {
                let faults = load(&sources(&text, &feature)).unwrap_err();
                let [fault] = &faults[..] else { panic!("one fault: {faults:#?}") };
                assert_eq!((fault.file.as_str(), fault.key.as_str()), (TEAM_FILE, key));
                assert_eq!(fault.problem, "must be fleet or host-user");
            }
        }
    }

    /// A workflow whose second stage, by `dev`, has `rest` as the rest of it,
    /// and whose `land` is `land`.
    fn landed(rest: &str, land: &str) -> String {
        let second = format!("    by: dev\n    do: plan\n{rest}    on: {{ pass: land }}\n");
        format!("{}land: {land}\n", two("{ pass: b }", &second))
    }

    #[test]
    fn a_stage_or_a_patrol_that_names_a_harness_or_a_model_is_refused_with_the_roster_entry() {
        let usual = "{ by: dev, do: land }";
        let faults = faults(TEAM, &landed("    harness: pi\n    model: a/b\n", usual));
        let keys: Vec<&str> = faults.iter().map(|(_, key, _)| key.as_str()).collect();
        assert_eq!(keys, ["stages[1].harness", "stages[1].model"]);
        assert_eq!(
            faults[0].2,
            "names its worker's harness, which a stage and a patrol name no more \
             (docs/adr/0020): delete the line `harness: pi`, and say what the role `dev` may run \
             in team.md's `roster`: `roster: { dev: { run: [{ harness: pi, model: a/b }] } }`"
        );
        assert!(faults[1].2.contains("delete the line `model: a/b`"), "{}", faults[1].2);
        // A human's stage names neither, and says the same.
        let human = two("{ pass: b }", "    by: human\n    model: opus\n    on: { pass: done }\n");
        let (key, problem) = fault(&human);
        assert_eq!(key, "stages[1].model");
        assert!(problem.contains("`roster: { <role>: { run: [{ harness: <harness>, model: opus"));
        let patrol = "by: dev\ndo: plan\nevery: 6h\nharness: codex\nmax_open: 3\n";
        let faults = patrolled(&[("patrols/a.md", patrol)]).unwrap_err();
        assert_eq!(faults, [("patrols/a.md".to_string(), "harness".to_string())]);
    }

    /// The usual team with `roster` as its team file's roster, and `FEATURE`.
    pub fn rostered(roster: &str) -> Result<Team, Vec<(String, String, String)>> {
        let team = format!("{TEAM}roster:\n{roster}");
        let loaded = load(&sources(&team, &[("workflows/feature.md", FEATURE)]));
        loaded.map_err(|faults| {
            faults.into_iter().map(|fault| (fault.file, fault.key, fault.problem)).collect()
        })
    }

    #[test]
    fn a_roster_gives_each_role_it_names_its_entry_in_order_and_the_rest_none() {
        let team = rostered(
            "  dev:\n    cast: Hard work on opus.\n    run:\n      - { harness: claude-code, \
             model: opus, good_at: hard work }\n      - { harness: pi }\n",
        )
        .unwrap();
        let dev = team.role("dev").unwrap();
        let allowed = |harness, model: Option<&str>, good_at: Option<&str>| Allowed {
            harness,
            model: model.map(str::to_string),
            good_at: good_at.map(str::to_string),
        };
        assert_eq!(
            dev.roster,
            Some(Entry {
                run: vec![
                    allowed(Harness::ClaudeCode, Some("opus"), Some("hard work")),
                    allowed(Harness::Pi, None, None),
                ],
            })
        );
        assert_eq!(team.role("reviewer").unwrap().roster, None);
        // A role with an entry may run its harnesses; one with none, the host kit's.
        let kit = [Harness::Codex];
        assert_eq!(Team::allowed(dev, &kit), [Harness::ClaudeCode, Harness::Pi]);
        assert_eq!(Team::allowed(team.role("reviewer").unwrap(), &kit), [Harness::Codex]);
    }

    #[test]
    fn every_fault_of_a_roster_is_reported_where_it_is() {
        let keys = |roster: &str| {
            let faults = rostered(roster).unwrap_err();
            faults.into_iter().map(|(file, key, _)| format!("{file}: {key}")).collect::<Vec<_>>()
        };
        assert_eq!(keys("  boss: { run: [{ harness: pi }] }\n"), ["team.md: roster.boss"]);
        assert_eq!(keys("  dev: { run: [] }\n"), ["team.md: roster.dev.run"]);
        assert_eq!(keys("  dev: { cast: x }\n"), ["team.md: roster.dev.run"]);
        assert_eq!(keys("  dev: { run: pi }\n"), ["team.md: roster.dev.run"]);
        assert_eq!(
            keys("  dev: { run: [{ harness: aider }] }\n"),
            ["team.md: roster.dev.run[0].harness"]
        );
        assert_eq!(
            keys("  dev: { run: [{ model: opus }] }\n"),
            ["team.md: roster.dev.run[0].harness"]
        );
        assert_eq!(
            keys("  dev: { run: [{ harness: pi, effort: high }] }\n"),
            ["team.md: roster.dev.run[0].effort"]
        );
        assert_eq!(
            keys("  dev: { run: [{ harness: pi, model: a }, { harness: pi, model: a }] }\n"),
            ["team.md: roster.dev.run[1]"]
        );
        assert_eq!(
            keys("  dev: { run: [{ harness: pi }], how: x }\n"),
            ["team.md: roster.dev.how"]
        );
        let faults = load(&sources(&format!("{TEAM}roster: [dev]\n"), &[])).unwrap_err();
        assert_eq!(places(&faults), ["team.md: roster"]);
        let (_, _, problem) = &rostered("  boss: { run: [{ harness: pi }] }\n").unwrap_err()[0];
        assert_eq!(problem, "`boss` is not a role of the team; team.md names: dev, reviewer");
    }

    /// The usual team, `dev` and `reviewer` holding limits for these
    /// harnesses, with `workflow` as its one workflow, `patrol` as its one
    /// patrol and `roster` its roster.
    fn limited(
        dev: &[Harness],
        reviewer: &[Harness],
        workflow: &str,
        patrol: &str,
        roster: &str,
    ) -> Result<Team, Vec<Fault>> {
        let team = format!("{TEAM}{roster}");
        let mut sources = sources(&team, &[("workflows/feature.md", workflow)]);
        sources.patrols = vec![file("patrols/sweep.md", patrol)];
        for (role, limits) in [(DEV, dev), (REVIEWER, reviewer)] {
            sources.roles[role].1.as_mut().unwrap().limits = limits.to_vec();
        }
        load(&sources)
    }

    /// Where each fault is, as "file: key".
    fn places(faults: &[Fault]) -> Vec<String> {
        faults.iter().map(|fault| format!("{}: {}", fault.file, fault.key)).collect()
    }

    #[test]
    fn a_roster_that_allows_a_role_a_harness_it_holds_no_limits_for_while_it_holds_some_is_a_fault()
    {
        use Harness::{ClaudeCode, Pi};
        let workflow = landed("", "{ by: reviewer, do: rule }");
        let patrol = "by: dev\ndo: plan\nevery: 6h\nmax_open: 3\n";
        let roster = "roster:\n  dev: { run: [{ harness: claude-code }, { harness: pi }] }\n";
        let faults = |dev: &[Harness]| match limited(dev, &[], &workflow, patrol, roster) {
            Ok(_) => vec![],
            Err(faults) => faults,
        };
        // A role with no limits at all is right for every harness.
        assert_eq!(faults(&[]), []);
        assert_eq!(faults(&[ClaudeCode, Pi]), []);
        // One with limits for one harness is right for that one alone.
        let faults = faults(&[ClaudeCode]);
        assert_eq!(places(&faults), ["team.md: roster.dev.run[1].harness"]);
        assert!(
            faults[0].problem.starts_with(
                "the role `dev` may run `pi`, and holds limits for `claude-code` (in its \
                 `.claude/settings.json`) and none for `pi`: "
            ),
            "{}",
            faults[0].problem
        );
        assert!(faults[0].problem.contains("in its `.pi/extensions`"), "{}", faults[0].problem);
        // What the role's range on a host gives it is `run`'s to refuse.
        let team = limited(&[ClaudeCode], &[], &workflow, patrol, "").unwrap();
        assert_eq!(unlimited(team.role("dev").unwrap(), ClaudeCode), None);
        assert!(unlimited(team.role("dev").unwrap(), Pi).is_some());
    }

    #[test]
    fn a_skill_a_role_does_not_hold_where_a_harness_it_may_run_finds_its_own_is_noted() {
        use Harness::{ClaudeCode, Codex};
        let workflow = landed("", "{ by: reviewer, do: rule }");
        let patrol = "by: dev\ndo: plan\nevery: 6h\nmax_open: 3\n";
        let notes = |own: &[(Harness, &[&str])], roster: &str, kit: &[Harness]| {
            let team = format!("{TEAM}{roster}");
            let mut sources = sources(&team, &[("workflows/feature.md", &workflow)]);
            sources.patrols = vec![file("patrols/sweep.md", patrol)];
            let own = own.iter().map(|(harness, skills)| {
                (*harness, skills.iter().map(|skill| skill.to_string()).collect())
            });
            sources.roles[DEV].1.as_mut().unwrap().own_skills = own.collect();
            let held = load(&sources).unwrap().held(kit);
            assert_eq!(held.faults, []);
            held.notes
        };
        // A role that holds no such directory: the fleet has Codex find its
        // skills, and there is nothing to say.
        assert_eq!(notes(&[], "", &Harness::ALL), [] as [String; 0]);
        assert_eq!(notes(&[(Codex, &["plan", "implement"])], "", &[Codex]), [] as [String; 0]);
        // One that holds one without the skill: every worker of it that may
        // run Codex is noted — by its roster entry, or the host kit's.
        let roster = "roster:\n  dev: { run: [{ harness: codex }] }\n";
        let noted = notes(&[(Codex, &[])], roster, &[ClaudeCode]);
        assert_eq!(noted.len(), 3, "{noted:?}");
        assert!(
            noted[0].starts_with(
                "the stage `a` of the workflow `feature` runs on `codex`, and its role `dev` \
                 holds skills of its own for Codex (`.agents/skills/` or `.codex/skills/`), \
                 `plan` not among them: "
            ),
            "{}",
            noted[0]
        );
        assert!(noted[2].starts_with("the patrol `sweep` runs on `codex`"), "{}", noted[2]);
        assert_eq!(notes(&[(Codex, &[])], "", &[ClaudeCode]), [] as [String; 0]);
        assert_eq!(notes(&[(Codex, &["implement"])], "", &[Codex]).len(), 3);
    }

    #[test]
    fn a_skill_s_file_holds_its_directory_s_name_and_a_description() {
        let why =
            |yaml: &str| malformed("plan", &serde_yaml::from_str(yaml).map_err(|_| "no".into()));
        assert_eq!(why("name: plan\ndescription: Plan it.\nallowed-tools: Read\n"), None);
        for (yaml, said) in [
            ("description: Plan it.\n", "says no `name`"),
            (
                "name: draft\ndescription: Plan it.\n",
                "says its `name` is `draft`, which its directory's is not",
            ),
            ("name: 7\ndescription: Plan it.\n", "says its `name` is a number"),
            ("name: plan\n", "says no `description`"),
            ("name: plan\ndescription: ' '\n", "says no `description`"),
            ("name: plan\ndescription: [a]\n", "says its `description` is a list"),
            ("- plan\n", "opens with frontmatter that is a list"),
        ] {
            assert_eq!(why(yaml).as_deref(), Some(said), "{yaml}");
        }
        assert_eq!(malformed("plan", &Err("is not there".into())).as_deref(), Some("is not there"));
    }

    #[test]
    fn a_skill_whose_file_is_malformed_is_a_fault_of_each_do_that_names_it() {
        let workflow = landed("", "{ by: reviewer, do: rule }");
        let patrol = "by: dev\ndo: plan\nevery: 6h\nmax_open: 3\n";
        let mut sources = sources(TEAM, &[("workflows/feature.md", &workflow)]);
        sources.patrols = vec![file("patrols/sweep.md", patrol)];
        // One no `do` names is nobody's fault.
        let malformed = [("plan", "says no `name`"), ("unused", "says no `name`")];
        sources.roles[DEV].1.as_mut().unwrap().malformed =
            malformed.iter().map(|(skill, why)| (skill.to_string(), why.to_string())).collect();
        let faults = load(&sources).unwrap().held(&[Harness::ClaudeCode]).faults;
        let stages = ["workflows/feature.md: stages[0].do", "workflows/feature.md: stages[1].do"];
        assert_eq!(places(&faults), [stages[0], stages[1], "patrols/sweep.md: do"]);
        assert_eq!(
            faults[2].problem,
            "`plan`, the skill the patrol `sweep` has the role `dev` perform, has a file no \
             harness is sure to show a worker: acme/dev/.claude/skills/plan/SKILL.md says no \
             `name`. A skill's file opens with frontmatter that holds `name`, the name of its \
             directory, and `description`: `afkconvoy help skill-file` says its form"
        );
        let many: Vec<String> = (0..10).map(|n| format!("p/m{n}")).collect();
        assert_eq!(some(&many), "p/m0, p/m1, p/m2, p/m3, p/m4, p/m5, p/m6, p/m7 and 2 more");
    }

    #[test]
    fn a_file_a_harness_would_load_in_place_of_a_roles_rules_is_a_fault_naming_it() {
        let mut sources = sources(TEAM, &[]);
        let read = sources.roles[DEV].1.as_mut().unwrap();
        read.in_place = vec![
            (Harness::Pi, "AGENTS.override.md".to_string()),
            (Harness::Pi, "AGENTS.md".to_string()),
            (Harness::Pi, "AGENTS.MD".to_string()),
            (Harness::Codex, "AGENTS.override.md".to_string()),
            (Harness::Codex, "AGENTS.md".to_string()),
        ];
        let faults = load(&sources).unwrap_err();
        // One fault a file, naming each harness that would load it.
        assert_eq!(places(&faults), ["team.md: roles.dev"; 3]);
        assert!(
            faults[1].problem.starts_with(
                "acme/dev holds `AGENTS.md`: pi and Codex load that in place of `CLAUDE.md`, the \
                 rules every harness works under."
            ),
            "{}",
            faults[1].problem
        );
        assert!(faults[0].problem.starts_with("acme/dev holds `AGENTS.override.md`: pi and "));
        assert!(faults[2].problem.starts_with("acme/dev holds `AGENTS.MD`: pi loads that in "));
    }

    #[test]
    fn a_stage_without_a_named_consult_escalates_for_the_watch() {
        let usual = "{ by: dev, do: land }";
        let consult = |rest: &str| {
            one(&landed(rest, usual)).workflow("feature").unwrap().stages()[1].consult.clone()
        };
        assert_eq!(consult(""), None);
        assert_eq!(consult("    escalate: { to: human }\n"), None);
        assert!(matches!(consult("    escalate: { to: dev, do: plan }\n"), Some(Consult::Role(_))));
        // A human performs no skill.
        let (key, problem) = fault(&landed("    escalate: { to: human, do: plan }\n", usual));
        assert_eq!(
            (key.as_str(), problem.as_str()),
            ("stages[1].escalate.do", "`to: human` performs no skill")
        );
    }

    #[test]
    fn a_harness_is_no_key_of_a_consult_a_landing_or_a_workflow() {
        let usual = "{ by: dev, do: land }";
        let consulted = "    escalate: { to: reviewer, do: rule, harness: claude-code }\n";
        assert_eq!(fault(&landed(consulted, usual)).0, "stages[1].escalate.harness");
        let land = "{ by: dev, do: land, harness: claude-code }";
        assert_eq!(fault(&landed("", land)).0, "land.harness");
        assert_eq!(fault(&format!("harness: claude-code\n{FEATURE}")).0, "harness");
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
            "stages[0].escalate.do"
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
        let sources = Sources {
            at: Some(at),
            team,
            rules: false,
            workflows: vec![],
            patrols: vec![],
            roles,
            labels: RepoLabels::default(),
        };
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
            "must be a role's directory, as owner/repo, owner/repo/path or ./path, or a map of \
its `dir` and `host_user` and optional `dsh_profile`, not a number"
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
        let sources = Sources {
            at: None,
            team,
            rules: false,
            workflows: vec![],
            patrols: vec![],
            roles: vec![],
            labels: RepoLabels::default(),
        };
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
                rules: false,
                workflows: vec![],
                patrols: vec![],
                roles,
                labels: RepoLabels::default(),
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
        let faults = load(&Sources {
            at,
            team,
            rules: false,
            workflows: vec![],
            patrols: vec![],
            roles,
            labels: RepoLabels::default(),
        })
        .unwrap_err();
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
