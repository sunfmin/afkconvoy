//! The tracker: the repo a fleet reads and writes its issues in — its
//! checkout's own, until the checkout's host names another (docs/adr/0027),
//! or its host's local tracker (docs/adr/0091) — and how the repo the fleet
//! works in names one of its issues: in a PR's closing line, a join commit, a
//! branch, a worktree and a claim's ref. The one home of every such name, so
//! that an issue of one tracker is never taken for an issue of the same
//! number in the repo or in another tracker. A tracker that is not the
//! checkout's own is named `<owner>/<tracker>`; a host's local tracker is
//! named so too, `local/<host>` (`local_name`), and every name is made of it
//! alike.

use flux_rs::attrs::*;
use serde::{Deserialize, Serialize};

use crate::closing::issues_closed_by;
use crate::config::RepoName;
use crate::line::Line;
use crate::record::canonical_number;
use crate::worker::worktree_name;

/// What a repo label begins with: `repo:<owner>/<repo>`.
pub const REPO_LABEL: &str = "repo:";

/// The visibility GitHub gives a repo anyone can read: anyone can comment on
/// its issues.
pub const PUBLIC: &str = "public";

/// What a checkout says its tracker is, as `--tracker` says it: `local`.
pub const LOCAL: &str = "local";

/// The repo a fleet reads and writes its issues in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tracker {
    /// The checkout's own repo: `gh` works it out from the checkout, and the
    /// repo names its issues `#<n>`.
    Own,
    /// Another repo, `tracker`, in which the issues of the checkout's repo,
    /// `repo`, sit beside other repos' and carry its repo label.
    Other { tracker: RepoName, repo: RepoName },
    /// The host's local tracker, named `tracker` (`local_name`), in which
    /// the issues of the checkout's repo, `repo`, are those whose repo is
    /// it: a field of the issue, not a label (docs/adr/0091).
    Local { tracker: RepoName, repo: RepoName },
}

/// What a checkout says of its tracker on its host, beyond its own repo,
/// which it says by saying none: another repo, `<owner>/<repo>`, or its
/// host's local tracker, `local`. Kept as that text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum Said {
    Repo(RepoName),
    Local,
}

impl Said {
    /// `text` as a checkout says its tracker: `local`, or a repo's name;
    /// `None` of anything else.
    pub fn read(text: &str) -> Option<Said> {
        match text {
            LOCAL => Some(Said::Local),
            text => RepoName::read(text).map(Said::Repo),
        }
    }

    /// The tracker of a checkout of `repo` on the host named `host` that
    /// says this.
    pub fn tracker(self, host: &str, repo: RepoName) -> Tracker {
        match self {
            Said::Repo(tracker) => Tracker::Other { tracker, repo },
            Said::Local => Tracker::Local { tracker: local_name(host), repo },
        }
    }

    /// The name of the tracker this says, on the host named `host`.
    pub fn name(&self, host: &str) -> RepoName {
        match self {
            Said::Repo(tracker) => tracker.clone(),
            Said::Local => local_name(host),
        }
    }

    /// The repo it names, where it names one.
    pub fn repo(&self) -> Option<&RepoName> {
        match self {
            Said::Repo(tracker) => Some(tracker),
            Said::Local => None,
        }
    }
}

impl std::fmt::Display for Said {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Said::Repo(tracker) => tracker.fmt(f),
            Said::Local => f.write_str(LOCAL),
        }
    }
}

impl From<Said> for String {
    fn from(said: Said) -> String {
        said.to_string()
    }
}

impl TryFrom<String> for Said {
    type Error = String;

    fn try_from(text: String) -> Result<Said, String> {
        Said::read(&text)
            .ok_or_else(|| format!("`{text}` is no tracker, `<owner>/<repo>` nor `{LOCAL}`"))
    }
}

