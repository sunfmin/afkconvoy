//! A cut (docs/adr/0028): the run that makes a new version of a repo from
//! its base branch — it commits the version on a branch of its own from the
//! base tip, gates that commit, tags it, and publishes and installs it. The
//! base branch never receives the version commit, and no landing turn waits
//! on a cut. An idle tick, or one past the landed work's age bound, starts one in a repo that names its
//! cut steps, once the base branch holds landed commits past the newest
//! version tag, or the repo declares its initial version on an untagged
//! remote; it is claimed on the remote as a run is, so one host cuts each
//! version. Where a cut stands, which version it cuts, what it is given and
//! what is left of it are decided here; running its steps, and starting and
//! ending it, are the effect layer's.

use serde::{Deserialize, Serialize};

use crate::claims::{Holder, InstanceId, RunClaim};
use crate::plan::Issue;
use crate::record::{self, canonical_number};

/// What the claim on a cut is keyed by: one cut of a repo runs at a time,
/// across its hosts.
pub const CLAIM_KEY: &str = "cut";

/// The name a cut's checkout is created under: one a repo, for one cut of it
/// runs at a time.
pub const CHECKOUT: &str = "afkconvoy-cut";

/// The line that opens the fenced block of a repo's config file its cut
/// steps are written in. It is not the config's own block, whose opening line
/// is ```` ```yaml ```` alone: an afkconvoy that reads only that block reads
/// past this one, and refuses nothing for it.
pub const BLOCK: &str = "```yaml cut";

/// The label a PR that landed since the newest version tag carries for the
/// cut to raise the minor version, not the patch.
pub const MINOR_LABEL: &str = "release:minor";

/// The label on an issue whose stage waits on the next cut of its repo: it
/// needs a version that is not installed yet (docs/adr/0056). The fleet puts
/// it on as it parks the claim, which keeps the issue off the frontier, and
/// takes it off every issue once a cut has published and installed its
/// version — nothing else does.
pub const WAITING_LABEL: &str = "convoy-waits-on-cut";

/// The label on the issue a failed cut opens: the host's watch answers it,
/// as it answers anything that holds a fleet back. A watch that finds the
/// failure one of the four kinds left to a human swaps it for the escalate
/// label, and only then is the issue a human's (sunfmin/control#801).
pub const FAILED_LABEL: &str = "convoy-cut-failed";

/// Whether a failed cut's issue carrying `labels` is the watch's to answer.
pub fn watched(labels: &[String]) -> bool {
    labels.iter().any(|label| label == FAILED_LABEL)
}

/// Why an issue that waits on the next cut is off the frontier.
pub const WAITS: &str = "waits on the next cut";

/// Whether an issue carrying `labels` waits on the next cut.
pub fn waits(labels: &[String]) -> bool {
    labels.iter().any(|label| label == WAITING_LABEL)
}

/// The variable a repo's cut steps are given the version in, `0.5.17`.
pub const VERSION_ENV: &str = "VERSION";

/// The word of the marker of the issue a failed cut opens.
pub const FAILED: &str = "convoy:cut-failed";

/// What a cut is given beside two gate limits — its own gate and the queue
/// it waits in for a worker lane — for the rest of its steps, publishing and
/// installing among them, in seconds: three hours.
pub const BESIDE_THE_GATES: u64 = 3 * 3600;

/// The default age of the oldest landing past the newest tag at which a
/// busy fleet starts a cut, in seconds: five hours where the repo omits
/// `busy_after_seconds`.
pub const BUSY_AFTER_SECONDS: u64 = 18_000;

fn busy_after_seconds() -> u64 {
    BUSY_AFTER_SECONDS
}

/// How long, in seconds, a cut is given in a repo whose gate limit is
/// `gate_seconds`: its own gate and the wait for its worker lane, each to the
/// limit, and `BESIDE_THE_GATES` for the rest. One still going then is ended,
/// and has failed.
pub fn time_limit(gate_seconds: u64) -> u64 {
    gate_seconds.saturating_mul(2).saturating_add(BESIDE_THE_GATES)
}

/// How long a cut's gate waits, in seconds, before it runs again after a
/// run red only on TIMEOUTs it said it cannot classify on an overloaded host
/// (`gate::overloaded_timeouts`): five minutes, its worker lane given back.
pub const GATE_RETRY_SECONDS: u64 = 300;

/// Whether a cut started at `started`, given `limit` seconds, can at `now`
/// wait `pause` and run its gate again — to its limit, `gate_seconds` —
/// before its time runs out. Where it cannot, the red run fails the step.
pub fn gate_again(started: u64, now: u64, pause: u64, gate_seconds: u64, limit: u64) -> bool {
    now.saturating_add(pause).saturating_add(gate_seconds) < started.saturating_add(limit)
}

/// A repo's cut steps: the lines that set the version, and the lines that
/// publish and install it, each run by `sh -e` in the cut's checkout with the
/// version in `VERSION_ENV`: the first line that fails fails the step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Steps {
    /// How long landed work may wait before a busy tick starts a cut.
    /// Zero starts one at the next tick, whatever the fleet holds.
    #[serde(default = "busy_after_seconds")]
    pub busy_after_seconds: u64,
    /// The version of the first cut where the remote has no version tag.
    /// Left out, an untagged repo is not cut; after a tag, this is ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial: Option<String>,
    /// Run on the base branch as it stood, the cut's snapshot: what it changes
    /// is the version's commit, on the cut's own branch, never the base's.
    pub set: String,
    /// Run with the version's tag checked out, once it is pushed.
    pub publish: String,
}

impl Steps {
    /// The steps, each with something to run and a valid initial version
    /// where declared → them, or the key whose value is invalid.
    pub fn checked(self) -> Result<Steps, &'static str> {
        if self.initial.is_some() && self.initial_version().is_none() {
            return Err("initial");
        }
        if self.set.trim().is_empty() {
            return Err("set");
        }
        if self.publish.trim().is_empty() {
            return Err("publish");
        }
        Ok(self)
    }

    /// The declared initial version, while it is three version numbers.
    pub fn initial_version(&self) -> Option<Version> {
        Version::parse(self.initial.as_deref()?)
    }
}

/// A version, as a version tag names it: `v<major>.<minor>.<patch>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// The version the tag `name` names: `v`, then three numbers as the fleet
    /// writes them (`record::canonical_number`), parted by `.`. Any other tag
    /// names none — `v1.2`, `v1.2.3-rc1`, `release-1`.
    pub fn tagged(name: &str) -> Option<Version> {
        Version::parse(name.strip_prefix('v')?)
    }

    /// Three version numbers without a tag's `v`, each as the fleet writes it.
    pub fn parse(name: &str) -> Option<Version> {
        let mut parts = name.split('.');
        let major = canonical_number(parts.next()?)?;
        let minor = canonical_number(parts.next()?)?;
        let patch = canonical_number(parts.next()?)?;
        match parts.next() {
            Some(_) => None,
            None => Some(Version { major, minor, patch }),
        }
    }

    /// The version a cut makes after this one: the next minor where `minor`,
    /// its patch 0, else the next patch. `None` past the largest number.
    pub fn next(self, minor: bool) -> Option<Version> {
        match minor {
            true => Some(Version { minor: self.minor.checked_add(1)?, patch: 0, ..self }),
            false => Some(Version { patch: self.patch.checked_add(1)?, ..self }),
        }
    }

    /// Its tag: `v0.5.17`.
    pub fn tag(self) -> String {
        format!("v{self}")
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A version tag on the remote: the version it names, and the commit it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub version: Version,
    pub commit: String,
}

