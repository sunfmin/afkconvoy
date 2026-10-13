//! Precedents (docs/adr/0011): a human's ruling on an escalated issue,
//! proposed once as one entry of the team's precedents file. Which rulings
//! are still to be proposed, what a proposal's claim, branch and checkout are
//! named for, how each proposal stands at one moment, what an entry is, and
//! what makes the PR a proposal's worker opened anything else.
//!
//! The fleet proposes and closes; it never merges on the team's repo. A human
//! merging the PR is what accepts a precedent.

use serde::Serialize;

use crate::claims::{Holder, InstanceId, RunClaim};
use crate::record::{self, fleet_number};
use crate::stage_record::{Author, Reading, StageRecord, Stated};
use crate::team::PRECEDENTS;

/// The kind of record the comment carries that says a ruling was proposed.
pub const PROPOSED: &str = "convoy:precedent-proposed";
/// What the claim on a proposal is keyed by, and its branch on the team's
/// repo and its checkout are named: this, then the ruling's tag.
const CLAIM_KEY: &str = "precedent-";
const NAME: &str = "convoy-precedent-";
/// What stands between the ruling and when it applies, in an entry.
const WHEN: &str = " **When:** ";

/// The variable a proposal's worker carries its ruling's issue in: the guard
/// reads from it that the worker is at no stage.
pub const PROPOSAL_ENV: &str = "AFKCONVOY_PROPOSAL";

/// One ruling a human gave an escalated issue: what a proposal is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Ruling {
    pub issue: u64,
    /// The id of the comment that is its stage record.
    pub comment: u64,
}

impl Ruling {
    /// What names everything of its proposal. It ends in no number alone:
    /// orca numbers a worktree whose name was taken with one.
    fn tag(&self) -> String {
        format!("{}-c{}", self.issue, self.comment)
    }

    /// The key of the claim on its proposal (`claims::RunClaim`).
    pub fn claim_key(&self) -> String {
        format!("{CLAIM_KEY}{}", self.tag())
    }

    /// The ruling a run's claim keyed `key` is on the proposal of; `None` for
    /// a claim on anything else.
    pub fn claimed(key: &str) -> Option<Ruling> {
        let (issue, comment) = key.strip_prefix(CLAIM_KEY)?.split_once("-c")?;
        Some(Ruling { issue: fleet_number(issue)?, comment: fleet_number(comment)? })
    }

    /// The branch of the team's repo its proposal is opened from: how the
    /// PR is found again.
    pub fn branch(&self) -> String {
        format!("{NAME}{}", self.tag())
    }

    /// The name its worker's checkout is created under.
    pub fn checkout_name(&self) -> String {
        self.branch()
    }
}

/// Whether `branch` is one a proposal is opened from.
pub fn is_proposal(branch: &str) -> bool {
    branch.starts_with(NAME)
}

/// The rulings of humans among the stage records of issue `issue`, oldest
/// first: `records` with how each read (`stage_record::history`) and the id
/// of its comment. A verdict a human declared for a `by: human` stage is no
/// ruling, and neither is a consult's; a ruling on a stage the issue was not
/// at ruled on nothing.
pub fn rulings(
    issue: u64,
    records: &[StageRecord],
    readings: &[Reading],
    comments: &[u64],
) -> Vec<Ruling> {
    let mut found = vec![];
    for (at, record) in records.iter().enumerate() {
        let ruled = record.by == Author::Human
            && record.stated == Stated::Declared(crate::position::Declared::Ruling)
            && readings.get(at) == Some(&Reading::Verdict);
        if let Some(comment) = comments.get(at).filter(|_| ruled) {
            found.push(Ruling { issue, comment: *comment });
        }
    }
    found
}

/// The comment that says `ruling` was proposed: a worker in the role `role`
/// was started for it. It is written before the worker is, and is what keeps
/// a ruling to one proposal.
pub fn proposed_comment(ruling: Ruling, role: &str, team: &str) -> String {
    let marker = record::marker(PROPOSED, &[("ruling", &ruling.comment.to_string())]);
    format!(
        "{marker}\n**afkconvoy** — a worker in the role `{role}` is started to propose this \
         ruling as a precedent: a PR on {team} that appends one entry to `{PRECEDENTS}`, from the \
         branch `{}`. Merging it is what accepts the precedent; the issue does not wait for it.",
        ruling.branch()
    )
}