/// The name of the local tracker of the host named `host`: `local/<host>`,
/// `<host>` in lower case with its `.local` suffix dropped (docs/adr/0091),
/// and each character a ref's or a branch's part could not take, or that
/// could end one badly — a `.` among them — a `-`. The one home of the name:
/// the fleet, its workers and its humans name the host's tracker by it.
pub fn local_name(host: &str) -> RepoName {
    let lowered = host.trim().to_ascii_lowercase();
    let host = lowered.strip_suffix(".local").unwrap_or(&lowered);
    let part: String =
        host.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '-' }).collect();
    let part = part.trim_matches('-');
    let part = if part.is_empty() { "host" } else { part };
    RepoName::made(LOCAL, part)
}

/// What a claim's ref, kept under `claim/<key>`, is to a fleet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claimed {
    /// The claim on this issue of its tracker.
    Issue(u64),
    /// The claim on an issue of another tracker — or of the repo itself, to
    /// a fleet whose tracker is another: no claim of this fleet's.
    Elsewhere,
    /// The claim on a run, which is no issue's (`claims::RunClaim`).
    Run,
}

/// One issue GitHub linked to a PR as one it closes: its number, and the
/// repo it is in, `owner/name`, where GitHub said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linked {
    pub number: u64,
    pub repo: Option<String>,
}

/// The repo label of the repo `repo`: what one of its issues carries in a
/// tracker that is not the repo itself.
pub fn repo_label(repo: &RepoName) -> String {
    format!("{REPO_LABEL}{}", repo.as_str().to_ascii_lowercase())
}

/// The repo labels among `labels`, whatever their case.
pub fn repo_labels(labels: &[String]) -> Vec<&str> {
    labels.iter().filter_map(|label| is_repo_label(label).then_some(label.as_str())).collect()
}

/// Whether `label` is a repo label, whatever its case.
fn is_repo_label(label: &str) -> bool {
    label.get(..REPO_LABEL.len()).is_some_and(|head| head.eq_ignore_ascii_case(REPO_LABEL))
}

/// Whether `tracker` can be a part of a ref's name, as a claim on one of its
/// issues names it (`Tracker::claim_key`): git lets no part of a ref's name
/// begin with `.`, nor end with `.` or `.lock`.
pub fn names_a_ref(tracker: &RepoName) -> bool {
    let lowered = lowered(tracker);
    lowered
        .split('/')
        .all(|part| !part.starts_with('.') && !part.ends_with('.') && !part.ends_with(".lock"))
}

/// Whether `visibility`, as GitHub says a repo's, is `PUBLIC`.
pub fn is_public(visibility: &str) -> bool {
    visibility.trim().eq_ignore_ascii_case(PUBLIC)
}

/// The line that has the checkout `checkout` say a private tracker: the fix
/// of a public one (`public`).
pub fn says_private(checkout: &str) -> String {
    format!("afkconvoy run {checkout} --tracker <owner>/<private repo>")
}

/// Why the fleet of the checkout `checkout` is refused, whose tracker,
/// `named`, GitHub says is public: anyone could write what the fleet reads as
/// its state and what its workers read as their task (docs/adr/0027).
pub fn public(named: &str, checkout: &str) -> String {
    format!(
        "the tracker of {checkout}, {named}, is public: anyone can comment on its issues, and so \
         write what the fleet reads as its state and what its workers read as their task. Say a \
         private one: `{}`",
        says_private(checkout)
    )
}

/// `name` as a part of a ref's name or of a worktree's: in lower case, so
/// that two hosts that spell the tracker in two cases meet on one name.
fn lowered(name: &RepoName) -> String {
    name.as_str().to_ascii_lowercase()
}

impl Tracker {
    /// The tracker's name, where it is not the checkout's own repo: another
    /// repo, or the host's local tracker.
    pub fn named(&self) -> Option<&RepoName> {
        match self {
            Tracker::Own => None,
            Tracker::Other { tracker, .. } | Tracker::Local { tracker, .. } => Some(tracker),
        }
    }

    /// The repo label the fleet's issues carry in its tracker: none in the
    /// checkout's own repo, whose issues are all its own, nor in the local
    /// tracker, where an issue's repo is a field.
    pub fn repo_label(&self) -> Option<String> {
        match self {
            Tracker::Own | Tracker::Local { .. } => None,
            Tracker::Other { repo, .. } => Some(repo_label(repo)),
        }
    }

