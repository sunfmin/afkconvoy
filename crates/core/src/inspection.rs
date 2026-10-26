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
/// changes of the repo it put on the base branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// What held the work back, where and why, in its own words.
    pub said: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub did: Vec<String>,
    /// The PRs it put on the base branch (`afkconvoy land --inspection`),
    /// which lands in the same turn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prs: Vec<u64>,
}

impl Finding {
    /// What it said and did, and its PRs on the base branch, in one line:
    /// what the log keeps.
    pub fn line(&self) -> String {
        let each = |items: &[String]| items.iter().map(|item| one_line(item)).collect::<Vec<_>>();
        let mut said = vec![one_line(&self.said)];
        if !self.did.is_empty() {
            said.push(format!("did: {}", each(&self.did).join("; ")));
        }
        if !self.prs.is_empty() {
            said.push(format!("on the base branch: {}", numbered(&self.prs)));
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
/// what held the work back, what it did and its PRs on the base branch, or
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
            "\nIts change of the repo is on the base branch, which lands it as any other: {}.\n",
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
3. Land it: `{afkconvoy} land --inspection`. It takes a turn: makes the PR's merge commit
   on the current base tip, gates that commit, records green and pushes that same commit as
   a fast-forward. On `merged`, it has landed. On `conflict`, resolve every file and commit
   the merge before asking again. On `gate_red`, fix the change and commit before asking
   again. On `target_moved`, ask for another turn on the new base tip. On `locked`, stop:
   the PR stays open while the fleet repairs its base branch.
## A red base branch in an unbound repo

If a gate is red, first read which locks stand (`{afkconvoy} plan`): a lock already standing
needs no second issue. Otherwise prove the base branch is red the same way by gating
`origin/{base_branch}` alone in a clean temporary checkout with `afkconvoy gate`, and remove that checkout after. A failure confined to your change raises no lock.
In an unbound repo, search the open issues for the fix before opening one: `{afkconvoy} flows`
gives the tracker, repo label and ready label in its issue-opening `line`. Comment your evidence
on the existing issue, or open one with the symptom, reproduction, evidence and acceptance
criteria requiring the cause and a test. Make the issue carry the ready label and repo label
that line names, then raise the fleet lock: `{afkconvoy} lock --issue <n>`. Stop running gates
while the lock stands, and say what you did below. This is the one exception to opening no issue.

## Your bounds

They are those of any unattended worker, and the guard holds you to them where it can:

- Only what the fleets of this host started or own. Never the user's own: their programs and files,
  other apps, the host runner, orca. End a process by its pid, `kill <pid>`, and only one the
  fleet started: a worker's or an inspection's, or one they left behind.
- Never delete what cannot be rebuilt: model weights, downloads, media ({kept}), a user's files, a
  change no commit holds.
- No force push, no push to the base branch, no merge outside a landing turn.
- Hand nothing to a human. Apart from the proven red base branch above, open no issue: what
  should be changed, change.

## Say what you did

You have {minutes_left} minutes. Check that the work goes on. Then write what held it back, where
and why — and what you could not fix, if anything, and why — to a file, and run this once, with a
`--did` for each thing you did, and stop:
```bash
{afkconvoy} inspected --text-file <file> [--did '<what you did>']...
```
Each is a line of the fleet's log, shown by `afkconvoy doctor` and the panel{shown_on}, and so is
each PR you put on the base branch. What you could not fix, the next episode inspects again.
";

/// Where an inspection works, and how it runs afkconvoy there: what every
/// inspection's brief is filled with, whatever its kind.
#[derive(Debug, Clone, Copy)]
pub struct Operator<'a> {
    /// The inspection's own checkout of the base branch, and the branch of
    /// its own checked out there, on which a change of the repo is made.
    pub checkout: &'a str,
    pub branch: &'a str,
    /// The base branch, which the PR of a change is opened against.
    pub base_branch: &'a str,
    /// How it runs afkconvoy from its checkout: `inspected`, `gate --land`
    /// and `land --inspection` follow it.
    pub afkconvoy: &'a str,
}

/// The brief of an inspection of `kind`, working as `operator` says, is
/// filled with these beside its own: `OPERATOR`'s, and its checkout's.
pub(crate) fn operator_fields(kind: Kind, operator: &Operator) -> [(&'static str, String); 7] {
    let shown_on = match kind {
        Kind::Worker => ", and left as a comment on the hot worker's issue",
        Kind::Host => ", beside admission's reading",
    };
    [
        ("kept", KEPT.map(|root| format!("`~/{root}`")).join(", ")),
        ("minutes_left", (TIME_LIMIT / 60).to_string()),
        ("shown_on", shown_on.to_string()),
        ("checkout", operator.checkout.to_string()),
        ("branch", operator.branch.to_string()),
        ("base_branch", operator.base_branch.to_string()),
        ("afkconvoy", operator.afkconvoy.to_string()),
    ]
}

/// `template` with its `{instruction}` and `{operator}` put in: the stock
/// consult's instruction where one is given, and `OPERATOR`.
pub(crate) fn with_operator(template: &str, instruction: Option<&str>) -> String {
    let instruction = instruction.map(|text| format!("\n{text}")).unwrap_or_default();
    template.replace("{instruction}", &instruction).replace("{operator}", OPERATOR)
}

/// What an inspection's brief is filled with.
#[derive(Debug, Clone, Copy)]
pub struct Fields<'a> {
    /// The hot worker: `#7`, or its worktree's name.
    pub worker: &'a str,
    /// The issue it is on, as a line of the brief: none of a patrol's.
    pub issue: Option<u64>,
    /// Why it turned hot, and the most it held, in words.
    pub why: &'a str,
    pub held: &'a str,
    /// Its worktree, its terminal, and where what shows it was taken down.
    pub worktree: &'a str,
    pub terminal: &'a str,
    pub evidence: &'a str,
    pub role: &'a str,
    pub skill: &'a str,
    /// The stock consult's instruction, where nothing else gives it: an
    /// unbound repo's stock consult starts in no directory of its own.
    pub instruction: Option<&'a str>,
    /// Where it works, and how it runs afkconvoy there.
    pub operator: Operator<'a>,
}

const BRIEF: &str = "\
You are an afkconvoy **inspection**, started in your role (`{role}`, skill `{skill}`) on a **hot
worker**: one whose processes take more than their share of this host. Nobody reads this terminal:
never wait for an answer.
{instruction}
## Your goal

The fleet's work goes on. Find what holds it back — what the hot worker takes so much for, and why:
work that is wrong, work that really is that heavy, or what is not the worker's at all — fix it,
check that the work goes on, and say what you did.

## What you are given

- **Hot worker:** {worker}{issue_line}. Hot for {why}; at the most it held {held}
- **Its worktree:** `{worktree}`
- **Its terminal:** `{terminal}`: read it with `orca terminal read --terminal {terminal}`
- **What was taken down as it turned hot:** `{evidence}` — the process tree with each command
  line, a five-second `sample` of the hottest process, its resident memory over the last minutes,
  the processes that wrote the most, and the largest directories of its worktree
- **Your checkout:** `{checkout}`, the base branch as it stood, on a branch of its own: `{branch}`
{operator}";

/// The brief an inspection of a hot worker is given.
pub fn brief(fields: &Fields) -> Result<String, String> {
    let issue_line = fields.issue.map(|issue| format!(", on issue #{issue}")).unwrap_or_default();
    let template = with_operator(BRIEF, fields.instruction).replace("{issue_line}", &issue_line);
    let operator = operator_fields(Kind::Worker, &fields.operator);
    let mut filled: Vec<(&str, &str)> = vec![
        ("role", fields.role),
        ("skill", fields.skill),
        ("worker", fields.worker),
        ("why", fields.why),
        ("held", fields.held),
        ("worktree", fields.worktree),
        ("terminal", fields.terminal),
        ("evidence", fields.evidence),
    ];
    filled.extend(operator.iter().map(|(name, value)| (*name, value.as_str())));
    fill(&template, &filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hot(worker: &str, since: u64, evidence: bool) -> Hot {
        Hot { worker: worker.to_string(), since, evidence }
    }

    #[test]
    fn a_hot_worker_is_inspected_once_per_episode_and_again_in_a_new_one() {
        let mut inspected = Inspected::default();
        let first = [hot("/w/a", 100, true)];
        assert_eq!(inspected.next(&first), Some(&first[0]));
        inspected.record(&first[0]);
        assert_eq!(inspected.next(&first), None, "once per episode");
        // The episode ended: forgotten, and the next one is inspected again.
        inspected.keep_to(&[]);
        assert_eq!(inspected, Inspected::default());
        let again = [hot("/w/a", 900, true)];
        assert_eq!(inspected.next(&again), Some(&again[0]));
    }

    #[test]
    fn the_worker_hot_longest_with_its_evidence_taken_is_next() {
        let inspected = Inspected::default();
        let hot = [hot("/w/a", 300, true), hot("/w/b", 100, false), hot("/w/c", 200, true)];
        // b's evidence is still being taken down
        assert_eq!(inspected.next(&hot).map(|hot| hot.worker.as_str()), Some("/w/c"));
        assert_eq!(inspected.next(&[]), None);
    }

    #[test]
    fn an_inspection_is_over_once_it_said_its_worker_is_gone_or_its_time_is_up() {
        assert!(!over(1000, false, false, 1000 + TIME_LIMIT - 1));
        assert!(over(1000, false, false, 1000 + TIME_LIMIT));
        assert!(over(1000, true, false, 1001));
        assert!(over(1000, false, true, 1001));
    }

    #[test]
    fn what_an_inspection_did_and_put_on_the_train_is_said_in_one_line() {
        let fixed = Finding {
            said: "four gates\n side by side".into(),
            did: vec!["killed 4242, an orphaned `cargo test`".into()],
            prs: vec![],
        };
        assert_eq!(
            fixed.line(),
            "four gates side by side · did: killed 4242, an orphaned `cargo test`"
        );
        let changed = Finding { prs: vec![451, 452], ..fixed.clone() };
        let line = changed.line();
        assert!(line.ends_with(" · on the base branch: #451, #452"), "{line}");
        // As versions before kept it: what it found, or what it left, and no PR.
        let kept: Finding = serde_json::from_str(r#"{"found": "heavy", "said": "x"}"#).unwrap();
        assert_eq!((kept.said.as_str(), kept.did.len(), kept.prs.len()), ("x", 0, 0));
        let left: Finding = serde_json::from_str(r#"{"said": "x", "left": ["y"]}"#).unwrap();
        assert_eq!(left.line(), "x");
    }

    #[test]
    fn what_is_kept_is_what_cannot_be_rebuilt_under_the_home_folder() {
        assert!(kept(".cache/huggingface/hub/models--x"));
        assert!(kept("Downloads") && kept("Downloads/"));
        assert!(kept("Library/Mobile Documents/x"));
        assert!(kept(""), "the home folder itself holds it");
        assert!(kept("Library"), "and so does a directory above it");
        assert!(!kept("Library/Caches/Homebrew"));
        assert!(!kept("DownloadsOld"), "a part, not a prefix");
        assert!(!kept("src/widgets/target"));
        assert!(kept("*") && kept("Down*/x") && kept(".cache/*"), "a pattern may be any");
        assert!(!kept("src/*"));
    }

    #[test]
    fn the_comment_says_what_was_done_and_put_on_the_train_and_is_no_stage_record() {
        let finding = Finding {
            said: "cargo build of 900 crates".into(),
            did: vec!["told #7 to run it through `afkconvoy exclusive`".into()],
            prs: vec![],
        };
        let said = comment("#7", "past 4.0 cores for 120s", "stock-consult", Some(&finding));
        assert!(said.starts_with("<!--convoy:inspection found=said-->\n"), "{said}");
        assert!(said.contains("> cargo build of 900 crates"), "{said}");
        assert!(said.contains("- told #7 to run it through"), "{said}");
        assert!(!said.contains("landing train"), "{said}");
        assert!(!said.contains(crate::stage_record::STAGE_RECORD), "{said}");
        let changed = Finding { prs: vec![451], ..finding };
        let said = comment("#7", "past 4.0 cores for 120s", "stock-consult", Some(&changed));
        assert!(said.contains("on the base branch, which lands it as any other: #451."), "{said}");
        let nothing = comment("#7", "past 4.0 cores for 120s", "stock-consult", None);
        assert!(nothing.contains("found=nothing") && nothing.contains("60 minutes"), "{nothing}");
    }

    #[test]
    fn the_stage_s_consult_inspects_and_nobody_where_it_says_human() {
        use crate::team::tests::one;
        let team = one("\
stages:
  - name: build
    by: dev
    do: implement
    on: { pass: review }
  - name: review
    by: dev
    do: implement
    escalate: { to: human }
    on: { pass: judged }
  - name: judged
    by: dev
    do: implement
    escalate: { to: reviewer, do: rule }
    on: { pass: done }
");
        let flow = &team.workflows[0];
        let at = |name: &str| flow.stages().iter().find(|stage| stage.name == name);
        assert_eq!(inspector(at("build")), Some(Inspector::Stock));
        assert_eq!(inspector(at("review")), None);
        let Some(Inspector::Role(work)) = inspector(at("judged")) else { panic!("a role") };
        assert_eq!(work.skill, "rule");
        // A patrol's worker, or a landing one: at no stage.
        assert_eq!(inspector(None), Some(Inspector::Stock));
    }

    #[test]
    fn the_brief_gives_the_goal_what_was_taken_down_the_authority_and_the_bounds() {
        let fields = Fields {
            worker: "#7",
            issue: Some(7),
            why: "CPU: past 4.0 cores for 120s",
            held: "6.2 cores, 1.5 GiB resident",
            worktree: "/w/a",
            terminal: "term-1",
            evidence: "/w/a/.git/afkconvoy-hot/evidence.md",
            role: "stock-consult",
            skill: "consult",
            instruction: Some("# The stock consult\n\nYou stand where a human would stand.\n"),
            operator: Operator {
                checkout: "/w/inspection",
                branch: "tester/afkconvoy-inspection",
                base_branch: "main",
                afkconvoy: "cd '/w/inspection' && afkconvoy",
            },
        };
        let said = brief(&fields).unwrap();
        assert!(said.contains("- **Hot worker:** #7, on issue #7. Hot for CPU: past"), "{said}");
        assert!(said.contains("# The stock consult"), "{said}");
        assert!(said.contains("## Your goal\n\nThe fleet's work goes on."), "{said}");
        assert!(said.contains("You are the operator on this host"), "{said}");
        assert!(said.contains("No force push, no push to the base branch"), "{said}");
        assert!(said.contains("`~/.cache/huggingface`"), "{said}");
        assert!(
            said.contains("afkconvoy inspected --text-file <file> [--did '<what you did>']..."),
            "{said}"
        );
        assert!(said.contains("You have 60 minutes"), "{said}");
        assert!(said.contains("left as a comment on the hot worker's issue"), "{said}");
        // A change of the repo it makes and lands itself, on no issue.
        assert!(said.contains("`/w/inspection`, the base branch as it stood, on a branch of its own: `tester/afkconvoy-inspection`"), "{said}");
        assert!(
            said.contains("gh pr create --base main --head tester/afkconvoy-inspection"),
            "{said}"
        );
        assert!(said.contains("takes a turn: makes"), "{said}");
        assert!(said.contains("`cd '/w/inspection' && afkconvoy land --inspection`"), "{said}");
        assert!(said.contains("open no issue"), "{said}");
        assert!(said.contains("## A red base branch in an unbound repo"), "{said}");
        assert!(said.contains("`origin/main` alone in a clean temporary checkout"), "{said}");
        assert!(said.contains("Comment your evidence\non the existing issue"), "{said}");
        assert!(said.contains("`cd '/w/inspection' && afkconvoy lock --issue <n>`"), "{said}");
        assert!(said.contains("Stop running gates\nwhile the lock stands"), "{said}");
        // Not a list of remedies, each mapped from a word it found, nor an
        // issue of a change for the fleet's workers.
        for word in ["`wrong`", "`heavy`", "`not-its`", "--found", "--left", "an issue of"] {
            assert!(!said.contains(word), "{word}: {said}");
        }
        let patrol = brief(&Fields { issue: None, instruction: None, ..fields }).unwrap();
        assert!(patrol.contains("- **Hot worker:** #7. Hot for"), "{patrol}");
        assert!(!patrol.contains("stock consult"), "{patrol}");
    }
}