/// The newest of `tags` — each a tag's name and its commit — by the version
/// it names; a tag that names none is not one.
pub fn newest(tags: &[(String, String)]) -> Option<Tag> {
    let mut newest: Option<Tag> = None;
    for (name, commit) in tags {
        let Some(version) = Version::tagged(name) else { continue };
        if newest.as_ref().is_none_or(|newest| version > newest.version) {
            newest = Some(Tag { version, commit: commit.clone() });
        }
    }
    newest
}

/// The PRs the subjects of the base branch's commits past a tag name, as a
/// landing names one — `<title> (#<pr>)` — lowest first, each once: the PRs a
/// cut past that tag holds.
pub fn prs_of<'a>(subjects: impl IntoIterator<Item = &'a str>) -> Vec<u64> {
    let mut prs: Vec<u64> = subjects
        .into_iter()
        .filter_map(|subject| {
            let (_, number) = subject.trim_end().strip_suffix(')')?.rsplit_once(" (#")?;
            canonical_number(number)
        })
        .collect();
    prs.sort_unstable();
    prs.dedup();
    prs
}

/// What a cut past `newest`, holding `prs`, makes: the next minor where one
/// of them is among `minor` — those carrying `MINOR_LABEL` — else the next
/// patch.
pub fn pick(newest: Version, prs: &[u64], minor: &[u64]) -> Option<Version> {
    newest.next(prs.iter().any(|pr| minor.contains(pr)))
}

/// What the effect layer read of a repo's cut.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Seen {
    /// The repo names its cut steps: it turns cutting on.
    pub on: bool,
    /// The newest version tag on the remote, while it has one.
    pub newest: Option<Tag>,
    /// The initial version the repo declares, used only without a version tag.
    pub initial: Option<Version>,
    /// How many commits the base branch holds past it.
    pub past: u64,
    /// When the oldest first-parent commit past the tag landed. An initial
    /// cut has no tag and remains idle-only.
    pub oldest_landing: Option<u64>,
    pub busy_after_seconds: u64,
    /// The claim on a cut, while one stands.
    pub claim: Option<RunClaim>,
    /// The open issue that says a cut failed, while one is open.
    pub failure: Option<u64>,
}

/// How a repo's cut stands: what a tick does about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    /// The repo names no cut steps: it turns cutting off, and nothing is cut.
    Off,
    /// A cut failed, and the issue that says so is still open.
    Failed,
    /// The remote has no version tag, and the repo declares no initial version.
    Untagged,
    /// The base branch holds no commit past the newest version tag.
    Tagged,
    /// A fleet lock stands: no cut starts, for the base it would snapshot may
    /// be red.
    Locked,
    /// A tick that ends idle starts one.
    Due,
    /// A cut of mine is still going: a tick asks after it.
    Running,
    /// A live peer's cut is still going.
    PeerRunning,
    /// The claim on a cut is a peer's presumed dead: a tick deletes it.
    StaleRun,
}

impl Standing {
    pub fn word(self) -> &'static str {
        match self {
            Standing::Off => "off",
            Standing::Failed => "failed",
            Standing::Untagged => "untagged",
            Standing::Tagged => "tagged",
            Standing::Locked => "locked",
            Standing::Due => "due",
            Standing::Running => "running",
            Standing::PeerRunning => "peer_running",
            Standing::StaleRun => "stale_run",
        }
    }
}

/// A repo's cut, as a working set holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub standing: Standing,
    /// The failed host checks while this cut is due: the one evaluation a
    /// tick says and starts the cut by (sunfmin/control#532).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_cutting_here: Option<Held>,
    /// The newest version tag, and how many commits the base branch holds
    /// past it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest_tag: Option<String>,
    /// The declared initial version, while the remote has no version tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial: Option<String>,
    pub past: u64,
    pub oldest_landing_age_seconds: Option<u64>,
    /// How many seconds remain before a busy tick may start it; zero once due.
    pub busy_in_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<u64>,
    /// The open issues that wait on the next cut (`WAITING_LABEL`), lowest
    /// first: a cut that installs its version puts them back on the frontier.
    pub waiting: Vec<u64>,
    /// What the open issue `failure` says of the cut that failed, and how to
    /// start the cut again from this host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<Failed>,
    /// The claim on a cut, while one stands.
    #[serde(skip)]
    pub claim: Option<RunClaim>,
}

/// A due cut this host does not claim: each cut need that failed at the
/// look, in declared order, and the user-only host-cache file of their last
/// stderr lines — one per checkout, named, never read, so its output stays
/// on the host (docs/adr/0053). The file stands until a look finds no need
/// failing, whichever need fails meanwhile (sunfmin/control#570). Plan and
/// the tick say it as `said`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub needs: Vec<NeedFailed>,
    pub stderr: String,
}

/// One cut need that failed on this host: the check's name, why it failed,
/// and what an install that ran for it, or is not tried again yet, did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedFailed {
    pub name: String,
    pub reason: String,
    pub install: Option<String>,
}

impl Held {
    /// `not cutting here: <name>, <name> failed`.
    pub fn said(&self) -> String {
        let names: Vec<&str> = self.needs.iter().map(|need| need.name.as_str()).collect();
        format!("not cutting here: {} failed", names.join(", "))
    }

    /// Each need's status and any install, and the stderr file, as doctor
    /// says them.
    pub fn evidence(&self) -> String {
        let needs: Vec<String> = self
            .needs
            .iter()
            .map(|need| {
                format!(
                    "{} failed ({}){}",
                    need.name,
                    need.reason,
                    need.install.as_deref().unwrap_or_default()
                )
            })
            .collect();
        format!(
            "the cut is due and host checks failed: {}; stderr: {}",
            needs.join("; "),
            self.stderr
        )
    }
}

impl Serialize for Held {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.said())
    }
}