    /// Whether an open issue of the tracker carrying `labels` is the repo's:
    /// in the checkout's own repo one that carries no repo label, which hands
    /// an issue to the repo it names; in another tracker one carrying the
    /// repo's. One that carries more is barred all the same (`plan::LabelBar`).
    /// Of the local tracker, every issue read is the repo's: it is read by
    /// its repo, whatever it carries.
    pub fn holds(&self, labels: &[String]) -> bool {
        let carried = repo_labels(labels);
        match self {
            Tracker::Own => carried.is_empty(),
            Tracker::Other { repo, .. } => {
                carried.iter().any(|label| label.eq_ignore_ascii_case(&repo_label(repo)))
            }
            Tracker::Local { .. } => true,
        }
    }

    /// What an issue of the tracker carrying `labels` lacks to be the repo's
    /// and no other's, as a command that hands it to the fleet makes it so
    /// (`intake`, `lock`): the repo label, of one in another tracker that
    /// carries no repo label; nothing, of one that is the repo's already. An
    /// error saying why of one that is another repo's, or more than one's.
    /// An issue of the local tracker is the repo's by its repo alone, `of`,
    /// the field its tracker keeps, whatever it carries: an error of one of
    /// another repo, nothing of one of this.
    pub fn to_take(&self, labels: &[String], of: Option<&str>) -> Result<Option<String>, String> {
        if let Tracker::Local { repo, .. } = self {
            return match of {
                Some(of) if !of.eq_ignore_ascii_case(repo.as_str()) => {
                    Err(format!("is of {of}: it is that repo's to work, and its fleet's"))
                }
                _ => Ok(None),
            };
        }
        let carried = repo_labels(labels);
        let mine = self.repo_label();
        let others: Vec<&str> = carried
            .iter()
            .copied()
            .filter(|label| mine.as_ref().is_none_or(|mine| !label.eq_ignore_ascii_case(mine)))
            .collect();
        if !others.is_empty() {
            return Err(format!(
                "carries the repo label of another repo ({}): it is that repo's to work, and \
                 its fleet's",
                others.join(", ")
            ));
        }
        Ok(mine.filter(|_| carried.is_empty()))
    }

    /// The line `gh issue <verb> <number>` of this tracker's issue, as a
    /// human runs it in a checkout of the repo: naming the tracker, where it
    /// is another repo — `gh` would act on the checkout's own issue of that
    /// number; `afkconvoy issue <verb> <number>` of the local tracker, which
    /// GitHub does not hold.
    #[spec(fn(&Tracker, &str, u64) -> Line[false])]
    pub fn gh_issue(&self, verb: &'static str, number: u64) -> Line {
        match self {
            Tracker::Own => Line::new().fixed("gh issue ").fixed(verb).fixed(" ").number(number),
            Tracker::Other { tracker, .. } => Line::new()
                .fixed("gh issue ")
                .fixed(verb)
                .fixed(" ")
                .number(number)
                .fixed(" --repo ")
                .program(tracker.as_str()),
            Tracker::Local { .. } => {
                Line::new().fixed("afkconvoy issue ").fixed(verb).fixed(" ").number(number)
            }
        }
    }

    /// Issue `number` as the repo names it, in a PR's closing line and a
    /// join commit: `#7` of its own, `acme/tracker#7` of another tracker's,
    /// `local/<host>#7` of the local tracker's.
    pub fn issue(&self, number: u64) -> String {
        match self.named() {
            None => format!("#{number}"),
            Some(tracker) => format!("{tracker}#{number}"),
        }
    }

    /// The issue of this tracker `text` names as `issue` writes it, whatever
    /// the case of the tracker's name; `None` of a name of any other issue.
    pub fn issue_named(&self, text: &str) -> Option<u64> {
        let number = match self.named() {
            None => text.strip_prefix('#')?,
            Some(tracker) => {
                let (named, number) = text.rsplit_once('#')?;
                if !named.eq_ignore_ascii_case(tracker.as_str()) {
                    return None;
                }
                number
            }
        };
        canonical_number(number)
    }

