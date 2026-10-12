//! A team as its files define it, loaded into a value that cannot hold a
//! malformed workflow or patrol (docs/adr/0008, docs/adr/0010). `load` is the only way
//! to a `Team`: it yields one, or every fault with the file and key it is in.
//! Whoever holds a `Team` checks nothing again.

use flux_rs::attrs::*;

use crate::config::{ESCALATE_LABEL, is_repo};

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

/// The keys of each map a definition holds: what `check`'s help must name.
pub const TEAM_KEYS: [&str; 3] = ["escalate_label", "roles", PRECEDENTS_KEY];
pub const PRECEDENTS_KEYS: [&str; 3] = ["by", "do", "max_open"];
pub const WORKFLOW_KEYS: [&str; 4] = ["max_rework", "max_consecutive_rework", "stages", LAND];
pub const STAGE_KEYS: [&str; 7] = ["name", "by", "do", "model", "writes", "escalate", "on"];
pub const ON_KEYS: [&str; 2] = ["pass", "reject"];
pub const ESCALATE_KEYS: [&str; 2] = ["to", "do"];
pub const LAND_KEYS: [&str; 2] = ["by", "do"];
pub const PATROL_KEYS: [&str; 5] = ["by", "do", "every", "model", "max_open"];

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
    /// Its yaml, or why it has none that can be read.
    pub yaml: Result<Node, String>,
}

/// What was read of a role's repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleRepo {
    /// The commit it was read at.
    pub commit: String,
    /// The entries of its skills directory.
    pub skills: Vec<String>,
}

/// Everything read of a team, and nothing yet checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    pub team: File,
    /// Every entry of the workflows directory.
    pub workflows: Vec<File>,
    /// Every entry of the patrols directory.
    pub patrols: Vec<File>,
    /// Each repo `role_repos` names, with what was read of it or why it could
    /// not be read.
    pub repos: Vec<(String, Result<RoleRepo, String>)>,
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

/// A loaded team.
#[derive(Debug, PartialEq, Eq)]
pub struct Team {
    /// Swapped in for the flow label when the fleet hands an issue to a human.
    pub escalate_label: String,
    pub roles: Vec<Role>,
    pub workflows: Vec<Workflow>,
    pub patrols: Vec<Patrol>,
    /// Who proposes a human's ruling as a precedent. `None`: the team names
    /// nobody, and none is proposed.
    pub precedents: Option<Precedents>,
}

impl Team {
    pub fn role(&self, name: &str) -> Option<&Role> {
        self.roles.iter().find(|role| role.name == name)
    }

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
}

/// One role of a team, as its repo stood when the team was loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Role {
    /// What the team's workflows call it.
    pub name: String,
    /// Its repo, as `owner/repo`.
    pub repo: String,
    /// The head of the repo's default branch when it was read.
    pub commit: String,
    pub skills: Vec<String>,
}

/// One skill of one role: what a worker is started to perform. Both exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Work {
    pub role: String,
    pub skill: String,
}

/// One patrol: a skill of a role, run on a schedule and for no issue. Its
/// name is one a ref and a worktree can carry (`patrol_name`), and its
/// interval is longer than nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patrol {
    /// The name of its file: what a repo's `patrols` names it by.
    pub name: String,
    pub work: Work,
    /// How long after one run the next is due, in seconds.
    pub every: u64,
    pub model: Option<String>,
    /// How many issues it opened may be open before it is skipped.
    pub max_open: u64,
}

/// Who proposes a human's ruling as a precedent — a skill of a role, started
/// once for the ruling — and how many proposals may wait for a human at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Precedents {
    pub work: Work,
    /// How many proposals may be open on the team's repo before no other is
    /// started.
    pub max_open: u64,
}

/// Who performs a stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Performer {
    /// A decision waited for from a human. No worker is started.
    Human,
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