/// How the cut `seen` stands for the fleet instance `me` at `now`, with
/// each instance's heartbeat in `heartbeats`, and `locked` while a fleet lock
/// stands. A cut still going comes first, whoever holds it and whatever the
/// rest says; then a repo that turns cutting off, the failure nobody
/// answered, the tags, and the lock.
pub fn row(
    seen: &Seen,
    me: Option<&InstanceId>,
    heartbeats: &[(String, u64)],
    now: u64,
    locked: bool,
) -> Row {
    let standing = match &seen.claim {
        Some(claim) => match claim.holder(me, heartbeats, now) {
            Holder::Mine => Standing::Running,
            Holder::Dead => Standing::StaleRun,
            Holder::Peer => Standing::PeerRunning,
        },
        None if !seen.on => Standing::Off,
        None if seen.failure.is_some() => Standing::Failed,
        None if seen.newest.is_none() && seen.initial.is_none() => Standing::Untagged,
        None if seen.newest.is_some() && seen.past == 0 => Standing::Tagged,
        None if locked => Standing::Locked,
        None => Standing::Due,
    };
    Row {
        standing,
        not_cutting_here: None,
        newest_tag: seen.newest.as_ref().map(|tag| tag.version.tag()),
        initial: seen.initial.filter(|_| seen.newest.is_none()).map(|version| version.to_string()),
        past: seen.past,
        oldest_landing_age_seconds: seen.oldest_landing.map(|landed| now.saturating_sub(landed)),
        busy_in_seconds: seen
            .oldest_landing
            .map(|landed| landed.saturating_add(seen.busy_after_seconds).saturating_sub(now)),
        failure: seen.failure,
        waiting: vec![],
        failed: None,
        claim: seen.claim.clone(),
    }
}

/// Whether a tick that ends with `held` claims held, `frontier` issues left
/// on its frontier
/// starts the cut `row`: it is due and either the age bound has been reached,
/// or the tick ends idle with admission open. An overdue cut starts even
/// while admission holds workers back: its heavy steps use the exclusive queue.
pub fn starts(row: &Row, held: usize, frontier: usize, held_back: bool) -> bool {
    row.standing == Standing::Due
        && (row.busy_in_seconds == Some(0) || (held == 0 && frontier == 0 && !held_back))
}

/// One step of a cut, in the order it takes them.
///
/// A record an earlier afkconvoy kept names steps it no longer takes: `pr`
/// reads as `push`, and `land` as `gate`, so a cut it left goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    /// On the snapshot `S`, the base tip as it stood: run the repo's `set`
    /// lines, and commit what they changed — or take up the commit a cut of
    /// the same version already tagged or pushed.
    Set,
    /// Push that commit to the cut's own branch, `afkconvoy-cut/v<version>-<stamp>`.
    #[serde(alias = "pr")]
    Push,
    /// Gate that commit, the fast gate and the slow gate, in a worker lane: no
    /// landing turn, no candidate, no barrier.
    #[serde(alias = "land")]
    Gate,
    /// Tag that commit `v<version>`, and push the tag.
    Tag,
    /// With the tag checked out, run the repo's `publish` lines.
    Publish,
}

impl Step {
    pub const ALL: [Step; 5] = [Step::Set, Step::Push, Step::Gate, Step::Tag, Step::Publish];

    pub fn word(self) -> &'static str {
        match self {
            Step::Set => "set",
            Step::Push => "push",
            Step::Gate => "gate",
            Step::Tag => "tag",
            Step::Publish => "publish",
        }
    }

    /// The step `word` names.
    pub fn named(word: &str) -> Option<Step> {
        Step::ALL.into_iter().find(|step| step.word() == word)
    }

    /// The steps from this one on, in order.
    pub fn from(self) -> Vec<Step> {
        Step::ALL.into_iter().skip_while(|step| *step != self).collect()
    }
}

/// One step a cut took, and how many seconds it took.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Took {
    pub step: Step,
    pub seconds: u64,
}

/// A step a cut failed at, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub step: Step,
    pub why: String,
    /// Of a gate that ran red: what its run named and where it is kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub red: Option<RedGate>,
}

impl Failure {
    /// The step `step` failed, for `why`, with no gate run to name.
    pub fn at(step: Step, why: impl Into<String>) -> Failure {
        Failure { step, why: why.into(), red: None }
    }
}

/// What a cut's gate that ran red named: the tests its output names as
/// failing (`gate::failing_tests`), what it printed from the first of them
/// on (`gate::quoted`), and the file its whole output is kept in — a file
/// of that run's own, which no later run of the gate writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedGate {
    #[serde(default)]
    pub tests: Vec<String>,
    #[serde(default)]
    pub quote: Vec<String>,
    pub output: String,
}

/// What a cut is, and what it did so far: kept in its checkout as it goes,
/// the fleet's start writing it and each step adding to it. What is left of
/// a cut once it ends is read off it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The version it makes, `0.5.17`, and the tag it was cut past, `v0.5.16`.
    /// No tag means the declared initial version: its cut holds all landings.
    pub version: String,
    #[serde(default)]
    pub from: Option<String>,
    /// The PRs it holds: those that landed since that tag, or all landings
    /// for an initial cut, lowest first.
    #[serde(default)]
    pub prs: Vec<u64>,
    /// Whether it raises the minor version, for a PR carrying `MINOR_LABEL`.
    #[serde(default)]
    pub minor: bool,
    /// When the fleet started it, in epoch seconds.
    pub started: u64,
    /// Its own branch, once it pushed it, and the version's commit, once it
    /// made it: the commit it gates and tags, whose one parent is the
    /// snapshot. An earlier afkconvoy's record names the commit its turn
    /// landed, `landed`: that is the commit it tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, alias = "landed", skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The step it is taking now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Step>,
    /// Each step it took, in order.
    #[serde(default)]
    pub took: Vec<Took>,
    /// The step it failed at, and why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<Failure>,
    /// A minor landing overtook this patch before it tagged. It stops at
    /// `tag`, without failing or publishing; the next tick picks the minor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded: Option<String>,
}

impl Record {
    /// Whether it took every step: the version is published and installed.
    pub fn done(&self) -> bool {
        self.failed.is_none()
            && self.superseded.is_none()
            && self.took.last().is_some_and(|took| took.step == Step::Publish)
    }

    /// The step it is at, or was last: where a cut that ended short of its
    /// last step goes on from.
    pub fn step(&self) -> Step {
        if let Some(failed) = &self.failed {
            return failed.step;
        }
        if let Some(at) = self.at {
            return at;
        }
        let last = self.took.last().map(|took| took.step);
        let next = last.and_then(|last| last.from().into_iter().nth(1));
        next.unwrap_or(Step::Set)
    }

    /// What it is and did, in one line: what the log keeps.
    pub fn line(&self) -> String {
        let origin = match &self.from {
            Some(tag) => format!("past {tag}"),
            None => "the initial version".to_string(),
        };
        let mut said = format!("v{} ({origin}", self.version);
        if self.minor {
            said.push_str(&format!(", the next minor for a PR labelled {MINOR_LABEL}"));
        }
        if !self.prs.is_empty() {
            said.push_str(&format!(", holding {}", numbered(&self.prs)));
        }
        said.push(')');
        if let Some(commit) = &self.commit {
            said.push_str(&format!(" · its commit {}", short(commit)));
        }
        if let Some(branch) = &self.branch {
            said.push_str(&format!(" on {branch}"));
        }
        if !self.took.is_empty() {
            let took: Vec<String> = self
                .took
                .iter()
                .map(|took| format!("{} {}s", took.step.word(), took.seconds))
                .collect();
            said.push_str(&format!(" · {}", took.join(", ")));
        }
        said
    }
}

