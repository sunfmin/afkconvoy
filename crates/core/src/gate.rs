//! The fleet's side of the gate (docs/adr/0059): what a gate run records of
//! the issue it was run for, and the comment a red run leaves on a PR. The
//! gate itself is the gate crate's, re-exported here.

pub use afkconvoy_gate::gate::*;

use crate::layout::{GATE_KIND, Layout};
use crate::record;

/// The record `GreenRun::write` makes, naming `issue` when the run was of
/// that issue's own tree. No issue is the record `write` makes: a
/// base-branch certification names none.
pub fn write_issued(
    run: &GreenRun,
    tree: &Tree,
    command: &GateCommand,
    issue: Option<u64>,
) -> String {
    match issue {
        Some(issue) => run.write_with(tree, command, &[("issue", &issue.to_string())]),
        None => run.write(tree, command),
    }
}

/// The record of a red run of `command` on `tree` for `issue`, finished at
/// `at`. It is not a green run: `GreenRun::recorded` reads one back as none.
/// The ref is not the green ref, which a red run deletes.
pub fn red_subject(
    tree: &Tree,
    command: &GateCommand,
    seconds: u64,
    host: &str,
    at: u64,
    issue: u64,
) -> String {
    let seconds = seconds.to_string();
    let at = at.to_string();
    let issue = issue.to_string();
    record::write(
        GATE,
        &[
            ("tree", tree.name()),
            ("command", command.text()),
            ("at", &at),
            ("seconds", &seconds),
            ("host", host),
            ("issue", &issue),
            ("red", "1"),
        ],
    )
}

/// Where a red run for `issue` is kept until a work account cites it. The
/// green ref stays one per tree and command; a red one is its own, so an
/// earlier red survives a later green.
pub fn red_ref(layout: Layout, issue: u64, at: u64, tree: u64, command: u64) -> String {
    layout.under(&format!("{GATE_KIND}red-{issue}-{at:016x}-{tree:016x}-{command:016x}"))
}

/// The comment a red gate run leaves on each PR it was a run of: the command,
/// how it ended under the gate limit of `gate_seconds`, and the end of its log.
pub fn red_comment(
    command: &GateCommand,
    ran: Ran,
    gate_seconds: u64,
    excerpt: &[String],
) -> String {
    let ended =
        if ran.timed_out { reached(gate_seconds) } else { format!("exited {}", ran.exit_code) };
    let log = if excerpt.is_empty() { "(no output)".to_string() } else { excerpt.join("\n") };
    format!(
        "The gate is red on the base branch with this PR on it: `{}` {ended}. Nothing landed; \
         the train's worker is fixing the train.\n\n```\n{log}\n```\n",
        command.text()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Tree {
        Tree::named("4b825dc642cb6eb9a060e54bf8d69288fbee4904")
    }

    fn command() -> GateCommand {
        GateCommand::new("make test")
    }

    fn ran(exit_code: i32) -> Ran {
        Ran { exit_code, timed_out: false, clean_before: true, clean_after: true, seconds: 42 }
    }

    fn written(now: u64) -> GreenRun {
        match after_run(&tree(), &command(), ran(0), "mini", now) {
            Recording::Write(run) => run,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_red_run_comments_its_command_and_how_it_ended() {
        let command = command();
        let comment = red_comment(&command, ran(2), 600, &["a".into(), "b".into()]);
        assert!(comment.contains("`make test` exited 2") && comment.contains("```\na\nb\n```"));
        let at_the_limit = Ran { timed_out: true, ..ran(124) };
        let comment = red_comment(&command, at_the_limit, 600, &[]);
        assert!(comment.contains("`make test` reached the gate limit of 600s and was ended"));
    }

    #[test]
    fn a_record_names_the_issue_its_run_was_of_and_a_red_one_is_no_green_run() {
        let (tree, command) = (tree(), command());
        let subject = written(1000).write(&tree, &command);
        assert_eq!(write_issued(&written(1000), &tree, &command, None), subject);
        let issued = write_issued(&written(1000), &tree, &command, Some(7));
        assert!(issued.ends_with(" host=mini issue=7"), "{issued}");
        assert_eq!(GreenRun::recorded(&issued, &tree, &command).map(|run| run.at()), Some(1000));
        let red = red_subject(&tree, &command, 17, "other", 1900, 7);
        assert!(red.contains(" red=1"), "{red}");
        assert_eq!(GreenRun::recorded(&red, &tree, &command), None);
    }
}