/// One stage of a workflow of `n` stages. Its place is below `n`, and so is
/// the place of the stage its `pass` leads to, when it leads to one. Its
/// `reject`, if it may reject, leads to a stage before it: every way back
/// through a workflow is a counted rejection.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(at: int, n: int)]
#[invariant(at < n)]
pub struct Stage {
    pub name: String,
    pub by: Performer,
    pub model: Option<String>,
    /// Whether it may commit. A stage that reviews does not.
    pub writes: bool,
    /// Who rules on its `escalate` verdict before a human is asked.
    pub consult: Option<Work>,
    #[field(usize[at])]
    at: usize,
    /// How many stages its workflow has.
    #[field(usize[n])]
    of: usize,
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
    #[spec(fn(&Stage[@s]) -> Option<usize{v: v < s.at && v < s.n}>)]
    pub fn reject(&self) -> Option<usize> {
        self.reject
    }
}

/// One workflow. It has a first stage; following `pass` from any of its stages
/// comes to the landing train or to nothing more to do; every stage can be
/// reached from the first; and `land` is set exactly when a `pass` leads there.
/// It has `n` stages, each a stage of a workflow of `n`: where one's `pass` or
/// `reject` leads, there is a stage.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(n: int)]
#[invariant(n > 0)]
pub struct Workflow {
    /// What follows `flow:` in its flow label.
    pub name: String,
    pub max_rework: u64,
    pub max_consecutive_rework: u64,
    #[field(Vec<Stage{v: v.n == n}>[n])]
    stages: Vec<Stage>,
    /// Who gates the PR and joins the landing train.
    pub land: Option<Work>,
}

impl Workflow {
    pub fn flow_label(&self) -> String {
        format!("{FLOW_LABEL}{}", self.name)
    }

    /// The stage an issue entering the workflow is at. There is one, by the
    /// workflow's invariant.
    #[spec(fn(&Workflow[@w]) -> &Stage{v: v.n == w.n})]
    pub fn first(&self) -> &Stage {
        &self.stages[0]
    }

    /// The stage at the place `at`: where a `pass` or a `reject` of one of
    /// its stages leads.
    #[spec(fn(&Workflow[@w], at: usize{at < w.n}) -> &Stage{v: v.n == w.n})]
    pub fn stage(&self, at: usize) -> &Stage {
        &self.stages[at]
    }

    #[spec(fn(&Workflow[@w]) -> &[Stage{v: v.n == w.n}][w.n])]
    pub fn stages(&self) -> &[Stage] {
        &self.stages
    }
}

/// The one place a `Workflow` is built: one with no stage never gets out, nor
/// one whose stages were made for a workflow of another number of them.
#[spec(fn(String, u64, u64, Vec<Stage{v: v.n == n}>, n: usize, Option<Work>) -> Option<Workflow[n]>)]
fn workflow(
    name: String,
    max_rework: u64,
    max_consecutive_rework: u64,
    stages: Vec<Stage>,
    n: usize,
    land: Option<Work>,
) -> Option<Workflow> {
    if n == 0 || stages.len() != n {
        return None;
    }
    Some(Workflow { name, max_rework, max_consecutive_rework, stages, land })
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

/// The repos a team file names as its roles: what to read before `load`. It
/// reports nothing — `load` says what is wrong with the file.
pub fn role_repos(team: &File) -> Vec<String> {
    let mut repos: Vec<String> = vec![];
    let Ok(Node::Map(entries)) = &team.yaml else { return repos };
    if let Some(Node::Map(roles)) = get(entries, "roles") {
        for (_, repo) in roles {
            if let Node::Text(repo) = repo
                && is_repo(repo)
                && !repos.contains(repo)
            {
                repos.push(repo.clone());
            }
        }
    }
    repos
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

/// The roles a team file names.
struct Roles<'n> {
    /// Whether the team file could be read at all. Where it could not, a role
    /// a workflow names is neither confirmed nor reported.
    known: bool,
    /// Every name under `roles`.
    names: Vec<&'n str>,
    /// The ones whose repo was read.
    loaded: Vec<Role>,
}

/// Reads the team file → its escalate label and its roles.
fn team_file<'n>(faults: &mut Vec<Fault>, sources: &'n Sources) -> (String, Roles<'n>) {
    let mut report = Report { file: &sources.team.path, faults };
    let mut roles = Roles { known: false, names: vec![], loaded: vec![] };
    let mut escalate_label = ESCALATE_LABEL.to_string();
    let node = match &sources.team.yaml {
        Ok(node) => node,
        Err(why) => {
            report.fault("", why.clone());
            return (escalate_label, roles);
        }
    };
    let Some(entries) = map(&mut report, "", node, &TEAM_KEYS) else {
        return (escalate_label, roles);
    };
    roles.known = true;
    if let Some(label) = text(&mut report, "", entries, "escalate_label") {
        escalate_label = label.to_string();
    }
    let named = match get(entries, "roles") {
        None => return (escalate_label, roles),
        Some(Node::Map(named)) => named,
        Some(other) => {
            let problem = format!("must be a map of role names to repos, not {}", kind(other));
            report.fault("roles", problem);
            return (escalate_label, roles);
        }
    };
    for (name, repo) in named {
        let key = under("roles", name);
        if roles.names.contains(&name.as_str()) {
            report.fault(&key, "written twice".to_string());
            continue;
        }
        roles.names.push(name);
        if name == HUMAN {
            report
                .fault(&key, format!("a role cannot be named `{HUMAN}`: `by: {HUMAN}` is a human"));
            continue;
        }
        let repo = match repo {
            Node::Text(repo) if is_repo(repo) => repo,
            Node::Text(_) => {
                report.fault(&key, "must be a repo, as owner/repo".to_string());
                continue;
            }
            other => {
                report.fault(&key, format!("must be a repo, as owner/repo, not {}", kind(other)));
                continue;
            }
        };
        match sources.repos.iter().find(|(read, _)| read == repo) {
            Some((_, Ok(read))) => roles.loaded.push(Role {
                name: name.clone(),
                repo: repo.clone(),
                commit: read.commit.clone(),
                skills: read.skills.clone(),
            }),
            Some((_, Err(why))) => report.fault(&key, format!("{repo} cannot be read: {why}")),
            None => report.fault(&key, format!("{repo} was not read")),
        }
    }
    (escalate_label, roles)
}

