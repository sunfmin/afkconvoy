//! A PR's merge identity, recoverable from base history, and worktree builds.
use crate::config::RepoName;
use crate::record::canonical_number;
use crate::tracker::Tracker;
use flux_rs::attrs::*;
/// Whose PR a merge commit names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Of {
    /// The issue of the fleet's tracker it closes: a worker's PR.
    Issue(u64),
    /// An issue of another tracker, as the commit names it: the PR of a
    /// fleet of the repo that works another tracker, whose issue this fleet
    /// leaves alone (docs/adr/0027).
    Elsewhere(String),
    /// The watch on the host so named, which made a change of the repo
    /// itself, on no issue (docs/adr/0026).
    Watch(String),
    /// The cut on the host so named, whose PR sets the version it makes, on
    /// no issue (docs/adr/0028).
    Cut(String),
}

/// The line of a join commit that names a watch's PR, the host after it.
const BY_WATCH: &str = "By a watch on ";
/// The line of a join commit that names a cut's PR, the host after it.
const BY_CUT: &str = "By a cut on ";

/// One PR's recorded landing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandedPr {
    pub of: Of,
    pub pr: u64,
    /// The commit that landed.
    pub commit: String,
    /// The PR's head it merged: the commit's second parent. Empty for a cut's
    /// landed commit, which has one parent; the commit that set the version
    /// stays on the cut's branch and is not recorded here.
    pub head: String,
}

impl LandedPr {
    /// The issue of the fleet's tracker the PR closes: none of an
    /// watch's or a cut's, nor of another tracker's issue.
    pub fn issue(&self) -> Option<u64> {
        match self.of {
            Of::Issue(issue) => Some(issue),
            Of::Elsewhere(_) | Of::Watch(_) | Of::Cut(_) => None,
        }
    }

    /// Whether it is a watch's PR.
    pub fn is_watch(&self) -> bool {
        matches!(self.of, Of::Watch(_))
    }
}

/// The line of a join commit that names the issue its PR closes, the issue
/// after it.
const CLOSES: &str = "Closes ";

/// The message of a PR's landing commit, read back by `read_landings`. An issue is named as the
/// repo names one of `tracker`'s (`Tracker::issue`).
pub fn merge_message(title: &str, pr: u64, of: &Of, tracker: &Tracker) -> String {
    let title = title.trim();
    let title = if title.is_empty() { format!("PR {pr}") } else { title.to_string() };
    match of {
        Of::Issue(issue) => format!("{title} (#{pr})\n\n{CLOSES}{}\n", tracker.issue(*issue)),
        Of::Elsewhere(named) => format!("{title} (#{pr})\n\n{CLOSES}{named}\n"),
        Of::Watch(host) => format!("{title} (#{pr})\n\n{BY_WATCH}{host}\n"),
        Of::Cut(host) => format!("{title} (#{pr})\n\n{BY_CUT}{host}\n"),
    }
}

/// The line `git log --grep -E -i` finds the join commits of issue `number`
/// of `tracker` by: a narrowing alone, which `read_landings` reads again.
pub fn closes_grep(tracker: &Tracker, number: u64) -> String {
    let named = tracker.issue(number);
    let escaped: String = named
        .chars()
        .flat_map(|c| match ".^$*+?()[]{}|\\".contains(c) {
            true => vec!['\\', c],
            false => vec![c],
        })
        .collect();
    format!("^{CLOSES}{escaped}$")
}

/// One commit as `git log` gives it: its sha, its parents, its whole message.
pub struct Logged<'a> {
    pub sha: &'a str,
    pub parents: &'a str,
    pub message: &'a str,
}

/// The joinings among `commits`, in the order given, as the fleet whose
/// tracker is `tracker` reads them. A joining is a merge commit whose subject
/// ends ` (#<pr>)` and whose message has a line `Closes <issue>` — of an
/// issue of `tracker`'s, `Of::Issue`, and of any other, `Of::Elsewhere` — or,
/// of a watch's PR, `By a watch on <host>`, and of a cut's,
/// `By a cut on <host>`. A cut may instead have one parent: the same subject
/// and a `By a cut on <host>` line. A commit with one parent and no such line
/// is never one — a fix titled `… (#12)` is a fix. Issue turns and watches
/// stay merge commits.
pub fn read_landings(commits: &[Logged], tracker: &Tracker) -> Vec<LandedPr> {
    let mut joins = vec![];
    for commit in commits {
        let parents: Vec<&str> = commit.parents.split_whitespace().collect();
        let subject = commit.message.lines().next().unwrap_or("");
        let pr = subject
            .strip_suffix(')')
            .and_then(|rest| rest.rsplit_once(" (#"))
            .and_then(|(_, number)| canonical_number(number));
        let closes = commit.message.lines().find_map(|line| line.strip_prefix(CLOSES));
        let issue = closes.map(str::trim).and_then(|named| match tracker.issue_named(named) {
            Some(issue) => Some(Of::Issue(issue)),
            None => names_an_issue(named).then(|| Of::Elsewhere(named.to_string())),
        });
        let by = |by: &str| {
            let host = commit.message.lines().find_map(|line| line.strip_prefix(by));
            host.map(str::trim).filter(|host| !host.is_empty()).map(str::to_string)
        };
        let of = issue
            .or_else(|| by(BY_WATCH).map(Of::Watch))
            .or_else(|| by("By an inspection on ").map(Of::Watch))
            .or_else(|| by(BY_CUT).map(Of::Cut));
        let Some(pr) = pr else { continue };
        let Some(of) = of else { continue };
        // A merge commit's second parent is the PR head. A cut's one-parent
        // landing has none: the commit that set the version stays off the base.
        let head = match (parents.as_slice(), &of) {
            ([_, head], _) => (*head).to_string(),
            ([_], Of::Cut(_)) => String::new(),
            _ => continue,
        };
        joins.push(LandedPr { of, pr, commit: commit.sha.to_string(), head });
    }
    joins
}

