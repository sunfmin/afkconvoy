//! The status board: the one comment on a claimed issue that shows a human how
//! far along it is. It is rendered from what the fleet knows and never read
//! back; it carries no time, so the same state renders the same text.

use flux_rs::attrs::*;

use crate::plan::Status;
use crate::record;

/// The marker a status board is found by.
pub const STATUS: &str = "convoy:status";

/// Every stage a claim can be shown at: the closed list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Claimed, its worker at it: no PR yet.
    Claimed,
    /// Its PR is open, and joining the landing train.
    Joining,
    /// Its PR is on the landing train.
    Joined,
    /// Its work is on the base branch.
    Merged,
    /// The issue was closed with nothing landed: it needed no change, or
    /// somebody closed it. One stage, because the settling that finishes a
    /// close cut short cannot tell the two apart without reading the board.
    Closed,
    /// Handed to a human.
    Escalated,
    /// Left waiting on the issues that block it.
    Parked,
}

impl Stage {
    pub const ALL: [Stage; 7] = [
        Stage::Claimed,
        Stage::Joining,
        Stage::Joined,
        Stage::Merged,
        Stage::Closed,
        Stage::Escalated,
        Stage::Parked,
    ];

    /// The stage a tick shows a claim in while it is held: `None` for one the
    /// same tick settles, whose last board the settling writes.
    pub fn held(status: Status) -> Option<Stage> {
        match status {
            Status::NoPr => Some(Stage::Claimed),
            Status::Joining => Some(Stage::Joining),
            Status::Joined => Some(Stage::Joined),
            Status::Landed | Status::Closed => None,
        }
    }

    /// Whether the claim ends in this stage: the board is its last.
    pub fn ends(self) -> bool {
        match self {
            Stage::Claimed | Stage::Joining | Stage::Joined => false,
            Stage::Merged | Stage::Closed | Stage::Escalated | Stage::Parked => true,
        }
    }
}

/// What one status board shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board<'a> {
    /// The fleet instance the claim is, or was, held by.
    pub instance: &'a str,
    pub stage: Stage,
    /// The issue's PR, when it has one.
    pub pr: Option<u64>,
    /// The open issues a parked one waits on.
    pub blocked_by: &'a [u64],
}

/// The steps of the board's task list, in order.
const STEPS: [&str; 5] =
    ["Claimed", "PR open", "Joining the landing train", "On the landing train", "Merged"];

/// How many of the steps are done in `stage`. An off-ramp keeps what was
/// reached: with a PR, as far as joining.
#[spec(fn(stage: Stage, has_pr: bool) -> usize{done: 1 <= done && done <= 5})]
fn steps_done(stage: Stage, has_pr: bool) -> usize {
    match stage {
        Stage::Claimed => 1,
        Stage::Joining => 3,
        Stage::Joined => 4,
        Stage::Merged => 5,
        Stage::Closed | Stage::Escalated | Stage::Parked => {
            if has_pr {
                3
            } else {
                1
            }
        }
    }
}

/// One line of the task list. Only a step the claim reached is ticked.
#[spec(fn(at: usize, done: usize{done <= 5}, pr: Option<u64>) -> String requires at < 5)]
fn step_line(at: usize, done: usize, pr: Option<u64>) -> String {
    let tick = if at < done { "x" } else { " " };
    let pr = match pr {
        Some(pr) if at == 1 => format!(" (#{pr})"),
        _ => String::new(),
    };
    format!("- [{tick}] {}{pr}", STEPS[at])
}

/// How the claim ended, when it did by anything but landing.
fn off_ramp(board: &Board) -> Option<String> {
    match board.stage {
        Stage::Claimed | Stage::Joining | Stage::Joined | Stage::Merged => None,
        Stage::Closed => Some(
            "**Closed with nothing landed** — the issue needed no change, or somebody closed it."
                .to_string(),
        ),
        Stage::Escalated => Some(
            "**Escalated to a human** — the comment that says why is on this issue. Its PR, \
             branch and worktree are kept."
                .to_string(),
        ),
        Stage::Parked => {
            let blockers: Vec<String> =
                board.blocked_by.iter().map(|number| format!("#{number}")).collect();
            Some(format!(
                "**Parked** — waiting on {}. It returns to the frontier when they close.",
                blockers.join(", ")
            ))
        }
    }
}

