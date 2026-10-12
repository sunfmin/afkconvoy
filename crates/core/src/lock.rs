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
         fleet works lock issues only, and no PR of another issue joins the landing train. \
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
            open_blockers: 0,
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