/// The comments of the rulings an issue's comments (`bodies`) say were
/// proposed.
pub fn proposed<'a>(bodies: impl IntoIterator<Item = &'a str>) -> Vec<u64> {
    let mut found = vec![];
    for body in bodies {
        let Some(rest) = body.strip_prefix("<!--") else { continue };
        let Some((marker, _)) = rest.split_once("-->") else { continue };
        if let Some(ruling) = record::read(PROPOSED, marker).and_then(|f| f.number("ruling")) {
            found.push(ruling);
        }
    }
    found
}

/// What the effect layer read of a bound repo's proposals.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Seen {
    /// How many proposals the team lets wait for a human at once; `None`
    /// where it names nobody who proposes, and none is started.
    pub max_open: Option<u64>,
    /// The rulings on open issues that follow a workflow that nothing says
    /// were proposed, lowest issue first and oldest first on each.
    pub unproposed: Vec<Ruling>,
    /// The claim on each proposal still going.
    pub claims: Vec<(Ruling, RunClaim)>,
    /// The branch of each open proposal on the team's repo.
    pub open: Vec<String>,
}

/// How one proposal stands: what a tick does about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    /// Nothing holds it back: a tick starts its worker.
    Due,
    /// The open proposals have reached the team's `max_open`.
    Full,
    /// A fleet lock stands: nothing is started.
    Locked,
    /// Its worker, started by me, may still be at it: a tick asks after it.
    Running,
    /// A live peer started its worker.
    PeerRunning,
    /// Its claim is a peer's presumed dead: a tick ends it.
    StaleRun,
}

/// One proposal, made or to be made, as a working set holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    #[serde(flatten)]
    pub ruling: Ruling,
    pub standing: Standing,
    /// The claim on it, while one stands.
    #[serde(skip)]
    pub claim: Option<RunClaim>,
}

/// How each proposal `seen` stands for the fleet instance `me` at `now`, with
/// each instance's heartbeat in `heartbeats`, and `locked` when a fleet lock
/// stands: those still claimed, then the rulings to propose. Of these, as
/// many are due as the team's `max_open` leaves room for beside the proposals
/// open and the ones a live fleet is still at.
pub fn rows(
    seen: &Seen,
    me: Option<&InstanceId>,
    heartbeats: &[(String, u64)],
    now: u64,
    locked: bool,
) -> Vec<Row> {
    let mut rows = vec![];
    // A proposal whose worker opened its PR is counted once.
    let mut waiting: Vec<String> = seen.open.clone();
    for (ruling, claim) in &seen.claims {
        let standing = match claim.holder(me, heartbeats, now) {
            Holder::Mine => Standing::Running,
            Holder::Peer => Standing::PeerRunning,
            Holder::Dead => Standing::StaleRun,
        };
        if standing != Standing::StaleRun && !waiting.contains(&ruling.branch()) {
            waiting.push(ruling.branch());
        }
        rows.push(Row { ruling: *ruling, standing, claim: Some(claim.clone()) });
    }
    let Some(max_open) = seen.max_open else { return rows };
    let mut open = waiting.len() as u64;
    for ruling in &seen.unproposed {
        if seen.claims.iter().any(|(claimed, _)| claimed == ruling) {
            continue;
        }
        let standing = if locked {
            Standing::Locked
        } else if open >= max_open {
            Standing::Full
        } else {
            open = open.saturating_add(1);
            Standing::Due
        };
        rows.push(Row { ruling: *ruling, standing, claim: None });
    }
    rows
}

/// One entry of the precedents file: the ruling in a sentence, when it
/// applies, and a link to where it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry<'a> {
    pub ruling: &'a str,
    pub when: &'a str,
    pub link: &'a str,
}