/// `numbers` as a sentence names them: `#451`, or `#451, #452`.
fn numbered(numbers: &[u64]) -> String {
    numbers.iter().map(|number| format!("#{number}")).collect::<Vec<_>>().join(", ")
}

/// A commit's first ten digits.
fn short(commit: &str) -> &str {
    commit.get(..10).unwrap_or(commit)
}

/// How a cut ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// It took every step.
    Cut,
    /// A minor landing overtook the patch: no tag, publication or failure.
    Superseded,
    /// It failed at a step, as it said.
    Failed(Failure),
    /// Its time ran out at a step: it was ended there.
    OutOfTime(Step),
    /// Its terminal is gone, and it said neither that it took every step nor
    /// where it failed: it was ended at a step from outside.
    Gone(Step),
}

impl Ending {
    /// The step a cut that ended short failed at, and why, in words; `None`
    /// of one that took every step.
    pub fn failure(&self, limit: u64) -> Option<Failure> {
        match self {
            Ending::Cut | Ending::Superseded => None,
            Ending::Failed(failure) => Some(failure.clone()),
            Ending::OutOfTime(step) => Some(Failure::at(
                *step,
                format!("its time ran out: a cut is given {} minutes", limit / 60),
            )),
            Ending::Gone(step) => Some(Failure::at(
                *step,
                "it was ended from outside: its terminal is gone, and it said nothing",
            )),
        }
    }
}

/// How the cut that `record` keeps ended at `now`, in a repo that gives a
/// cut `limit` seconds — its terminal `gone` or not; `None` while it still
/// runs. A record that cannot be read is a cut that said nothing, started
/// when its claim was (`started`).
pub fn ending(
    record: Option<&Record>,
    started: u64,
    gone: bool,
    now: u64,
    limit: u64,
) -> Option<Ending> {
    let step = record.map_or(Step::Set, Record::step);
    if let Some(record) = record {
        if record.superseded.is_some() {
            return Some(Ending::Superseded);
        }
        if record.done() {
            return Some(Ending::Cut);
        }
        if let Some(failed) = &record.failed {
            return Some(Ending::Failed(failed.clone()));
        }
    }
    let started = record.map_or(started, |record| record.started);
    if now.saturating_sub(started) >= limit {
        return Some(Ending::OutOfTime(step));
    }
    gone.then_some(Ending::Gone(step))
}

/// The title of the issue a failed cut of `version` opens.
pub fn failure_title(version: &str) -> String {
    format!("afkconvoy: the cut of v{version} failed")
}

/// Whether `title` is the title of the issue a failed cut opens, of any
/// version: how the report is found again.
fn is_failure_title(title: &str) -> bool {
    title.strip_prefix("afkconvoy: the cut of v").is_some_and(|rest| rest.ends_with(" failed"))
}

/// The open issue among `issues` that says a cut failed: one titled as a
/// failed cut's (`failure_title`) that carries `FAILED_LABEL` — the watch's
/// to answer — or `escalate_label` — a human's, or opened by an afkconvoy
/// before the label — the lowest numbered of them.
pub fn failure_of(issues: &[Issue], escalate_label: &str) -> Option<u64> {
    let mut found: Option<u64> = None;
    for issue in issues {
        let labelled = issue.labels.iter().any(|l| l == FAILED_LABEL || l == escalate_label);
        let reports = is_failure_title(&issue.title) && labelled;
        if reports && found.is_none_or(|first| issue.number < first) {
            found = Some(issue.number);
        }
    }
    found
}

/// The body of the issue a cut that failed opens: which step failed, why,
/// what it did before, and how it goes on, which `goes_on` is — the line
/// run in its checkout `checkout` on the host `host`, which is kept; where
/// its checkout is gone (`None`), how the cut starts again, in words.
pub fn failure_report(
    record: &Record,
    failure: &Failure,
    host: &str,
    checkout: Option<&str>,
    goes_on: &str,
) -> String {
    let step = failure.step.word();
    let marker = record::marker(
        FAILED,
        &[
            ("version", &record.version),
            ("step", step),
            ("host", host),
            ("checkout", checkout.unwrap_or("")),
        ],
    );
    let mut said = format!(
        "{marker}\n**afkconvoy** — the cut of v{} failed at its step `{step}`: {}\n\n",
        record.version,
        failure.why.trim()
    );
    said.push_str(&format!("What it was: {}.\n\n", record.line()));
    if let Some(red) = &failure.red {
        said.push_str(&red_gate(red));
    }
    match checkout {
        Some(checkout) => said.push_str(&format!(
            "Its checkout is kept, `{checkout}` on the host `{host}`, with what each step \
             printed in its git dir. The line that goes on from the step that failed, once what \
             failed it is fixed:\n\n```sh\n{goes_on}\n```\n\nIt takes that step and the ones \
             after it, and closes this issue once it took the last. No cut starts while this \
             issue is open: close it once the version is cut, or once you have looked and want \
             the fleet to cut again from the start.\n"
        )),
        None => said.push_str(&format!(
            "Its checkout is gone from the host `{host}`, so no `--from` line can go on with it. \
             To start the cut again, once what failed it is fixed: {goes_on}\n\nNo cut starts \
             while this issue is open.\n"
        )),
    }
    said.push_str(&format!(
        "\nThe host's watch answers this issue, labelled `{FAILED_LABEL}`. Where what failed it \
         is one of the four kinds left to a human, the watch swaps that label for the escalate \
         label.\n"
    ));
    if failure.step == Step::Publish {
        said.push_str(&format!(
            "\nIts tag is pushed: closed with no line run, v{} stays tagged and is never \
             published, and the next cut is of the version after it, once more lands.\n",
            record.version
        ));
    }
    said
}

/// What the report of a cut whose gate ran red says of that run: the tests
/// that failed, what it printed from the first of them on, and where its
/// whole output is kept.
fn red_gate(red: &RedGate) -> String {
    let mut said = match red.tests.as_slice() {
        [] => "Its gate's output names no failing test. It ended:\n\n".to_string(),
        tests => {
            let named: Vec<String> = tests.iter().map(|test| format!("`{test}`")).collect();
            format!(
                "The tests its gate failed on: {}. Its output from the first of them on:\n\n",
                named.join(", ")
            )
        }
    };
    said.push_str(&format!("````\n{}\n````\n\n", red.quote.join("\n")));
    said.push_str(&format!(
        "That run's whole output is kept in `{}`, a file of its own: a later \
         `afkconvoy cut --from gate` writes its run to another, and leaves this one.\n\n",
        red.output
    ));
    said
}

/// What the issue a failed cut opened says of it, read back off its body
/// (`failure_report`): its version, the step that failed and why, the host
/// and checkout it kept, and the line that goes on from that step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reported {
    pub version: String,
    pub step: String,
    pub why: String,
    pub host: Option<String>,
    /// Its kept checkout; `None` where the report says it is gone.
    pub checkout: Option<String>,
    pub goes_on: Option<String>,
}

