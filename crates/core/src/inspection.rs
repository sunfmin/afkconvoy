//! An inspection (docs/adr/0026): a worker started so that the fleet's work
//! goes on — on a hot worker, in the role of the stage's consult (the stock
//! consult where the stage names none, and in an unbound repo), or on the
//! host once admission keeps holding back (`crate::host_inspection`). It is
//! the operator on its host: it finds what holds the work back, fixes it,
//! checks that the work goes on and says what it did. Who inspects, which hot
//! worker is inspected next, what an inspection is told, what it may never
//! touch, and what is left of what it did are decided here; taking down the
//! evidence, starting the inspection and ending it are the effect layer's.

use serde::{Deserialize, Serialize};

use crate::prompt::fill;
use crate::record;
use crate::team::{Consult, Stage, Work};

/// The variable an inspection's environment names what it inspects in — the
/// hot worker, or the host: the guard holds a worker with it as the operator
/// on its host, within the bounds of any unattended worker.
pub const INSPECTION_ENV: &str = "AFKCONVOY_INSPECTION";

/// How long, in seconds, an inspection is given: what it said by then is
/// what is kept. An hour, for a change of the repo it makes is gated within
/// it, the gate's wait in the host's queue among it.
pub const TIME_LIMIT: u64 = 3600;

/// The name an inspection's checkout is created under: one a repo, for one
/// inspection runs on its host at a time.
pub const CHECKOUT: &str = "afkconvoy-inspection";

/// The word of the marker of an inspection's comment: no stage record, and
/// nothing derives the stage an issue is at from it (docs/adr/0010).
const MARKER: &str = "convoy:inspection";

/// What under the home folder cannot be rebuilt, or is expensive to fetch
/// again — model weights, downloads, media, Docker's images, the user's own
/// files: an inspection never deletes it, and the guard refuses a deletion
/// it reads there.
pub const KEPT: [&str; 13] = [
    ".cache/huggingface",
    ".cache/modelscope",
    ".ollama",
    ".lmstudio",
    ".docker",
    "Library/Containers/com.docker.docker",
    "Downloads",
    "Movies",
    "Music",
    "Pictures",
    "Documents",
    "Desktop",
    "Library/Mobile Documents",
];

/// Whether `relative` — a path under the home folder, relative to it, its
/// parts parted by `/` — is in what is kept (`KEPT`), or holds some of it. A
/// part a shell pattern stands in (`*`, `?`, `[`) may be any.
pub fn kept(relative: &str) -> bool {
    let parts: Vec<&str> = relative.split('/').filter(|part| !part.is_empty()).collect();
    let pattern = |part: &str| part.contains(['*', '?', '[']);
    KEPT.iter().any(|root| {
        let root: Vec<&str> = root.split('/').collect();
        parts.iter().zip(root.iter()).all(|(part, root)| part == root || pattern(part))
    })
}

/// What an inspection is started on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A hot worker.
    Worker,
    /// The host, on which admission held back new workers past a span
    /// (`crate::host_inspection`).
    Host,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Worker => "worker",
            Kind::Host => "host",
        }
    }
}

/// How the log, the panel and `doctor` say an inspection ended that said
/// what it did — beside `nothing`, of one that said nothing in time.
pub const SAID: &str = "said";

/// What an inspection said as it ended: what held the work back — and what
/// it could not fix, if anything — and each thing it did; with the PRs of the
/// changes of the repo it put on the landing train.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// What held the work back, where and why, in its own words.
    pub said: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub did: Vec<String>,
    /// The PRs it put on the landing train (`afkconvoy land --inspection`),
    /// which the train lands as any other.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prs: Vec<u64>,
}

impl Finding {
    /// What it said and did, and its PRs on the landing train, in one line:
    /// what the log keeps.
    pub fn line(&self) -> String {
        let each = |items: &[String]| items.iter().map(|item| one_line(item)).collect::<Vec<_>>();
        let mut said = vec![one_line(&self.said)];
        if !self.did.is_empty() {
            said.push(format!("did: {}", each(&self.did).join("; ")));
        }
        if !self.prs.is_empty() {
            said.push(format!("on the landing train: {}", numbered(&self.prs)));
        }
        said.join(" · ")
    }
}

/// `numbers` as a sentence names them: `#451`, or `#451, #452`.
fn numbered(numbers: &[u64]) -> String {
    numbers.iter().map(|number| format!("#{number}")).collect::<Vec<_>>().join(", ")
}

/// `text`, its lines and spaces run into one line.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Who inspects a hot worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inspector<'t> {
    /// The consult its stage names: this skill of a role.
    Role(&'t Work),
    /// The stock consult.
    Stock,
}

/// Who inspects the hot worker of a bound repo at `stage`: the consult the
/// stage names, the stock consult where it names none — and where the
/// worker is at no stage, a patrol's or a landing worker's. Nobody where the
/// stage says `escalate: { to: human }` or a human performs it: then nothing
/// is inspected.
pub fn inspector(stage: Option<&Stage>) -> Option<Inspector<'_>> {
    let Some(stage) = stage else { return Some(Inspector::Stock) };
    match &stage.consult {
        Some(Consult::Role(work)) => Some(Inspector::Role(work)),
        Some(Consult::Stock) => Some(Inspector::Stock),
        None => None,
    }
}