/// Whether `named` names an issue as a join commit does: `#7`, or
/// `owner/repo#7`.
fn names_an_issue(named: &str) -> bool {
    let Some((repo, number)) = named.rsplit_once('#') else { return false };
    canonical_number(number).is_some() && (repo.is_empty() || RepoName::read(repo).is_some())
}

/// A worker's worktree has outgrown its build when what it built is more
/// than this many times its fresh build…
pub const OUTGROWN_TIMES: u64 = 3;

/// …and more than this many bytes, 1 GiB: a small repo's worktree is not cut
/// again and again.
pub const OUTGROWN_FLOOR: u64 = 1_073_741_824;

/// What the last landing in a worker's worktree measured of it, in bytes:
/// everything git ignores there, and its fresh build when one is on record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Built {
    pub built: u64,
    pub fresh: Option<u64>,
}

impl Built {
    /// Has the worktree outgrown its build (`outgrown`)?
    pub fn outgrown(self) -> bool {
        outgrown(self.built, self.fresh)
    }
}

/// Is `built` more than `OUTGROWN_TIMES` times `fresh`? Asked of `built - 1`
/// divided, for the product may not fit.
#[spec(fn(built: u64{built > 0}, fresh: u64) -> bool[built > fresh * OUTGROWN_TIMES])]
fn past_fresh(built: u64, fresh: u64) -> bool {
    fresh <= (built - 1) / OUTGROWN_TIMES
}