    /// What the claim on issue `number` is kept under, after `claim/`: `7`
    /// of the checkout's own repo, `acme/tracker/7` of another tracker,
    /// `local/<host>/7` of the local tracker. No run's key holds a `/`.
    pub fn claim_key(&self, number: u64) -> String {
        match self.named() {
            None => number.to_string(),
            Some(tracker) => format!("{}/{number}", lowered(tracker)),
        }
    }

    /// What the claim kept under `key` (`claim_key`) is to this fleet.
    pub fn claimed(&self, key: &str) -> Claimed {
        let issue = match key.rsplit_once('/') {
            Some((tracker, number)) => Some((Some(tracker), number)),
            None => canonical_number(key).map(|_| (None, key)),
        };
        let Some((of, number)) = issue else { return Claimed::Run };
        let mine = match (self.named(), of) {
            (None, None) => true,
            (Some(tracker), Some(of)) => of == lowered(tracker),
            _ => false,
        };
        match canonical_number(number) {
            Some(number) if mine => Claimed::Issue(number),
            _ => Claimed::Elsewhere,
        }
    }

    /// The name issue `number`'s worktree is created under, which orca
    /// derives its branch from: `issue-7-<slug>` of the checkout's own repo,
    /// `acme-tracker-7-<slug>` of another tracker's, `local-<host>-7-<slug>`
    /// of the local tracker's.
    pub fn worktree_name(&self, number: u64, title: &str) -> String {
        let name = worktree_name(number, title);
        match self.named() {
            None => name,
            Some(tracker) => {
                let slug: String = lowered(tracker)
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .collect();
                let rest = name.strip_prefix("issue-").unwrap_or(&name);
                format!("{slug}-{rest}")
            }
        }
    }

    /// The issues of this tracker a PR whose body is `body` closes: those its
    /// body names after a closing keyword as `issue` writes them, and those
    /// GitHub linked to it in the tracker — of the checkout's own repo, those
    /// in the PR's own repo, `of`, where GitHub said which that is. An issue
    /// named or linked in any other repo is none of them.
    pub fn closed_by(&self, body: &str, linked: &[Linked], of: Option<&str>) -> Vec<u64> {
        let linked: Vec<u64> = linked
            .iter()
            .filter_map(|link| self.holds_link(link, of).then_some(link.number))
            .collect();
        issues_closed_by(body, &linked, self.named().map(RepoName::as_str))
    }