/// One hot worker, as the inspector sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hot {
    /// Its worktree, which tells it from every other worker.
    pub worker: String,
    /// When its episode started.
    pub since: u64,
    /// Whether what shows it was taken down: it is inspected only then.
    pub evidence: bool,
}

/// The episodes the inspector has had its say on: each hot worker is
/// inspected once per episode.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inspected(Vec<(String, u64)>);

impl Inspected {
    /// Keeps only the episodes of `hot`: one that ended is forgotten, and the
    /// worker hot again later is inspected again.
    pub fn keep_to(&mut self, hot: &[Hot]) {
        self.0.retain(|(worker, since)| {
            hot.iter().any(|hot| hot.worker == *worker && hot.since == *since)
        });
    }

    /// Records that the inspector had its say on `hot`'s episode.
    pub fn record(&mut self, hot: &Hot) {
        if !self.has(hot) {
            self.0.push((hot.worker.clone(), hot.since));
        }
    }

    fn has(&self, hot: &Hot) -> bool {
        self.0.iter().any(|(worker, since)| *worker == hot.worker && *since == hot.since)
    }

    /// The hot worker to inspect next, of `hot`: the one hot longest whose
    /// evidence is taken down and whose episode was not inspected yet.
    pub fn next<'h>(&self, hot: &'h [Hot]) -> Option<&'h Hot> {
        let mut next: Option<&Hot> = None;
        for one in hot {
            let due = one.evidence && !self.has(one);
            if due && next.is_none_or(|next| one.since < next.since) {
                next = Some(one);
            }
        }
        next
    }
}

/// Whether the inspection started at `started` is over at `now`: it said
/// what it did, its worker is gone, or its time is up.
pub fn over(started: u64, said: bool, gone: bool, now: u64) -> bool {
    said || gone || now.saturating_sub(started) >= TIME_LIMIT
}

/// The comment left on the hot worker's issue once its inspection is over:
/// what held the work back, what it did and its PRs on the landing train, or
/// that it said nothing in time. A marker of its own: it is no stage record.
pub fn comment(worker: &str, why: &str, by: &str, finding: Option<&Finding>) -> String {
    let word = finding.map_or("nothing", |_| SAID);
    let marker = record::marker(MARKER, &[("found", word)]);
    let head = format!(
        "{marker}\n**afkconvoy** — the worker {worker} turned hot ({why}), and was inspected by \
         `{by}`, as the operator on this host."
    );
    let Some(finding) = finding else {
        return format!("{head} The inspection said nothing in {} minutes.\n", TIME_LIMIT / 60);
    };
    let listed = |items: &[String]| {
        items.iter().map(|item| format!("- {}", one_line(item))).collect::<Vec<_>>().join("\n")
    };
    let mut said = format!("{head}\n\nWhat held the work back:\n\n{}\n", quoted(&finding.said));
    if !finding.did.is_empty() {
        said.push_str(&format!("\nWhat it did:\n\n{}\n", listed(&finding.did)));
    }
    if !finding.prs.is_empty() {
        said.push_str(&format!(
            "\nIts change of the repo is on the landing train, which lands it as any other: {}.\n",
            numbered(&finding.prs)
        ));
    }
    said
}

fn quoted(said: &str) -> String {
    said.trim().lines().map(|line| format!("> {line}")).collect::<Vec<_>>().join("\n")
}

/// What every inspection is told of its authority and its bounds, and how it
/// says what it did — after what it is given, which its kind says.
const OPERATOR: &str = "
## Your authority

You are the operator on this host: do what the operator would do at its keyboard, whatever that
takes, so that the work goes on. For example — not a list to keep: end a runaway or orphaned
process; tell a worker what is wrong at its terminal (`orca terminal send --terminal <terminal>
--text '<line>' --enter`), or restart, nudge or switch it (`afkconvoy restart`, `nudge`,
`switch`); have a gate you stopped run again; change a host setting (`afkconvoy run --concurrency`,
admission's shares: `afkconvoy run --help`); free disk; send heavy work through
`afkconvoy exclusive`; change the repo — its `gate_command`, say. Each command's `--help` says how.

## A change of the repo

Make it yourself, in your checkout, and land it as any worker's change lands — on no issue:

1. Commit it on your checkout's branch, `{branch}`, and push that: `git push origin HEAD`.
2. Open its PR against `{base_branch}`, saying what it changes and why, and closing no issue:
   `gh pr create --base {base_branch} --head {branch} --title '<title>' --body '<what and why>'`.
3. Put it on the landing train: `{afkconvoy} land --inspection`. It takes a turn: merges the
   train's green tip, gates, records green and joins before releasing the turn. On `joined` or
   `merged` it is done: the train lands on that record, and you need not wait for it. On
   `conflict`, resolve every file, commit the merge and ask again. Red: fix your change, commit
   and ask again. On `locked`, a fleet lock stands: the PR stays open off the train, and you say so.