/// Reads the team file's `precedents` → who proposes a ruling as one;
/// `Some(None)` where the file has no such key, `None` when the key is at
/// fault. What is wrong with the file itself is `team_file`'s to say.
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
/// team file already says why the role cannot be had.
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
    let loaded = roles.loaded.iter().find(|loaded| loaded.name == role)?;
    let skill = skill?;
    if !loaded.skills.iter().any(|has| has == skill) {
        let problem = format!(
            "`{skill}` is not a skill of the role `{role}`: {} has no {SKILLS}/{skill} at {}",
            loaded.repo, loaded.commit
        );
        report.fault(&under(at, skill_key), problem);
        return None;
    }
    Some(Work { role: role.to_string(), skill: skill.to_string() })
}

/// One stage as its file wrote it, its targets still names.
struct Draft<'n> {
    /// Its key: `stages[<place>]`.
    key: String,
    name: Option<&'n str>,
    by: Option<Performer>,
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
        for worker_key in ["do", "model", "escalate"] {
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
    Some(Draft { key, name, by, model, writes, consult, pass, reject })
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

/// Reads one entry of the workflows directory → the workflow it defines, when
/// nothing is wrong with it.
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
    let land = match get(entries, LAND) {
        None => Some(None),
        Some(node) => match map(report, LAND, node, &LAND_KEYS) {
            Some(entries) => work(report, roles, LAND, entries, ("by", "do")).map(Some),
            None => None,
        },
    };
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
        stages.push(stage(draft, at, n, among(edges.pass?, n)?, reject)?);
    }
    workflow(name.to_string(), max_rework?, max_consecutive_rework?, stages, n, land?)
}

/// Reads one entry of the patrols directory → the patrol it defines, when
/// nothing is wrong with it.
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
    let model = text(report, "", entries, "model").map(str::to_string);
    let max_open = limit(report, "", entries);
    Some(Patrol { name: name.to_string(), work: work?, every: every?, model, max_open: max_open? })
}

/// The stage a draft with nothing wrong in it is, at the place `at` of a
/// workflow of `n` stages: `None` when that is no place of such a workflow.
#[spec(fn(Draft, at: usize, n: usize, Pass{v: v.to_stage => v.to < n}, Option<usize{v: v < at}>)
    -> Option<Stage[at, n]>)]