/// `body`, the report of a failed cut → what it says; `None` of a body that
/// is no such report. A report written before its marker named the host and
/// the checkout says them in its words.
pub fn reported(body: &str) -> Option<Reported> {
    let fields = record::read_marker(FAILED, body)?;
    let version = fields.get("version")?.to_string();
    let step = fields.get("step")?.to_string();
    let why = body
        .lines()
        .find_map(|line| line.strip_prefix("**afkconvoy** — the cut of v")?.split_once("`: "))
        .map(|(_, why)| why.trim().to_string())
        .unwrap_or_default();
    let kept = body.split_once("Its checkout is kept, `").and_then(|(_, rest)| {
        let (checkout, rest) = rest.split_once("` on the host `")?;
        let (host, _) = rest.split_once('`')?;
        Some((checkout.to_string(), host.to_string()))
    });
    let (host, checkout) = match (fields.get("host"), kept) {
        (Some(host), _) => (Some(host.to_string()), fields.get("checkout").map(str::to_string)),
        (None, Some((checkout, host))) => (Some(host), Some(checkout)),
        (None, None) => (None, None),
    };
    let goes_on = checkout.as_ref().and_then(|_| {
        let (_, rest) = body.split_once("```sh\n")?;
        let (line, _) = rest.split_once("\n```")?;
        Some(line.trim().to_string())
    });
    Some(Reported { version, step, why, host, checkout, goes_on })
}

/// Where the checkout a failed cut kept stands, seen from one host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kept {
    /// On this host: its `--from` line goes on with it here.
    Here,
    /// On the host its report names, another: the line runs only there.
    Elsewhere,
    /// Gone: no `--from` line can go on with it.
    Gone,
}

/// The branch a failed cut pushed, and whether the next cut of its version
/// takes up its commit: only one on the base tip as it stands is
/// (`pushed`, and the cut's step `set`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Branch {
    pub name: String,
    pub commit: String,
    pub taken_up: bool,
}

/// A failed cut as a working set holds it: what its open issue says, where
/// its checkout stands from this host, the branch it pushed, and how the cut
/// starts again, in words with the lines that do it (`restart`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failed {
    pub issue: u64,
    pub version: String,
    pub step: String,
    pub why: String,
    pub host: Option<String>,
    pub checkout: Option<String>,
    pub kept: Kept,
    pub goes_on: Option<String>,
    pub branch: Option<Branch>,
    pub restart: String,
}

/// How the cut that `reported` failed starts again from a host where its
/// checkout stands `kept`, its pushed branch `branch`: `close` is the line
/// that closes its issue, and `delete` the one that deletes that branch.
pub fn restart(
    issue: u64,
    reported: &Reported,
    kept: Kept,
    branch: Option<&Branch>,
    close: &str,
    delete: &str,
) -> String {
    let starts_again = format!(
        "`{close}`, once what failed it is fixed: the next idle tick, or busy tick past the age \
         bound, cuts again from the start, on the base tip as it stands"
    );
    let goes_on = reported.goes_on.as_deref().unwrap_or("afkconvoy cut --from <step>");
    let mut said = match kept {
        Kept::Here => format!(
            "#{issue}: its checkout is kept here; once what failed it is fixed, `{goes_on}` goes \
             on from the step `{}` and closes it. A fix that must land first is not in that \
             checkout's snapshot: then {starts_again}",
            reported.step
        ),
        Kept::Elsewhere => format!(
            "#{issue}: its checkout is kept on the host `{}`, where `{goes_on}` goes on from the \
             step `{}`; where it is gone there too, or a fix must land first, {starts_again}",
            reported.host.as_deref().unwrap_or("?"),
            reported.step
        ),
        Kept::Gone => format!(
            "#{issue}: its kept checkout is gone, so no `--from` line can go on with it: \
             {starts_again}"
        ),
    };
    said.push_str(&match branch {
        None => ". It pushed no branch.".to_string(),
        Some(branch) if branch.taken_up => format!(
            ". Its branch `{}` ({}) stands on the base tip: the next cut of v{} takes up that \
             commit and pushes no second branch, so it stays.",
            branch.name,
            short(&branch.commit),
            reported.version
        ),
        Some(branch) => format!(
            ". Its branch `{}` ({}) stands on an older base: the next cut makes its own commit \
             on the base tip and pushes a branch of its own, taking up nothing of it, so it may \
             go: `{delete}`.",
            branch.name,
            short(&branch.commit)
        ),
    });
    said
}

/// The branch a cut of `version` started at `started` commits on: its own,
/// never the base branch. Its name begins `branch_prefix(version)`.
pub fn branch(version: &str, started: u64) -> String {
    format!("{}{started}", branch_prefix(version))
}

/// What the name of every branch a cut of `version` commits on begins with.
pub fn branch_prefix(version: &str) -> String {
    format!("{CHECKOUT}/v{version}-")
}

/// The branch among `heads` — each a branch's name and its commit — that a
/// cut of `version` pushed, and its commit: the one whose stamp is latest,
/// where more than one is. A later cut of that version goes on from it,
/// rather than pushing a second.
pub fn pushed(heads: &[(String, String)], version: &str) -> Option<(String, String)> {
    let prefix = branch_prefix(version);
    let stamped = heads.iter().filter_map(|(name, commit)| {
        let stamp = canonical_number(name.strip_prefix(&prefix)?)?;
        Some((stamp, name, commit))
    });
    let latest = stamped.max_by_key(|(stamp, _, _)| *stamp)?;
    Some((latest.1.clone(), latest.2.clone()))
}

/// The version of the cut started at `started` that pushed one of
/// `heads`' branches, where one did: what a cut whose own record is out of
/// reach is known by.
pub fn version_started(heads: &[(String, String)], started: u64) -> Option<String> {
    let suffix = format!("-{started}");
    heads.iter().find_map(|(name, _)| {
        let version = name.strip_prefix(&format!("{CHECKOUT}/v"))?.strip_suffix(&suffix)?;
        Version::parse(version).map(|version| version.to_string())
    })
}