/// The text of a status board: its marker, who holds the claim, the task
/// list, and the off-ramp the claim took.
pub fn render(board: &Board) -> String {
    let who = if board.stage.ends() { "was worked" } else { "is being worked" };
    let mut lines = vec![
        record::marker(STATUS, &[]),
        format!("**afkconvoy** — this issue {who} by fleet instance `{}`.", board.instance),
        String::new(),
    ];
    let done = steps_done(board.stage, board.pr.is_some());
    lines.push(step_line(0, done, board.pr));
    lines.push(step_line(1, done, board.pr));
    lines.push(step_line(2, done, board.pr));
    lines.push(step_line(3, done, board.pr));
    lines.push(step_line(4, done, board.pr));
    if let Some(ended) = off_ramp(board) {
        lines.push(String::new());
        lines.push(ended);
    }
    lines.join("\n")
}

/// Which of an issue's comments is its status board: the first that carries
/// the marker. What it says is never read.
pub fn board_comment<'a>(bodies: impl IntoIterator<Item = &'a str>) -> Option<usize> {
    for (at, body) in bodies.into_iter().enumerate() {
        if record::read_marker(STATUS, body).is_some() {
            return Some(at);
        }
    }
    None
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A step past the end of the task list.
    #[should_fail]
    fn a_step_the_list_does_not_have(pr: Option<u64>) -> String {
        step_line(5, 5, pr)
    }

    /// More steps ticked than the list has.
    #[should_fail]
    fn more_steps_done_than_there_are(pr: Option<u64>) -> String {
        step_line(0, 6, pr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(stage: Stage, pr: Option<u64>) -> Board<'static> {
        Board { instance: "alpha", stage, pr, blocked_by: &[3, 4] }
    }

    fn ticked(text: &str) -> usize {
        text.lines().filter(|line| line.starts_with("- [x] ")).count()
    }

    #[test]
    fn every_stage_has_a_rendering_of_its_own() {
        let mut seen: Vec<String> = vec![];
        for stage in Stage::ALL {
            let text = render(&board(stage, Some(70)));
            assert!(text.starts_with("<!--convoy:status-->\n**afkconvoy** — "), "{text}");
            assert_eq!(text.lines().filter(|line| line.starts_with("- [")).count(), 5, "{text}");
            assert!(text.contains("fleet instance `alpha`"));
            assert!(!seen.contains(&text), "{stage:?} renders as another stage does");
            seen.push(text);
        }
        assert_eq!(seen.len(), 7);
    }

    #[test]
    fn every_status_a_tick_finds_a_claim_in_is_shown_or_settled() {
        use Status::*;
        let shown: Vec<Option<Stage>> =
            [NoPr, Joining, Joined, Landed, Closed].into_iter().map(Stage::held).collect();
        let (c, j, on) = (Stage::Claimed, Stage::Joining, Stage::Joined);
        assert_eq!(shown, [Some(c), Some(j), Some(on), None, None]);
        for stage in Stage::ALL {
            assert_eq!(stage.ends(), ![c, j, on].contains(&stage), "{stage:?}");
        }
    }

    #[test]
    fn the_task_list_ticks_what_was_reached() {
        let claimed = render(&board(Stage::Claimed, None));
        assert_eq!(ticked(&claimed), 1);
        assert!(claimed.contains("- [x] Claimed\n- [ ] PR open\n- [ ] Joining the landing train"));
        assert!(claimed.contains("is being worked by"));
        let joining = render(&board(Stage::Joining, Some(70)));
        assert!(joining.contains("- [x] PR open (#70)\n- [x] Joining the landing train\n- [ ] On"));
        assert_eq!(ticked(&render(&board(Stage::Joined, Some(70)))), 4);
        let merged = render(&board(Stage::Merged, Some(70)));
        assert_eq!(ticked(&merged), 5);
        assert!(merged.contains("was worked by") && merged.ends_with("- [x] Merged"));
    }

    #[test]
    fn an_off_ramp_keeps_what_was_reached_and_says_how_it_ended() {
        let escalated = render(&board(Stage::Escalated, None));
        assert_eq!(ticked(&escalated), 1);
        assert!(escalated.contains("**Escalated to a human**"));
        assert_eq!(ticked(&render(&board(Stage::Escalated, Some(70)))), 3);
        let parked = render(&board(Stage::Parked, None));
        assert!(parked.ends_with(
            "**Parked** — waiting on #3, #4. It returns to the frontier when they close."
        ));
        assert!(render(&board(Stage::Closed, None)).contains("**Closed with nothing landed**"));
        assert_eq!(ticked(&render(&board(Stage::Closed, Some(70)))), 3);
    }

    #[test]
    fn the_same_state_renders_the_same_text_and_is_found_by_its_marker() {
        let text = render(&board(Stage::Joining, Some(70)));
        assert_eq!(text, render(&board(Stage::Joining, Some(70))));
        let bodies = ["a comment", "<!--convoy:branch name=x-->", text.as_str(), text.as_str()];
        assert_eq!(board_comment(bodies), Some(2));
        assert_eq!(board_comment(["convoy:status", "<!--convoy:statusx-->"]), None);
    }
}