fn stage(draft: Draft, at: usize, n: usize, pass: Pass, reject: Option<usize>) -> Option<Stage> {
    if at >= n {
        return None;
    }
    Some(Stage {
        name: draft.name?.to_string(),
        by: draft.by?,
        model: draft.model,
        writes: draft.writes?,
        consult: draft.consult?,
        at,
        of: n,
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
        Workflow { name, max_rework: 0, max_consecutive_rework: 0, stages: Vec::new(), land: None }
    }

    /// Whatever stages were listed, taken as a workflow without a look.
    #[should_fail]
    fn a_workflow_of_whatever_was_listed(name: String, stages: Vec<Stage>) -> Workflow {
        Workflow { name, max_rework: 0, max_consecutive_rework: 0, stages, land: None }
    }

    /// A `pass` past the last stage of a workflow.
    #[should_fail]
    #[spec(fn(String, n: usize{n > 0}) -> Stage[0, n])]
    fn a_pass_past_the_last_stage(name: String, n: usize) -> Stage {
        Stage {
            name,
            by: Performer::Human,
            model: None,
            writes: false,
            consult: None,
            at: 0,
            of: n,
            pass: Pass::Stage(n),
            reject: None,
        }
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
        File { path: path.to_string(), yaml: serde_yaml::from_str(yaml).map_err(|e| e.to_string()) }
    }

    fn repo(name: &str, skills: &[&str]) -> (String, Result<RoleRepo, String>) {
        let skills = skills.iter().map(|skill| skill.to_string()).collect();
        (name.to_string(), Ok(RoleRepo { commit: format!("{name}@1"), skills }))
    }

    fn sources(team: &str, workflows: &[(&str, &str)]) -> Sources {
        Sources {
            team: file(TEAM_FILE, team),
            workflows: workflows.iter().map(|(path, yaml)| file(path, yaml)).collect(),
            patrols: vec![],
            repos: vec![
                repo("acme/dev", &["plan", "implement", "land"]),
                repo("acme/reviewer", &["review", "rule"]),
            ],
        }
    }

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
        let work = Work { role: "dev".to_string(), skill: "plan".to_string() };
        assert_eq!(
            team.patrol("sweep"),
            Some(&Patrol {
                name: "sweep".to_string(),
                work,
                every: 6 * 3600,
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
        let work = Work { role: "reviewer".to_string(), skill: "rule".to_string() };
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
        assert_eq!((dev.repo.as_str(), dev.commit.as_str()), ("acme/dev", "acme/dev@1"));
        assert_eq!(dev.skills, ["plan", "implement", "land"]);
        let flow = team.workflow("feature").unwrap();
        assert_eq!(flow.flow_label(), "flow:feature");
        assert_eq!(flow.first().name, "plan");
        let work = |role: &str, skill: &str| Work { role: role.into(), skill: skill.into() };
        assert_eq!(flow.land, Some(work("dev", "land")));
        let [plan, build, review, sign_off] = flow.stages() else { panic!("four stages") };
        assert_eq!((plan.at(), plan.pass(), plan.reject()), (0, Pass::Stage(1), None));
        assert_eq!(build.by, Performer::Role(work("dev", "implement")));
        assert_eq!(build.model.as_deref(), Some("opus"));
        assert_eq!(build.consult, Some(work("reviewer", "rule")));
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
        sources.repos.push(("acme/ops".to_string(), Err("no such repository".to_string())));
        let found = load(&sources).unwrap_err();
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(
            found[0].to_string(),
            "team.md: roles.ops: acme/ops cannot be read: no such repository"
        );
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
    fn the_repos_to_read_are_the_ones_the_team_file_names() {
        let team = file(
            TEAM_FILE,
            "roles: { dev: acme/dev, qa: acme/dev, ops: nowhere, n: 3, r: acme/r }",
        );
        assert_eq!(role_repos(&team), ["acme/dev", "acme/r"]);
        assert!(role_repos(&file(TEAM_FILE, "roles: [a/b]")).is_empty());
        assert!(role_repos(&file(TEAM_FILE, "roles: {")).is_empty());
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
