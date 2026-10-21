//! An inspection (docs/adr/0019): a worker in the role of the stage's consult
//! — the stock consult where the stage names none, and in an unbound repo —
//! started on a hot worker to find out what it takes so much for, and why.
//! Who inspects, which hot worker is inspected next, what an inspection is
//! told and what is typed and left of what it found are decided here; taking
//! down the evidence, starting the inspection and ending it are the effect
//! layer's.

use serde::{Deserialize, Serialize};

use crate::host_inspection::Item;
use crate::prompt::fill;
use crate::record;
use crate::team::{Consult, Stage, Work};

/// The variable an inspection's environment names the hot worker it
/// inspects in: the guard holds a worker with it as a consult.
pub const INSPECTION_ENV: &str = "AFKCONVOY_INSPECTION";

/// How long, in seconds, an inspection is given: what it found by then is
/// what is typed.
pub const TIME_LIMIT: u64 = 900;

/// The name an inspection's checkout is created under: one a repo, for one
/// inspection runs on its host at a time.
pub const CHECKOUT: &str = "afkconvoy-inspection";

/// The word of the marker of an inspection's comment: no stage record, and
/// nothing derives the stage an issue is at from it (docs/adr/0010).
const MARKER: &str = "convoy:inspection";

/// What an inspection found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Found {
    /// The work is wrong: the hot worker is told what and where, and to fix
    /// it before going on.
    Wrong,
    /// The work is that heavy: the hot worker is told so, and to run the
    /// heavy command through `afkconvoy exclusive` from now on.
    Heavy,
    /// What takes so much is not the worker's — of a host inspection, not
    /// this fleet's: it is told nothing.
    NotIts,
    /// Of a host inspection: something on the disk can be rebuilt. The
    /// inspection frees it itself, as its `afkconvoy inspected` is told.
    Reclaimable,
    /// Of a host inspection: the fleet's own shape outgrows the host. The
    /// fleet opens one issue saying so.
    Fleet,
}

impl Found {
    pub const ALL: [Found; 5] =
        [Found::Wrong, Found::Heavy, Found::NotIts, Found::Reclaimable, Found::Fleet];

    /// How the command that declares it, and the log, say it.
    pub fn word(self) -> &'static str {
        match self {
            Found::Wrong => "wrong",
            Found::Heavy => "heavy",
            Found::NotIts => "not-its",
            Found::Reclaimable => "reclaimable",
            Found::Fleet => "fleet",
        }
    }

    /// Whether an inspection of `kind` finds it.
    pub fn of(self, kind: Kind) -> bool {
        kind.found().contains(&self)
    }

    pub fn read(word: &str) -> Option<Found> {
        Found::ALL.into_iter().find(|found| found.word() == word)
    }
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
    /// What an inspection of this kind finds, in its own words.
    pub fn found(self) -> [Found; 3] {
        match self {
            Kind::Worker => [Found::Wrong, Found::Heavy, Found::NotIts],
            Kind::Host => [Found::Reclaimable, Found::Fleet, Found::NotIts],
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Kind::Worker => "worker",
            Kind::Host => "host",
        }
    }
}

/// What an inspection found, and what it says of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub found: Found,
    /// What, where and why, in its own words.
    pub said: String,
    /// Of a host inspection: what it freed, and what it named for a human.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reclaimed: Vec<Item>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub named: Vec<Item>,
}

impl Finding {
    /// `found`, as `said`, which freed and named nothing.
    pub fn new(found: Found, said: &str) -> Finding {
        Finding { found, said: said.to_string(), reclaimed: vec![], named: vec![] }
    }

