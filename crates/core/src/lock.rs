//! The fleet lock (docs/adr/0011): which issues are one, what a standing lock
//! keeps the fleet off, and the record a worker that raises one leaves.

use crate::config::Config;
use crate::plan::Issue;
use crate::record;

/// The kind of record the comment of a raised lock carries.
pub const LOCK: &str = "convoy:lock";

/// The fleet locks standing among the open `issues`: those carrying the
/// repo's lock label, lowest number first. Whatever else an issue carries, and
/// whoever claims it, says nothing here: a lock stands until its issue closes.
pub fn standing(issues: &[Issue], config: &Config) -> Vec<u64> {
    let mut locks: Vec<u64> = vec![];
    for issue in issues {
        if issue.labels.contains(&config.lock_label) {
            locks.push(issue.number);
        }
    }
    locks.sort();
    locks
}

/// Whether the standing `locks` keep the fleet off `issue`: one stands, and
/// `issue` is not one. The one home of the rule: the frontier, a pass and a
/// join all ask it.
pub fn keeps_off(locks: &[u64], issue: u64) -> bool {
    !locks.is_empty() && !locks.contains(&issue)
}

/// The standing `locks` as a sentence names them: `#7`, or `#7, #9`.
pub fn named(locks: &[u64]) -> String {
    let numbers: Vec<String> = locks.iter().map(|lock| format!("#{lock}")).collect();
    numbers.join(", ")
}

/// Why an issue the standing `locks` keep the fleet off is not worked.
pub fn reason(locks: &[u64]) -> String {
    format!("a fleet lock stands ({})", named(locks))
}

/// A fleet lock that has ended: its issue, and when that closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed {
    pub number: u64,
    /// When its issue closed, in epoch seconds.
    pub at: u64,
}

/// The lock that explains an `escalate` or a failure declared at `at` and not
/// acted on since: one that closed no earlier — it stood when that was
/// declared, or was raised before anybody read it. A broken base branch is
/// what such a lock is raised for, and what it explains is run again, not
/// carried out. Of several, the last to close; `None` where none explains it.
pub fn explains(closed: &[Closed], at: u64) -> Option<u64> {
    let mut last: Option<Closed> = None;
    for lock in closed {
        if lock.at >= at && last.is_none_or(|found| lock.at > found.at) {
            last = Some(*lock);
        }
    }
    last.map(|lock| lock.number)
}

/// What the stage record of a stage that runs again says, the lock `lock`
/// having explained what stopped it.
pub fn closed_text(lock: u64) -> String {
    format!(
        "A fleet lock stood (#{lock}) and has closed. What this stage declared, or how it \
         stopped, may be the fault that lock was raised for and not this issue's: it is not \
         carried out. The stage runs again, which counts as no rework."
    )
}

/// The comment a raised lock leaves on its issue: the record of the role that
/// raised it — `None`: a worker of an unbound repo, which has none — and
/// what the lock does, for whoever reads the issue.
pub fn raised_comment(role: Option<&str>) -> String {
    let marker = record::marker(LOCK, &[("role", role.unwrap_or(""))]);
    let by = match role {
        Some(role) => format!("a worker in the role `{role}`"),
        None => "a worker in no role".to_string(),
    };
    format!(
        "{marker}\n**afkconvoy** — {by} raised a fleet lock on this issue. While it is open the \
         fleet works lock issues only, and no PR of another issue lands in a turn. \
         Closing this issue ends the lock."
    )
}

/// The role a raised lock's comment records, when `body` is one and names a
/// role.
pub fn raised_by(body: &str) -> Option<String> {
    record::read_marker(LOCK, body)?.get("role").map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;

    fn config(lock_label: Option<&str>) -> Config {
        Config::new(Settings {
            base_branch: Some("main".into()),
            gate_command: Some("true".into()),
            lock_label: lock_label.map(str::to_string),
            ..Settings::default()
        })
        .unwrap()
    }

    fn issue(number: u64, labels: &[&str]) -> Issue {
        Issue {
            number,
            title: String::new(),
            labels: labels.iter().map(|label| label.to_string()).collect(),
            open_blockers: vec![],
        }
    }

    #[test]
    fn a_lock_is_an_open_issue_carrying_the_lock_label_whatever_else_it_carries() {
        let issues = [
            issue(9, &["fleet-lock"]),
            issue(2, &["ready-for-agent"]),
            issue(4, &["fleet-lock", "epic"]),
            issue(5, &["stop"]),
        ];
        assert_eq!(standing(&issues, &config(None)), [4, 9], "lowest first");
        assert_eq!(standing(&issues, &config(Some("stop"))), [5]);
        assert_eq!(standing(&issues[1..2], &config(None)), Vec::<u64>::new());
    }

    #[test]
    fn a_standing_lock_keeps_the_fleet_off_every_issue_that_is_not_one() {
        assert!(keeps_off(&[4, 9], 2));
        assert!(!keeps_off(&[4, 9], 9));
        assert!(!keeps_off(&[], 2), "no lock stands: nothing is kept off");
        assert_eq!(reason(&[4, 9]), "a fleet lock stands (#4, #9)");
    }

    #[test]
    fn a_closed_lock_explains_what_was_declared_before_it_closed() {
        let closed = [Closed { number: 13, at: 500 }, Closed { number: 4, at: 100 }];
        assert_eq!(explains(&closed, 450), Some(13), "declared while #13 stood");
        assert_eq!(explains(&closed, 500), Some(13));
        assert_eq!(explains(&closed, 90), Some(13), "of several, the last to close");
        assert_eq!(explains(&closed, 501), None, "declared once the last lock had closed");
        assert_eq!(explains(&[], 90), None);
        assert!(closed_text(13).starts_with("A fleet lock stood (#13) and has closed."));
    }

    #[test]
    fn a_raised_lock_records_the_role_that_raised_it() {
        let comment = raised_comment(Some("watch"));
        assert!(comment.starts_with("<!--convoy:lock role=watch-->\n**afkconvoy** — "));
        assert!(comment.contains("a worker in the role `watch` raised a fleet lock"));
        assert_eq!(raised_by(&comment).as_deref(), Some("watch"));
        let unbound = raised_comment(None);
        assert!(unbound.starts_with("<!--convoy:lock-->\n"), "{unbound}");
        assert_eq!(raised_by(&unbound), None);
        assert_eq!(raised_by("<!--convoy:status-->"), None);
    }
}