/// Has a worker's worktree outgrown its build — `built` more than
/// `OUTGROWN_TIMES` its `fresh` build and more than `OUTGROWN_FLOOR`? With no
/// fresh build on record (a worktree built in before measuring began), more
/// than the floor is enough: the worktree cut again records one.
#[spec(fn(built: u64, fresh: Option<u64>) -> bool{v: v => built > OUTGROWN_FLOOR})]
pub fn outgrown(built: u64, fresh: Option<u64>) -> bool {
    if built <= OUTGROWN_FLOOR {
        return false;
    }
    match fresh {
        Some(fresh) => past_fresh(built, fresh),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_merge_message_reads_back() {
        let message = merge_message("  Add plan (really) ", 31, &Of::Issue(7), &Tracker::Own);
        assert_eq!(message, "Add plan (really) (#31)\n\nCloses #7\n");
        let commits = [
            Logged { sha: "m1", parents: "t0 h31", message: &message },
            Logged { sha: "f1", parents: "m1", message: "fix the train (#31)\n\nCloses #7\n" },
            Logged { sha: "m2", parents: "f1 h40", message: "Merge branch 'main'" },
            Logged {
                sha: "m3",
                parents: "m2 h32",
                message: &merge_message("", 32, &Of::Issue(8), &Tracker::Own),
            },
        ];
        let joins = read_landings(&commits, &Tracker::Own);
        assert_eq!(joins.len(), 2, "a one-parent commit and an ordinary merge are no joining");
        let of = Of::Issue(7);
        assert_eq!(joins[0], LandedPr { of, pr: 31, commit: "m1".into(), head: "h31".into() });
        assert_eq!((joins[1].pr, joins[1].issue(), joins[1].head.as_str()), (32, Some(8), "h32"));
    }

    #[test]
    fn a_fleet_reads_the_joins_of_its_own_trackers_issues_and_leaves_the_rest_elsewhere() {
        let tracker = |name| Tracker::Other {
            tracker: RepoName::read(name).unwrap(),
            repo: RepoName::read("acme/dev").unwrap(),
        };
        let mine = merge_message("Add plan", 31, &Of::Issue(7), &tracker("acme/tracker"));
        assert_eq!(mine, "Add plan (#31)\n\nCloses acme/tracker#7\n");
        let theirs = merge_message("Add it", 32, &Of::Issue(7), &tracker("bob/notes"));
        let own = merge_message("Add own", 33, &Of::Issue(7), &Tracker::Own);
        let commits = [
            Logged { sha: "m1", parents: "t0 h31", message: &mine },
            Logged { sha: "m2", parents: "m1 h32", message: &theirs },
            Logged { sha: "m3", parents: "m2 h33", message: &own },
            Logged { sha: "m4", parents: "m3 h34", message: "Odd (#34)\n\nCloses #x\n" },
        ];
        let read = |tracker: &Tracker| -> Vec<Of> {
            read_landings(&commits, tracker).into_iter().map(|join| join.of).collect()
        };
        let elsewhere = |named: &str| Of::Elsewhere(named.to_string());
        assert_eq!(
            read(&tracker("ACME/tracker")),
            [Of::Issue(7), elsewhere("bob/notes#7"), elsewhere("#7")]
        );
        assert_eq!(
            read(&Tracker::Own),
            [elsewhere("acme/tracker#7"), elsewhere("bob/notes#7"), Of::Issue(7)]
        );
        assert_eq!(closes_grep(&tracker("acme/a.b"), 7), "^Closes acme/a\\.b#7$");
    }

    #[test]
    fn an_watch_s_pr_joins_on_no_issue_and_reads_back_by_its_host() {
        let host = Of::Watch("felixmbp14".into());
        let message = merge_message("Queue the gates", 45, &host, &Tracker::Own);
        assert_eq!(message, "Queue the gates (#45)\n\nBy a watch on felixmbp14\n");
        let commits = [
            Logged { sha: "m1", parents: "t0 h45", message: &message },
            Logged { sha: "m2", parents: "m1 h46", message: "Odd (#46)\n\nBy a watch on \n" },
        ];
        let joins = read_landings(&commits, &Tracker::Own);
        assert_eq!(joins.len(), 1, "a line that names no host names no watch");
        assert_eq!(
            joins[0],
            LandedPr { of: host, pr: 45, commit: "m1".into(), head: "h45".into() }
        );
        assert_eq!(joins[0].issue(), None);
    }

    #[test]
    fn a_historical_inspection_landing_is_read_as_a_watch() {
        let commits = [Logged {
            sha: "m1",
            parents: "b h",
            message: "Operator fix (#45)\n\nBy an inspection on h1\n",
        }];
        let landed = read_landings(&commits, &Tracker::Own);
        assert_eq!(landed[0].of, Of::Watch("h1".into()));
        assert_eq!(landed[0].issue(), None);
    }

    #[test]
    fn a_cut_s_pr_joins_on_no_issue_and_reads_back_by_its_host() {
        let cut = Of::Cut("felixmbp14".into());
        let message = merge_message("afkconvoy 0.5.17", 470, &cut, &Tracker::Own);
        assert_eq!(message, "afkconvoy 0.5.17 (#470)\n\nBy a cut on felixmbp14\n");
        let commits = [Logged { sha: "m1", parents: "t0 h470", message: &message }];
        let joins = read_landings(&commits, &Tracker::Own);
        assert_eq!(
            joins,
            [LandedPr { of: cut, pr: 470, commit: "m1".into(), head: "h470".into() }]
        );
        assert_eq!(joins[0].issue(), None);
    }

    #[test]
    fn a_cut_with_one_parent_is_a_landing_and_a_fix_with_one_parent_is_not() {
        let cut = Of::Cut("felixmbp14".into());
        let message = merge_message("afkconvoy 0.5.37", 600, &cut, &Tracker::Own);
        let commits = [
            Logged { sha: "c1", parents: "base", message: &message },
            Logged { sha: "f1", parents: "c1", message: "afkconvoy 0.5.37 (#600)\n" },
            Logged { sha: "f2", parents: "f1", message: "fix the manifest (#12)\n\nCloses #12\n" },
            Logged {
                sha: "w1",
                parents: "f2",
                message: "Queue the gates (#45)\n\nBy a watch on felixmbp14\n",
            },
            Logged {
                sha: "old",
                parents: "t0 h470",
                message: &merge_message("afkconvoy 0.5.17", 470, &cut, &Tracker::Own),
            },
        ];
        let joins = read_landings(&commits, &Tracker::Own);
        assert_eq!(
            joins.len(),
            2,
            "a one-parent fix or watch is no landing; an old merge still is"
        );
        assert_eq!(
            joins[0],
            LandedPr { of: cut.clone(), pr: 600, commit: "c1".into(), head: String::new() }
        );
        assert_eq!(joins[1].pr, 470);
        assert_eq!(joins[1].head, "h470");
        assert!(matches!(joins[1].of, Of::Cut(_)));
    }

    #[test]
    fn a_worktree_has_outgrown_its_build_past_three_times_it_and_the_floor() {
        const GIB: u64 = OUTGROWN_FLOOR;
        assert!(outgrown(4 * GIB, Some(GIB)));
        assert!(!outgrown(3 * GIB, Some(GIB)), "three times is within");
        assert!(outgrown(3 * GIB + 1, Some(GIB)));
        assert!(!outgrown(GIB, Some(1)), "the floor keeps a small repo's");
        assert!(outgrown(GIB + 1, Some(1)));
        assert!(!outgrown(GIB, None));
        assert!(outgrown(GIB + 1, None), "with no fresh build, the floor alone");
        assert!(!outgrown(u64::MAX, Some(u64::MAX)));
        assert!(outgrown(u64::MAX, Some(u64::MAX / 3 - 1)));
        assert!(Built { built: 2 * GIB, fresh: None }.outgrown());
    }
}