    /// The line typed at the hot worker; none where what takes so much is
    /// not its own.
    pub fn typed(&self) -> Option<String> {
        let said = self.said.split_whitespace().collect::<Vec<_>>().join(" ");
        match self.found {
            Found::Wrong => Some(format!(
                "afkconvoy: an inspection of what your processes take of this host found the \
                 work wrong: {said} Fix that before you go on."
            )),
            Found::Heavy => Some(format!(
                "afkconvoy: an inspection of what your processes take of this host found the \
                 work that heavy: {said} From now on run the heavy command through \
                 `afkconvoy exclusive -- <command>`, so that it runs alone on the host."
            )),
            Found::NotIts | Found::Reclaimable | Found::Fleet => None,
        }
    }
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
/// what it found, its worker is gone, or its time is up.
pub fn over(started: u64, found: bool, gone: bool, now: u64) -> bool {
    found || gone || now.saturating_sub(started) >= TIME_LIMIT
}

/// The comment left on the hot worker's issue once its inspection is over:
/// what it found and why, or that it found nothing in time. A marker of its
/// own: it is no stage record.
pub fn comment(worker: &str, why: &str, by: &str, finding: Option<&Finding>) -> String {
    let found = finding.map_or("nothing", |finding| finding.found.word());
    let marker = record::marker(MARKER, &[("found", found)]);
    let head = format!(
        "{marker}\n**afkconvoy** — the worker {worker} turned hot ({why}), and was inspected by \
         `{by}`."
    );
    let Some(finding) = finding else {
        return format!(
            "{head} The inspection said nothing in {} minutes; nothing was typed at the \
             worker.\n",
            TIME_LIMIT / 60
        );
    };
    let told = match finding.found {
        Found::Wrong => "The work is wrong. The worker was told what is wrong, and to fix it.",
        Found::Heavy => {
            "The work is that heavy. The worker was told to run the heavy command through \
             `afkconvoy exclusive`."
        }
        Found::NotIts | Found::Reclaimable | Found::Fleet => {
            "What takes so much is not the worker's. Nothing was typed at it."
        }
    };
    format!("{head} {told}\n\n{}\n", quoted(&finding.said))
}

fn quoted(said: &str) -> String {
    said.trim().lines().map(|line| format!("> {line}")).collect::<Vec<_>>().join("\n")
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
    /// The inspection's own checkout of the base branch.
    pub checkout: &'a str,
    pub role: &'a str,
    pub skill: &'a str,
    /// The stock consult's instruction, where nothing else gives it: an
    /// unbound repo's stock consult starts in no directory of its own.
    pub instruction: Option<&'a str>,
    /// The line it declares what it found with: `--found <found> --text-file
    /// <file>` follow it.
    pub inspected_command: &'a str,
}

const BRIEF: &str = "\
You are an afkconvoy **inspection**, started in your role (`{role}`, skill `{skill}`) on a **hot
worker**: one whose processes take more than their share of this host. Nobody reads this terminal:
never wait for an answer.
{instruction}
- **Hot worker:** {worker}{issue_line}. Hot for {why}; at the most it held {held}
- **Its worktree:** `{worktree}`, read-only to you
- **Its terminal:** `{terminal}`. Read it (`orca terminal read --terminal {terminal}`); never type
  at it: the fleet types what you find
- **What was taken down as it turned hot:** `{evidence}` — the process tree with each command
  line, a five-second `sample` of the hottest process, its resident memory over the last minutes,
  the processes that wrote the most, and the largest directories of its worktree
- **Your checkout:** `{checkout}`, the base branch as it stood

## What to find

What takes so much, and why. Look at the evidence, the worker's terminal and its worktree, and at
the processes as they are now (`ps`, `sample`, `du`). It is one of three things:

- `wrong`: the work is wrong — a test that never ends, a leak, a fixture copied a thousand times,
  a temporary directory made and never removed. Say what is wrong and where.
- `heavy`: the work really is that heavy. Say which command is heavy.
- `not-its`: what takes so much is not the worker's — a daemon outside its tree, another fleet's
  gate.

You may read and run anything that changes nothing. Commit nothing, push nothing, and change
nothing on this host: stop no process and delete no file. You hand nothing on: the issue stays at
its stage and the hot worker keeps working.

## Say what you found

You have {minutes} minutes. Write what you found, where and why, to a file, then run this once,
with `wrong`, `heavy` or `not-its`, and stop:
```bash
{inspected_command} --found <found> --text-file <file>
```
The fleet types it at the hot worker — not `not-its` — and leaves it as a comment on the issue.
";

/// The brief an inspection is given.
pub fn brief(fields: &Fields) -> Result<String, String> {
    let issue_line = fields.issue.map(|issue| format!(", on issue #{issue}")).unwrap_or_default();
    let instruction = fields.instruction.map(|text| format!("\n{text}")).unwrap_or_default();
    let template =
        BRIEF.replace("{issue_line}", &issue_line).replace("{instruction}", &instruction);
    fill(
        &template,
        &[
            ("role", fields.role),
            ("skill", fields.skill),
            ("worker", fields.worker),
            ("why", fields.why),
            ("held", fields.held),
            ("worktree", fields.worktree),
            ("terminal", fields.terminal),
            ("evidence", fields.evidence),
            ("checkout", fields.checkout),
            ("minutes", &(TIME_LIMIT / 60).to_string()),
            ("inspected_command", fields.inspected_command),
        ],
    )
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
    fn an_inspection_is_over_once_it_found_its_worker_is_gone_or_its_time_is_up() {
        assert!(!over(1000, false, false, 1000 + TIME_LIMIT - 1));
        assert!(over(1000, false, false, 1000 + TIME_LIMIT));
        assert!(over(1000, true, false, 1001));
        assert!(over(1000, false, true, 1001));
    }

    #[test]
    fn what_was_found_is_typed_at_the_worker_unless_it_is_not_its_own() {
        let finding = |found| Finding::new(found, "the test\n `loops`  forever.");
        let wrong = finding(Found::Wrong).typed().unwrap();
        assert!(
            wrong.contains("found the work wrong: the test `loops` forever. Fix that"),
            "{wrong}"
        );
        assert!(!wrong.contains('\n'), "one line: {wrong}");
        let heavy = finding(Found::Heavy).typed().unwrap();
        assert!(heavy.contains("`afkconvoy exclusive -- <command>`"), "{heavy}");
        assert_eq!(finding(Found::NotIts).typed(), None);
        for found in Found::ALL {
            assert_eq!(Found::read(found.word()), Some(found));
        }
        assert_eq!(Found::read("bogus"), None);
    }

    #[test]
    fn each_kind_finds_in_words_of_its_own() {
        assert!(Found::Wrong.of(Kind::Worker) && !Found::Wrong.of(Kind::Host));
        assert!(Found::Fleet.of(Kind::Host) && !Found::Fleet.of(Kind::Worker));
        assert!(Found::NotIts.of(Kind::Host) && Found::NotIts.of(Kind::Worker));
        let typed = Finding::new(Found::Reclaimable, "x").typed();
        assert_eq!(typed, None);
    }

    #[test]
    fn the_comment_says_what_was_found_and_is_no_stage_record() {
        let finding = Finding::new(Found::Heavy, "cargo build of 900 crates");
        let said = comment("#7", "past 4.0 cores for 120s", "stock-consult", Some(&finding));
        assert!(said.starts_with("<!--convoy:inspection found=heavy-->\n"), "{said}");
        assert!(said.contains("> cargo build of 900 crates"), "{said}");
        assert!(!said.contains(crate::stage_record::STAGE_RECORD), "{said}");
        let nothing = comment("#7", "past 4.0 cores for 120s", "stock-consult", None);
        assert!(nothing.contains("found=nothing") && nothing.contains("15 minutes"), "{nothing}");
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
    fn the_brief_names_the_worker_its_evidence_and_the_command_that_ends_it() {
        let fields = Fields {
            worker: "#7",
            issue: Some(7),
            why: "CPU: past 4.0 cores for 120s",
            held: "6.2 cores, 1.5 GiB resident",
            worktree: "/w/a",
            terminal: "term-1",
            evidence: "/w/a/.git/afkconvoy-hot/evidence.md",
            checkout: "/w/inspection",
            role: "stock-consult",
            skill: "consult",
            instruction: Some("# The stock consult\n\nYou stand where a human would stand.\n"),
            inspected_command: "cd '/w/inspection' && afkconvoy inspected",
        };
        let said = brief(&fields).unwrap();
        assert!(said.contains("- **Hot worker:** #7, on issue #7. Hot for CPU: past"), "{said}");
        assert!(said.contains("# The stock consult"), "{said}");
        assert!(said.contains("afkconvoy inspected --found <found> --text-file <file>"), "{said}");
        assert!(said.contains("You have 15 minutes"), "{said}");
        let patrol = brief(&Fields { issue: None, instruction: None, ..fields }).unwrap();
        assert!(patrol.contains("- **Hot worker:** #7. Hot for"), "{patrol}");
        assert!(!patrol.contains("stock consult"), "{patrol}");
    }
}