impl<'a> Entry<'a> {
    /// The one line that is the entry.
    pub fn line(&self) -> String {
        format!("- {}{WHEN}{} ({})", self.ruling, self.when, self.link)
    }

    /// The entry `line` is; `None` when it is none: a part is missing.
    pub fn read(line: &'a str) -> Option<Entry<'a>> {
        let (said, link) = line.strip_prefix("- ")?.strip_suffix(')')?.rsplit_once(" (")?;
        let (ruling, when) = said.split_once(WHEN)?;
        let (ruling, when) = (ruling.trim(), when.trim());
        let linked = !link.is_empty() && !link.contains(char::is_whitespace);
        if ruling.is_empty() || when.is_empty() || !linked {
            return None;
        }
        Some(Entry { ruling, when, link })
    }
}

/// The form of an entry, as a worker and a human are told it.
pub fn entry_form(link: &str) -> String {
    Entry { ruling: "<the ruling, in a sentence>", when: "<when it applies>", link }.line()
}

/// One line of a file's part of a diff, past its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line<'a> {
    Added(&'a str),
    Removed(&'a str),
    Kept,
}

/// The lines of the hunks in `part` — one file's part of a diff — in order. A
/// file that ended in no newline has its last line rewritten by whatever is
/// appended to it: that line reads as kept. A line of a hunk begins with what
/// it is, so an empty one is none — it is what a diff was handed over with.
fn hunk_lines(part: &str) -> Vec<Line<'_>> {
    let mut lines: Vec<Line> = vec![];
    let mut in_hunk = false;
    // The line a `\ No newline at end of file` took the newline of.
    let mut unended: Option<&str> = None;
    for line in part.lines() {
        if line.starts_with("@@") {
            in_hunk = true;
        } else if !in_hunk || line.is_empty() {
            continue;
        } else if line.starts_with('\\') {
            if let Some(Line::Removed(text)) = lines.last().copied() {
                unended = Some(text);
            }
        } else if let Some(text) = line.strip_prefix('+') {
            if unended == Some(text) {
                lines.pop();
                lines.push(Line::Kept);
            } else {
                lines.push(Line::Added(text));
            }
            unended = None;
        } else if let Some(text) = line.strip_prefix('-') {
            lines.push(Line::Removed(text));
        } else {
            lines.push(Line::Kept);
        }
    }
    lines
}

/// Why the PR whose diff is `diff` — as `git diff` writes one — is no
/// proposal of the ruling at `link`, when it is none: it changes anything but
/// the precedents file, or does anything there but append one entry that
/// links to the ruling.
pub fn refused(diff: &str, link: &str) -> Option<String> {
    let form = format!("an entry is one line, `{}`", entry_form(link));
    // A header names the file twice: once is a rename, or another file.
    let precedents = format!("a/{PRECEDENTS} b/{PRECEDENTS}");
    let mut own: Option<&str> = None;
    let mut others: Vec<String> = vec![];
    for part in diff.split("diff --git ").skip(1) {
        let header = part.lines().next().unwrap_or("");
        if header == precedents {
            own = Some(part);
        } else {
            let file = header.rsplit_once(" b/").map_or(header, |(_, to)| to);
            others.push(format!("`{file}`"));
        }
    }
    if !others.is_empty() {
        return Some(format!(
            "it changes {}: a proposal changes `{PRECEDENTS}` and nothing else",
            others.join(", ")
        ));
    }
    let Some(part) = own else {
        return Some(format!("it changes nothing: a proposal appends one entry to `{PRECEDENTS}`"));
    };
    let lines = hunk_lines(part);
    let mut added: Vec<&str> = vec![];
    for line in &lines {
        match line {
            Line::Removed(text) => {
                return Some(format!(
                    "it removes or rewrites a line of `{PRECEDENTS}` (`{}`): a proposal only \
                     appends",
                    text.trim()
                ));
            }
            Line::Added(text) => added.push(text),
            Line::Kept => {}
        }
    }
    let [entry] = added.as_slice() else {
        return Some(format!(
            "it adds {} lines to `{PRECEDENTS}`: a proposal appends one entry, and {form}",
            added.len()
        ));
    };
    if lines.last() != Some(&Line::Added(entry)) {
        return Some(format!(
            "it adds its line above the end of `{PRECEDENTS}`: a proposal appends its entry"
        ));
    }
    match Entry::read(entry) {
        None => Some(format!("the line it appends is no entry: {form}")),
        Some(read) if read.link != link => Some(format!(
            "its entry links to {}, not to the ruling it was started for: {form}",
            read.link
        )),
        Some(_) => None,
    }
}

/// The comment a PR that is no proposal is closed with.
pub fn closing_comment(reason: &str) -> String {
    format!(
        "**afkconvoy** — closed: this PR was opened to propose a precedent, and {reason}. Nothing \
         was merged, and the ruling is not proposed again: write the entry by hand if it should \
         be kept."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claims::Owner;
    use crate::lease::LEASE;
    use crate::position::Declared;

    const LINK: &str = "https://github.com/acme/app/issues/7#issuecomment-2001";

    fn ruling(issue: u64, comment: u64) -> Ruling {
        Ruling { issue, comment }
    }

    fn claim(of: Ruling, instance: &str) -> (Ruling, RunClaim) {
        let owner = Owner { instance: instance.into(), host: "h".into(), ts: 0 };
        (of, RunClaim { key: of.claim_key(), owner: Some(owner), sha: "c".into(), over: false })
    }

    fn standing(seen: &Seen, locked: bool) -> Vec<(u64, Standing)> {
        let me = InstanceId::parse("me").unwrap();
        let rows = rows(seen, Some(&me), &[("peer".into(), 1000)], 1000, locked);
        rows.iter().map(|row| (row.ruling.comment, row.standing)).collect()
    }

    #[test]
    fn everything_of_a_proposal_is_named_for_its_ruling() {
        let of = ruling(7, 2001);
        assert_eq!(of.claim_key(), "precedent-7-c2001");
        assert_eq!(Ruling::claimed(&of.claim_key()), Some(of));
        assert_eq!(of.branch(), "convoy-precedent-7-c2001");
        assert!(is_proposal(&of.branch()) && !is_proposal("issue-7"));
        // Neither a patrol's claim nor a name a hand wrote reads as one.
        for not in ["patrol-sweep", "precedent-7", "precedent-7-c", "precedent-07-c1", "7"] {
            assert_eq!(Ruling::claimed(not), None, "{not}");
        }
        // orca's `-<k>` on a name taken never reads as another ruling's checkout.
        assert!(!crate::worker::is_worktree_named("convoy-precedent-7-c20", &of.checkout_name()));
        assert!(!crate::worker::is_worktree_named(
            &ruling(7, 20011).checkout_name(),
            &of.checkout_name()
        ));
    }

    #[test]
    fn a_human_s_ruling_is_one_and_a_human_s_verdict_or_a_consult_s_ruling_is_none() {
        let record = |by: Author, declared: Declared| StageRecord {
            stage: "build".to_string(),
            stated: Stated::Declared(declared),
            by,
            blocked_by: vec![],
            text: "said".to_string(),
        };
        let consult = Author::Worker {
            consult: true,
            claim: "c".into(),
            role_commit: "r".into(),
            team_commit: "t".into(),
        };
        let records = [
            record(Author::Human, Declared::Ruling),
            record(Author::Human, Declared::Pass),
            record(consult, Declared::Ruling),
            record(Author::Human, Declared::Ruling),
            record(Author::Human, Declared::Ruling),
        ];
        let sent = StageRecord { stated: Stated::Sent, ..records[0].clone() };
        let records = [&records[..], &[sent]].concat();
        use Reading::*;
        let readings = [Verdict, Verdict, Verdict, OtherStage, Verdict, Sent];
        let found = rulings(7, &records, &readings, &[11, 12, 13, 14, 15, 16]);
        assert_eq!(found, [ruling(7, 11), ruling(7, 15)]);
    }

    #[test]
    fn a_ruling_proposed_is_marked_on_its_issue() {
        let comment = proposed_comment(ruling(7, 2001), "lead", "acme/team");
        assert!(comment.starts_with("<!--convoy:precedent-proposed ruling=2001-->\n"), "{comment}");
        assert!(comment.contains("the branch `convoy-precedent-7-c2001`"), "{comment}");
        let other = proposed_comment(ruling(7, 2009), "lead", "acme/team");
        let bodies = [comment.as_str(), "chat", "<!--convoy:stage stage=x-->", other.as_str()];
        assert_eq!(proposed(bodies), [2001, 2009]);
        // A marker quoted further down a comment marks nothing.
        assert_eq!(proposed([format!("> {comment}").as_str()]), Vec::<u64>::new());
    }

    #[test]
    fn a_ruling_is_due_while_the_open_proposals_leave_room() {
        let unproposed = vec![ruling(7, 1), ruling(7, 2), ruling(9, 3)];
        let seen = Seen { max_open: Some(2), unproposed, ..Seen::default() };
        use Standing::*;
        assert_eq!(standing(&seen, false), [(1, Due), (2, Due), (3, Full)]);
        assert_eq!(standing(&seen, true), [(1, Locked), (2, Locked), (3, Locked)]);
        let one_open = Seen { open: vec!["convoy-precedent-4-c9".into()], ..seen.clone() };
        assert_eq!(standing(&one_open, false), [(1, Due), (2, Full), (3, Full)]);
        let none = Seen { max_open: Some(0), ..seen.clone() };
        assert_eq!(standing(&none, false), [(1, Full), (2, Full), (3, Full)]);
        // A team that names nobody who proposes has none proposed.
        assert_eq!(standing(&Seen { max_open: None, ..seen }, false), []);
    }

    #[test]
    fn a_proposal_still_going_stands_by_whose_it_is_and_takes_room() {
        let (mine, peers, dead) = (ruling(7, 1), ruling(7, 2), ruling(8, 3));
        let seen = Seen {
            max_open: Some(3),
            unproposed: vec![mine, ruling(9, 4), ruling(9, 5)],
            claims: vec![claim(mine, "me"), claim(peers, "peer"), claim(dead, "gone")],
            // Mine has opened its PR: it is one proposal, not two.
            open: vec![mine.branch()],
        };
        use Standing::*;
        assert_eq!(
            standing(&seen, false),
            [(1, Running), (2, PeerRunning), (3, StaleRun), (4, Due), (5, Full)]
        );
        // What is going is ended under a lock too, and whoever proposes or not.
        let ending = Seen { max_open: None, ..seen };
        assert_eq!(standing(&ending, true), [(1, Running), (2, PeerRunning), (3, StaleRun)]);
        let me = InstanceId::parse("me").unwrap();
        let late = rows(&ending, Some(&me), &[("peer".into(), 1000)], 1001 + LEASE, false);
        assert_eq!(late[1].standing, StaleRun);
    }

    #[test]
    fn an_entry_is_the_ruling_when_it_applies_and_a_link() {
        let entry = Entry {
            ruling: "Retry a network test once before calling it red.",
            when: "the failure is a timeout (not a refusal) against a host we do not run.",
            link: LINK,
        };
        let line = entry.line();
        assert_eq!(
            line,
            format!(
                "- Retry a network test once before calling it red. **When:** the failure is a \
                 timeout (not a refusal) against a host we do not run. ({LINK})"
            )
        );
        assert_eq!(Entry::read(&line), Some(entry));
        for not in [
            "Retry once. **When:** always. (https://x)",
            "- Retry once. (https://x)",
            "- Retry once. **When:** always.",
            "- Retry once. **When:** always. (see the issue)",
            "- **When:** always. (https://x)",
            "- Retry once. **When:**  (https://x)",
        ] {
            assert_eq!(Entry::read(not), None, "{not}");
        }
        assert!(Entry::read(&entry_form(LINK)).is_some());
    }

    /// The diff of a PR that changes `precedents.md` by `hunk`.
    fn diff(hunk: &str) -> String {
        format!(
            "diff --git a/precedents.md b/precedents.md\nindex 1a2b3c4..5d6e7f8 100644\n--- \
             a/precedents.md\n+++ b/precedents.md\n{hunk}"
        )
    }

    fn entry() -> String {
        Entry { ruling: "Retry once.", when: "a host we do not run times out.", link: LINK }.line()
    }

    #[test]
    fn a_pr_that_appends_one_entry_to_the_precedents_file_is_a_proposal() {
        let appended = diff(&format!(
            "@@ -1,2 +1,3 @@\n # Precedents\n - Old. **When:** x. (u)\n+{}\n",
            entry()
        ));
        assert_eq!(refused(&appended, LINK), None);
        // To a file that ended in no newline: its last line is rewritten, and reads as kept.
        let unended = diff(&format!(
            "@@ -1 +1,2 @@\n-# Precedents\n\\ No newline at end of file\n+# Precedents\n+{}\n",
            entry()
        ));
        assert_eq!(refused(&unended, LINK), None);
        // The file's first line: the team kept none before.
        let created = format!(
            "diff --git a/precedents.md b/precedents.md\nnew file mode 100644\nindex \
             0000000..5d6e7f8\n--- /dev/null\n+++ b/precedents.md\n@@ -0,0 +1 @@\n+{}\n",
            entry()
        );
        assert_eq!(refused(&created, LINK), None);
    }

    #[test]
    fn a_pr_that_changes_anything_else_or_does_anything_but_append_one_entry_is_refused() {
        let entry = entry();
        let why = |diff: &str| refused(diff, LINK).expect("refused");
        let beside = format!(
            "{}diff --git a/team.md b/team.md\nindex 1..2 100644\n--- a/team.md\n+++ \
             b/team.md\n@@ -1 +1,2 @@\n roles:\n+  x: a/b\n",
            diff(&format!("@@ -1 +1,2 @@\n # Precedents\n+{entry}\n"))
        );
        assert!(why(&beside).starts_with("it changes `team.md`: a proposal changes"));
        let renamed = "diff --git a/precedents.md b/rules.md\nsimilarity index 100%\nrename from \
                       precedents.md\nrename to rules.md\n";
        assert!(why(renamed).starts_with("it changes `rules.md`"), "{}", why(renamed));
        assert!(why("").starts_with("it changes nothing"));
        let rewrites =
            diff(&format!("@@ -1,2 +1,2 @@\n # Precedents\n-- Old. **When:** x. (u)\n+{entry}\n"));
        assert!(why(&rewrites).contains("removes or rewrites a line"), "{}", why(&rewrites));
        let two = diff(&format!("@@ -1 +1,3 @@\n # Precedents\n+{entry}\n+{entry}\n"));
        assert!(why(&two).starts_with("it adds 2 lines"), "{}", why(&two));
        let spaced = diff(&format!("@@ -1 +1,3 @@\n # Precedents\n+\n+{entry}\n"));
        assert!(why(&spaced).starts_with("it adds 2 lines"));
        let mode = "diff --git a/precedents.md b/precedents.md\nold mode 100644\nnew mode 100755\n";
        assert!(why(mode).starts_with("it adds 0 lines"));
        let inserted =
            diff(&format!("@@ -1,2 +1,3 @@\n # Precedents\n+{entry}\n - Old. **When:** x. (u)\n"));
        assert!(why(&inserted).contains("above the end"), "{}", why(&inserted));
        let prose = diff("@@ -1 +1,2 @@\n # Precedents\n+Always retry.\n");
        assert!(why(&prose).starts_with("the line it appends is no entry"));
        let elsewhere = entry.replace("issues/7", "issues/8");
        let other = diff(&format!("@@ -1 +1,2 @@\n # Precedents\n+{elsewhere}\n"));
        assert!(why(&other).contains("not to the ruling it was started for"), "{}", why(&other));
        let comment = closing_comment(&why(&prose));
        assert!(comment.contains("Nothing was merged") && comment.contains(LINK), "{comment}");
    }
}