/// The title of a cut's commit and of its tag: the repo's name and the
/// version, `afkconvoy 0.5.17`.
pub fn title(repo: &str, version: &str) -> String {
    format!("{repo} {version}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claims::Owner;
    use crate::lease::LEASE;

    fn tags(names: &[(&str, &str)]) -> Vec<(String, String)> {
        names.iter().map(|(name, commit)| (name.to_string(), commit.to_string())).collect()
    }

    fn version(major: u64, minor: u64, patch: u64) -> Version {
        Version { major, minor, patch }
    }

    #[test]
    fn a_version_tag_is_v_and_three_numbers() {
        assert_eq!(Version::tagged("v0.5.16"), Some(version(0, 5, 16)));
        assert_eq!(Version::tagged("v10.0.0"), Some(version(10, 0, 0)));
        for not in ["0.5.16", "v0.5", "v0.5.16.1", "v0.5.16-rc1", "v0.05.1", "v", "release-1"] {
            assert_eq!(Version::tagged(not), None, "{not}");
        }
        assert_eq!(version(0, 5, 16).tag(), "v0.5.16");
        assert_eq!(version(0, 5, 16).to_string(), "0.5.16");
    }

    #[test]
    fn the_newest_tag_is_the_highest_version_not_the_last_listed() {
        let listed = tags(&[("v0.5.9", "a"), ("v0.5.16", "b"), ("v0.5.10", "c"), ("nightly", "d")]);
        assert_eq!(newest(&listed), Some(Tag { version: version(0, 5, 16), commit: "b".into() }));
        assert_eq!(newest(&tags(&[("nightly", "d")])), None);
        assert_eq!(newest(&[]), None);
    }

    #[test]
    fn a_cut_makes_the_next_patch_or_the_next_minor_for_a_pr_labelled_so() {
        let from = version(0, 5, 16);
        assert_eq!(pick(from, &[451, 452], &[]), Some(version(0, 5, 17)));
        assert_eq!(pick(from, &[451, 452], &[300, 452]), Some(version(0, 6, 0)));
        assert_eq!(pick(from, &[451], &[300]), Some(version(0, 5, 17)), "a PR it does not hold");
        assert_eq!(version(0, 5, u64::MAX).next(false), None);
    }

    #[test]
    fn the_prs_past_a_tag_are_read_off_the_subjects_of_its_landings() {
        let subjects = [
            "Merge commit 'abc' into a branch",
            "The fleet reads no PR opened from a fork (#464)",
            "afkconvoy 0.5.16 (#450)",
            "a fix (#0012)",
            "The fleet reads no PR opened from a fork (#464)",
            "Issues are read in the tracker (#465)  ",
        ];
        assert_eq!(prs_of(subjects), [450, 464, 465]);
    }

    fn seen() -> Seen {
        Seen {
            on: true,
            newest: Some(Tag { version: version(0, 5, 16), commit: "t".into() }),
            initial: None,
            past: 4,
            oldest_landing: None,
            busy_after_seconds: BUSY_AFTER_SECONDS,
            claim: None,
            failure: None,
        }
    }

    fn claim(instance: &str) -> RunClaim {
        let owner = Owner { instance: instance.into(), host: "h".into(), ts: 0 };
        RunClaim { key: CLAIM_KEY.into(), owner: Some(owner), sha: "c".into(), over: false }
    }

    fn standing(seen: &Seen, locked: bool) -> Standing {
        let me = InstanceId::parse("me").unwrap();
        row(seen, Some(&me), &[("peer".into(), 1000)], 1000, locked).standing
    }

    #[test]
    fn a_cut_is_due_with_commits_past_the_newest_tag_in_a_repo_that_turns_it_on() {
        assert_eq!(standing(&seen(), false), Standing::Due);
        assert_eq!(standing(&Seen { on: false, ..seen() }, false), Standing::Off);
        assert_eq!(standing(&Seen { past: 0, ..seen() }, false), Standing::Tagged);
        assert_eq!(standing(&Seen { newest: None, ..seen() }, false), Standing::Untagged);
        assert_eq!(standing(&Seen { failure: Some(9), ..seen() }, false), Standing::Failed);
        assert_eq!(standing(&seen(), true), Standing::Locked);
        let said = row(&seen(), None, &[], 1000, false);
        assert_eq!((said.newest_tag.as_deref(), said.past), (Some("v0.5.16"), 4));
    }

    #[test]
    fn a_cut_still_going_is_whose_its_claim_says_whatever_else_holds() {
        let off = Seen { on: false, failure: Some(9), ..seen() };
        assert_eq!(
            standing(&Seen { claim: Some(claim("me")), ..off.clone() }, true),
            Standing::Running
        );
        assert_eq!(
            standing(&Seen { claim: Some(claim("peer")), ..off.clone() }, false),
            Standing::PeerRunning
        );
        let me = InstanceId::parse("me").unwrap();
        let dead = Seen { claim: Some(claim("peer")), ..seen() };
        let stale = row(&dead, Some(&me), &[("peer".into(), 1000)], 1001 + LEASE, false);
        assert_eq!(stale.standing, Standing::StaleRun);
    }

    #[test]
    fn a_declared_initial_version_is_due_only_on_an_untagged_remote() {
        let first = Seen { newest: None, initial: Some(version(1, 2, 3)), past: 0, ..seen() };
        let row = row(&first, None, &[], 1000, false);
        assert_eq!(row.standing, Standing::Due);
        assert_eq!(row.initial.as_deref(), Some("1.2.3"));
        assert_eq!(row.newest_tag, None);
        assert_eq!(standing(&first, true), Standing::Locked);
        assert_eq!(standing(&Seen { failure: Some(9), ..first }, false), Standing::Failed);
        let tagged = Seen { initial: Some(version(1, 2, 3)), past: 0, ..seen() };
        assert_eq!(standing(&tagged, false), Standing::Tagged);
        assert_eq!(super::row(&tagged, None, &[], 1000, false).initial, None);
    }

    #[test]
    fn only_a_tick_that_ends_idle_with_admission_open_starts_a_due_cut() {
        let due = row(&seen(), None, &[], 1000, false);
        assert!(starts(&due, 0, 0, false));
        assert!(!starts(&due, 1, 0, false), "a claim held");
        assert!(!starts(&due, 0, 1, false), "the frontier left");
        assert!(!starts(&due, 0, 0, true), "admission holds back");
        let tagged = row(&Seen { past: 0, ..seen() }, None, &[], 1000, false);
        assert!(!starts(&tagged, 0, 0, false));
    }

    #[test]
    fn the_age_bound_starts_a_busy_cut_but_cannot_bypass_its_claim_failure_or_lock() {
        assert_eq!(BUSY_AFTER_SECONDS, 18_000, "omitting the key waits five hours");
        let seen = Seen { oldest_landing: Some(1000), ..seen() };
        let bound = 1000 + BUSY_AFTER_SECONDS;
        let before = row(&seen, None, &[], bound - 1, false);
        assert_eq!(before.oldest_landing_age_seconds, Some(BUSY_AFTER_SECONDS - 1));
        assert_eq!(before.busy_in_seconds, Some(1));
        assert!(!starts(&before, 8, 4, true));
        let due = row(&seen, None, &[], bound, false);
        assert!(starts(&due, 8, 4, true));
        let locked = row(&seen, None, &[], bound, true);
        assert!(!starts(&locked, 8, 4, false));
        let failed = row(&Seen { failure: Some(9), ..seen.clone() }, None, &[], bound, false);
        assert!(!starts(&failed, 8, 4, false));
        let running = row(
            &Seen { claim: Some(claim("peer")), ..seen },
            None,
            &[("peer".into(), bound)],
            bound,
            false,
        );
        assert!(!starts(&running, 8, 4, false));
    }

    #[test]
    fn zero_starts_a_busy_cut_at_the_landing_and_another_bound_is_kept() {
        let landed = Seen { oldest_landing: Some(1000), ..seen() };
        let zero = Seen { busy_after_seconds: 0, ..landed.clone() };
        let at = row(&zero, None, &[], 1000, false);
        assert_eq!(at.busy_in_seconds, Some(0));
        assert!(starts(&at, 8, 4, true), "zero permits a busy cut at the next tick");
        let kept = Seen { busy_after_seconds: 120, ..landed };
        let early = row(&kept, None, &[], 1119, false);
        assert_eq!((early.oldest_landing_age_seconds, early.busy_in_seconds), (Some(119), Some(1)));
        assert!(!starts(&early, 8, 4, true));
        assert!(starts(&row(&kept, None, &[], 1120, false), 8, 4, true));
    }

    #[test]
    fn a_cut_gates_again_only_where_the_pause_and_a_whole_gate_fit_its_time() {
        let limit = time_limit(600);
        assert!(gate_again(1000, 1000, GATE_RETRY_SECONDS, 600, limit));
        let last = 1000 + limit - GATE_RETRY_SECONDS - 600;
        assert!(gate_again(1000, last - 1, GATE_RETRY_SECONDS, 600, limit));
        assert!(!gate_again(1000, last, GATE_RETRY_SECONDS, 600, limit));
        assert!(!gate_again(1000, u64::MAX, GATE_RETRY_SECONDS, 600, limit));
    }

    #[test]
    fn a_cut_is_given_two_gate_limits_and_three_hours() {
        assert_eq!(BESIDE_THE_GATES, 10_800);
        // The default gate limit is 3600s, so a cut is given five hours.
        assert_eq!(time_limit(3600), 18_000);
        assert_eq!(time_limit(600), 2 * 600 + 10_800);
        assert_eq!(time_limit(u64::MAX), u64::MAX);
    }

    fn record() -> Record {
        Record {
            version: "0.5.17".into(),
            from: Some("v0.5.16".into()),
            prs: vec![451, 452],
            minor: false,
            started: 1000,
            branch: None,
            commit: None,
            at: None,
            took: vec![],
            failed: None,
            superseded: None,
        }
    }

    #[test]
    fn a_cut_ends_once_it_took_every_step_failed_ran_out_of_time_or_is_gone() {
        let limit = time_limit(600);
        assert_eq!(ending(Some(&record()), 1000, false, 1001, limit), None);
        let all = Step::ALL.map(|step| Took { step, seconds: 1 }).to_vec();
        let done = Record { took: all, ..record() };
        assert!(done.done());
        assert_eq!(ending(Some(&done), 1000, true, 1001, limit), Some(Ending::Cut));
        let failure = Failure::at(Step::Gate, "red");
        let failed = Record { failed: Some(failure.clone()), at: Some(Step::Gate), ..record() };
        assert_eq!(ending(Some(&failed), 1000, false, 1001, limit), Some(Ending::Failed(failure)));
        let landing = Record { at: Some(Step::Gate), ..record() };
        assert_eq!(
            ending(Some(&landing), 1000, false, 1000 + limit, limit),
            Some(Ending::OutOfTime(Step::Gate))
        );
        assert_eq!(ending(Some(&landing), 1000, true, 1001, limit), Some(Ending::Gone(Step::Gate)));
        // A record that cannot be read is a cut that said nothing.
        assert_eq!(ending(None, 1000, true, 1001, limit), Some(Ending::Gone(Step::Set)));
        assert_eq!(ending(None, 1000, false, 1001, limit), None);
        let out = Ending::OutOfTime(Step::Gate).failure(limit).unwrap();
        assert_eq!(out.step, Step::Gate);
        assert!(out.why.contains("its time ran out"), "{}", out.why);
        assert!(out.why.contains("200 minutes"), "{}", out.why);
        let said = failure_report(
            &landing,
            &out,
            "h1",
            Some("/w/cut"),
            "cd '/w/cut' && afkconvoy cut --from land",
        );
        assert!(said.contains("its time ran out"), "{said}");
    }

    #[test]
    fn a_cut_goes_on_from_the_step_after_the_last_it_took() {
        assert_eq!(record().step(), Step::Set);
        let set = Record { took: vec![Took { step: Step::Set, seconds: 3 }], ..record() };
        assert_eq!(set.step(), Step::Push);
        assert_eq!(Step::Tag.from(), [Step::Tag, Step::Publish]);
        assert_eq!(Step::named("publish"), Some(Step::Publish));
        assert_eq!(Step::named("deploy"), None);
    }

    #[test]
    fn what_a_cut_did_is_said_in_one_line_with_each_step_and_its_seconds() {
        let did = Record {
            branch: Some("afkconvoy-cut/v0.5.17-1000".into()),
            commit: Some("0123456789abcdef".into()),
            took: vec![
                Took { step: Step::Set, seconds: 3 },
                Took { step: Step::Gate, seconds: 270 },
            ],
            ..record()
        };
        assert_eq!(
            did.line(),
            "v0.5.17 (past v0.5.16, holding #451, #452) · its commit 0123456789 on \
             afkconvoy-cut/v0.5.17-1000 · set 3s, gate 270s"
        );
        let minor = Record { minor: true, prs: vec![], ..record() };
        assert_eq!(
            minor.line(),
            "v0.5.17 (past v0.5.16, the next minor for a PR labelled release:minor)"
        );
    }

    #[test]
    fn an_initial_cuts_record_names_no_previous_tag() {
        let first = Record { version: "1.2.3".into(), from: None, ..record() };
        assert!(first.line().starts_with("v1.2.3 (the initial version, holding #451"));
        let old = serde_json::to_value(record()).unwrap();
        assert_eq!(old["from"], "v0.5.16");
        let read: Record = serde_json::from_value(old).unwrap();
        assert_eq!(read.from.as_deref(), Some("v0.5.16"));
    }

    #[test]
    fn a_failed_cut_is_found_again_by_its_title_and_the_escalate_label() {
        let issue = |number: u64, title: &str, labels: &[&str]| Issue {
            number,
            title: title.to_string(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            open_blockers: vec![],
        };
        let issues = [
            issue(9, &failure_title("0.5.17"), &["ready-for-human"]),
            issue(4, &failure_title("0.5.17"), &["bug"]),
            issue(7, &failure_title("0.5.16"), &["ready-for-human"]),
            issue(3, "afkconvoy: the patrol `x` failed", &["ready-for-human"]),
        ];
        assert_eq!(failure_of(&issues, "ready-for-human"), Some(7));
        assert_eq!(failure_of(&issues, "fleet-lock"), None);
        // The watch's label finds it as the escalate label does.
        let watched = [issue(5, &failure_title("0.5.17"), &[FAILED_LABEL])];
        assert_eq!(failure_of(&watched, "fleet-lock"), Some(5));
        let failure = Failure::at(Step::Publish, "exited 1");
        let said = failure_report(
            &record(),
            &failure,
            "h1",
            Some("/w/cut"),
            "cd '/w/cut' && afkconvoy cut --from publish",
        );
        assert!(
            said.starts_with(
                "<!--convoy:cut-failed version=0.5.17 step=publish host=h1 checkout=/w/cut-->\n"
            ),
            "{said}"
        );
        assert!(said.contains("failed at its step `publish`: exited 1"), "{said}");
        assert!(said.contains("cd '/w/cut' && afkconvoy cut --from publish"), "{said}");
        assert!(said.contains("No cut starts while this issue is open"), "{said}");
        // Its tag is pushed: closed, the version stays unpublished.
        assert!(said.contains("v0.5.17 stays tagged and is never published"), "{said}");
        let failure = Failure::at(Step::Gate, "red");
        let said = failure_report(&record(), &failure, "h1", Some("/w/cut"), "x");
        assert!(!said.contains("Its tag is pushed"), "{said}");
        // A red gate's report names its failing tests, quotes the run and
        // says where the whole of it is kept.
        let red = RedGate {
            tests: vec!["afkconvoy cutting::a".into(), "afkconvoy cutting::b".into()],
            quote: vec!["  FAIL [ 0.1s] afkconvoy cutting::a".into(), "  boom".into()],
            output: "/w/.git/convoy-cut-gate-1.log".into(),
        };
        let failure = Failure { red: Some(red), ..Failure::at(Step::Gate, "red") };
        let said = failure_report(&record(), &failure, "h1", Some("/w/cut"), "x");
        assert!(
            said.contains("failed on: `afkconvoy cutting::a`, `afkconvoy cutting::b`"),
            "{said}"
        );
        assert!(said.contains("````\n  FAIL [ 0.1s] afkconvoy cutting::a\n  boom\n````"), "{said}");
        assert!(said.contains("kept in `/w/.git/convoy-cut-gate-1.log`"), "{said}");
        let failure = Failure {
            red: Some(RedGate { tests: vec![], quote: vec!["x".into()], output: "o".into() }),
            ..Failure::at(Step::Gate, "red")
        };
        let said = failure_report(&record(), &failure, "h1", Some("/w/cut"), "x");
        assert!(said.contains("names no failing test"), "{said}");
    }

    #[test]
    fn a_failure_report_reads_back_and_says_how_the_cut_starts_again() {
        let failure = Failure::at(Step::Gate, "red: `steer::a` timed out");
        let line = "cd '/w/cut' && afkconvoy cut --from gate";
        let body = failure_report(&record(), &failure, "h1", Some("/w/cut"), line);
        assert!(body.contains(FAILED_LABEL), "{body}");
        let read = reported(&body).unwrap();
        assert_eq!(
            read,
            Reported {
                version: "0.5.17".into(),
                step: "gate".into(),
                why: "red: `steer::a` timed out".into(),
                host: Some("h1".into()),
                checkout: Some("/w/cut".into()),
                goes_on: Some(line.into()),
            }
        );
        // One written before its marker named host and checkout.
        let old = body.replacen(" host=h1 checkout=/w/cut", "", 1);
        assert_eq!(reported(&old).unwrap(), read);
        assert_eq!(reported("a body that is no report"), None);

        let gone = failure_report(&record(), &failure, "h1", None, "close this issue");
        assert!(gone.contains("Its checkout is gone from the host `h1`"), "{gone}");
        let gone = reported(&gone).unwrap();
        assert_eq!((gone.checkout, gone.goes_on), (None, None));

        let close = "gh issue close 9";
        let delete = "git push origin --delete afkconvoy-cut/v0.5.17-1000";
        let stale = Branch {
            name: "afkconvoy-cut/v0.5.17-1000".into(),
            commit: "0123456789abcdef".into(),
            taken_up: false,
        };
        let here = restart(9, &read, Kept::Here, None, close, delete);
        assert!(here.contains("kept here"), "{here}");
        assert!(here.contains(line), "{here}");
        assert!(here.contains("It pushed no branch."), "{here}");
        let elsewhere = restart(9, &read, Kept::Elsewhere, Some(&stale), close, delete);
        assert!(elsewhere.contains("kept on the host `h1`"), "{elsewhere}");
        assert!(elsewhere.contains(close), "{elsewhere}");
        let gone = restart(9, &read, Kept::Gone, Some(&stale), close, delete);
        assert!(gone.contains("its kept checkout is gone"), "{gone}");
        assert!(gone.contains(&format!("`{close}`")), "{gone}");
        assert!(gone.contains("stands on an older base"), "{gone}");
        assert!(gone.contains(&format!("so it may go: `{delete}`")), "{gone}");
        let kept = Branch { taken_up: true, ..stale };
        let gone = restart(9, &read, Kept::Gone, Some(&kept), close, delete);
        assert!(gone.contains("takes up that commit"), "{gone}");
        assert!(gone.contains("so it stays"), "{gone}");
        assert!(!gone.contains(delete), "{gone}");
    }

    #[test]
    fn a_record_an_earlier_afkconvoy_kept_goes_on_at_the_step_it_names_now() {
        let old = serde_json::json!({"version": "0.5.17", "from": "v0.5.16", "started": 1000,
            "pr": 470, "landed": "abc", "at": "land",
            "took": [{"step": "set", "seconds": 1}, {"step": "pr", "seconds": 2}]});
        let read: Record = serde_json::from_value(old).unwrap();
        assert_eq!(read.commit.as_deref(), Some("abc"), "the commit its turn landed is tagged");
        assert_eq!(read.step(), Step::Gate);
        assert_eq!(read.took[1].step, Step::Push);
        assert_eq!(serde_json::to_value(Step::Push).unwrap(), "push");
    }

    #[test]
    fn a_cut_s_branch_is_its_own_and_a_later_cut_of_the_version_finds_it() {
        assert_eq!(branch("0.5.64", 1793666786), "afkconvoy-cut/v0.5.64-1793666786");
        let heads = tags(&[
            ("main", "m"),
            ("afkconvoy-cut/v0.5.64-1793666786", "a"),
            ("afkconvoy-cut/v0.5.64-1793672283", "d"),
            ("afkconvoy-cut/v0.5.640-1793672999", "x"),
            ("afkconvoy-cut/v0.5.63-1793600000", "o"),
        ]);
        assert_eq!(
            pushed(&heads, "0.5.64"),
            Some(("afkconvoy-cut/v0.5.64-1793672283".into(), "d".into())),
            "the latest of the version, not another version's"
        );
        assert_eq!(pushed(&heads, "0.5.65"), None);
        assert_eq!(version_started(&heads, 1793666786).as_deref(), Some("0.5.64"));
        assert_eq!(version_started(&heads, 1), None);
    }

    #[test]
    fn the_cut_steps_need_both_lines() {
        let steps = |set: &str, publish: &str| Steps {
            busy_after_seconds: BUSY_AFTER_SECONDS,
            initial: None,
            set: set.into(),
            publish: publish.into(),
        };
        assert!(steps("bump", "ship").checked().is_ok());
        assert_eq!(steps(" ", "ship").checked(), Err("set"));
        assert_eq!(steps("bump", "").checked(), Err("publish"));
    }
}