    /// Whether `link`, an issue GitHub linked to a PR of the repo `of`, is
    /// one of this tracker's: of the checkout's own repo, one in `of`, or in
    /// no repo GitHub named; of another tracker, one in it — of the local
    /// tracker, which GitHub never links, none.
    fn holds_link(&self, link: &Linked, of: Option<&str>) -> bool {
        match (self.named(), link.repo.as_deref()) {
            (None, Some(repo)) => of.is_none_or(|of| repo.eq_ignore_ascii_case(of)),
            (None, None) => true,
            (Some(tracker), Some(repo)) => repo.eq_ignore_ascii_case(tracker.as_str()),
            (Some(_), None) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn other() -> Tracker {
        let (tracker, repo) = (RepoName::read("Acme/Tracker"), RepoName::read("acme/dev"));
        Tracker::Other { tracker: tracker.unwrap(), repo: repo.unwrap() }
    }

    fn local() -> Tracker {
        Said::Local.tracker("Offices-MacBook-Pro.local", RepoName::read("acme/dev").unwrap())
    }

    #[test]
    fn a_hosts_local_tracker_is_named_for_the_host_in_lower_case_without_local() {
        for (host, named) in [
            ("Offices-MacBook-Pro.local", "local/offices-macbook-pro"),
            ("h1", "local/h1"),
            ("Offices-MBP.lan", "local/offices-mbp-lan"),
            ("-odd host.lock.", "local/odd-host-lock"),
            ("..", "local/host"),
        ] {
            let name = local_name(host);
            assert_eq!(name.as_str(), named, "{host}");
            assert!(names_a_ref(&name), "{host}");
        }
        assert_eq!(local().named().unwrap().as_str(), "local/offices-macbook-pro");
    }

    #[test]
    fn a_checkout_says_local_or_a_repo_and_keeps_it_as_said() {
        assert_eq!(Said::read("local"), Some(Said::Local));
        assert_eq!(
            Said::read("acme/tracker"),
            Some(Said::Repo(RepoName::read("acme/tracker").unwrap()))
        );
        for refused in ["", "Local", "acme", "local/"] {
            assert_eq!(Said::read(refused), None, "{refused}");
        }
        let kept: Vec<Said> = serde_json::from_str(r#"["local", "acme/tracker"]"#).unwrap();
        assert_eq!(serde_json::to_string(&kept).unwrap(), r#"["local","acme/tracker"]"#);
        assert!(serde_json::from_str::<Said>(r#""nowhere""#).is_err());
    }

    #[test]
    fn every_name_of_a_local_trackers_issue_is_made_as_another_trackers() {
        let local = local();
        assert_eq!(local.issue(7), "local/offices-macbook-pro#7");
        assert_eq!(local.issue_named("Local/Offices-MacBook-Pro#7"), Some(7));
        assert_eq!(local.issue_named("#7"), None);
        assert_eq!(local.claim_key(7), "local/offices-macbook-pro/7");
        assert_eq!(local.claimed("local/offices-macbook-pro/7"), Claimed::Issue(7));
        assert_eq!(local.claimed("local/other/7"), Claimed::Elsewhere);
        assert_eq!(local.claimed("7"), Claimed::Elsewhere);
        assert_eq!(local.worktree_name(7, "Add plan"), "local-offices-macbook-pro-7-add-plan");
        let body = "Closes local/offices-macbook-pro#7\nCloses #8\nCloses acme/tracker#9";
        let linked = [Linked { number: 3, repo: Some("acme/dev".into()) }];
        assert_eq!(local.closed_by(body, &linked, Some("acme/dev")), [7]);
        assert_eq!(local.repo_label(), None);
        let labels = ["repo:acme/web".to_string()];
        assert!(local.holds(&labels));
        assert_eq!(local.to_take(&labels, Some("Acme/Dev")), Ok(None));
        let refused = local.to_take(&[], Some("acme/web")).unwrap_err();
        assert!(refused.starts_with("is of acme/web"), "{refused}");
        assert_eq!(local.gh_issue("close", 7).own(), "afkconvoy issue close 7");
    }

    #[test]
    fn the_repo_names_an_issue_of_its_own_by_its_number_and_one_of_another_tracker_by_both() {
        assert_eq!(Tracker::Own.issue(7), "#7");
        assert_eq!(other().issue(7), "Acme/Tracker#7");
        assert_eq!(Tracker::Own.issue_named("#7"), Some(7));
        assert_eq!(other().issue_named("acme/tracker#7"), Some(7));
        for elsewhere in ["#7", "acme/other#7", "tracker#7", "acme/tracker#07"] {
            assert_eq!(other().issue_named(elsewhere), None, "{elsewhere}");
        }
        assert_eq!(Tracker::Own.issue_named("acme/tracker#7"), None);
    }

    #[test]
    fn a_claim_is_this_fleets_only_on_an_issue_of_its_own_tracker() {
        assert_eq!(Tracker::Own.claim_key(7), "7");
        assert_eq!(other().claim_key(7), "acme/tracker/7");
        assert_eq!(Tracker::Own.claimed("7"), Claimed::Issue(7));
        assert_eq!(Tracker::Own.claimed("acme/tracker/7"), Claimed::Elsewhere);
        assert_eq!(other().claimed("acme/tracker/7"), Claimed::Issue(7));
        assert_eq!(other().claimed("7"), Claimed::Elsewhere);
        assert_eq!(other().claimed("acme/other/7"), Claimed::Elsewhere);
        for run in ["patrol-triage", "slow-gate", "precedent-7-c20"] {
            assert_eq!(
                (Tracker::Own.claimed(run), other().claimed(run)),
                (Claimed::Run, Claimed::Run)
            );
        }
    }

    #[test]
    fn a_tracker_names_a_ref_unless_a_part_of_it_begins_with_a_dot_or_ends_as_git_forbids() {
        for named in ["acme/tracker", "acme/the.tracker", "a-b/c_d"] {
            assert!(names_a_ref(&RepoName::read(named).unwrap()), "{named}");
        }
        for refused in ["acme/.github", "acme/tracker.", "acme/tracker.LOCK"] {
            assert!(!names_a_ref(&RepoName::read(refused).unwrap()), "{refused}");
        }
    }

    #[test]
    fn a_public_tracker_is_refused_naming_it_and_the_line_that_says_a_private_one() {
        assert!(is_public("public\n") && is_public("Public"));
        for private in ["private", "internal", ""] {
            assert!(!is_public(private), "{private}");
        }
        let refused = public("acme/dev", "/work/dev");
        assert!(refused.starts_with("the tracker of /work/dev, acme/dev, is public"), "{refused}");
        let fix = "`afkconvoy run /work/dev --tracker <owner>/<private repo>`";
        assert!(refused.ends_with(fix), "{refused}");
    }

    #[test]
    fn an_issues_worktree_is_named_for_its_tracker() {
        assert_eq!(Tracker::Own.worktree_name(7, "Add plan"), "issue-7-add-plan");
        assert_eq!(other().worktree_name(7, "Add plan"), "acme-tracker-7-add-plan");
    }

    #[test]
    fn a_repo_label_names_the_repo_whatever_its_case() {
        assert_eq!(other().repo_label().as_deref(), Some("repo:acme/dev"));
        assert_eq!(Tracker::Own.repo_label(), None);
        let labels = ["bug", "Repo:acme/dev", "repo:acme/web"].map(String::from);
        assert_eq!(repo_labels(&labels), ["Repo:acme/dev", "repo:acme/web"]);
    }

    #[test]
    fn a_tracker_holds_the_issues_carrying_its_repos_label_and_the_own_repo_those_carrying_none() {
        let labels = |all: &[&str]| all.iter().map(|label| label.to_string()).collect::<Vec<_>>();
        assert!(Tracker::Own.holds(&labels(&["ready-for-agent"])));
        assert!(!Tracker::Own.holds(&labels(&["repo:acme/web"])));
        assert!(other().holds(&labels(&["Repo:Acme/Dev"])));
        assert!(other().holds(&labels(&["repo:acme/dev", "repo:acme/web"])));
        assert!(!other().holds(&labels(&["repo:acme/web"])));
        assert!(!other().holds(&labels(&["ready-for-agent"])));
    }

    #[test]
    fn an_issue_handed_to_the_fleet_is_given_the_repo_label_unless_it_is_another_repos() {
        let labels = |all: &[&str]| all.iter().map(|label| label.to_string()).collect::<Vec<_>>();
        assert_eq!(other().to_take(&labels(&["bug"]), None), Ok(Some("repo:acme/dev".to_string())));
        assert_eq!(other().to_take(&labels(&["Repo:Acme/Dev"]), None), Ok(None));
        assert_eq!(Tracker::Own.to_take(&labels(&["bug"]), None), Ok(None));
        for theirs in [&["repo:acme/web"][..], &["repo:acme/dev", "repo:acme/web"]] {
            let refused = other().to_take(&labels(theirs), None).unwrap_err();
            assert!(refused.contains("(repo:acme/web)"), "{refused}");
        }
        let refused = Tracker::Own.to_take(&labels(&["repo:acme/web"]), None).unwrap_err();
        assert!(refused.contains("(repo:acme/web)"), "{refused}");
    }

    #[test]
    fn a_pr_closes_the_issues_of_the_tracker_it_names_and_no_other() {
        let linked = |number, repo: &str| Linked { number, repo: Some(repo.to_string()) };
        let links =
            [linked(3, "acme/dev"), linked(4, "acme/tracker"), Linked { number: 5, repo: None }];
        assert_eq!(Tracker::Own.closed_by("Closes #7", &links, Some("acme/dev")), [3, 5, 7]);
        assert_eq!(Tracker::Own.closed_by("Closes acme/tracker#8", &[], None), [] as [u64; 0]);
        assert_eq!(other().closed_by("Closes acme/tracker#8\nCloses #7", &links, None), [4, 8]);
    }
}
