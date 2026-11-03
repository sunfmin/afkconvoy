//! The status board: the one comment on a claimed issue that shows a human how
//! far along it is. It is rendered from what the fleet knows and never read
//! back; it carries no time, so the same state renders the same text.

use flux_rs::attrs::*;

use crate::left::Kept;
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
    /// Its stage declared its verdict: the claim ended, and the issue is
    /// where its stage records put it.
    Declared,
}

impl Stage {
    pub const ALL: [Stage; 7] = [
        Stage::Claimed,
        Stage::Joining,
        Stage::Merged,
        Stage::Closed,
        Stage::Escalated,
        Stage::Parked,
        Stage::Declared,
    ];

    /// The stage a tick shows a claim in while it is held: `None` for one the
    /// same tick settles, whose last board the settling writes.
    pub fn held(status: Status) -> Option<Stage> {
        match status {
            Status::NoPr | Status::AtStage => Some(Stage::Claimed),
            Status::Landing => Some(Stage::Joining),
            Status::Landed | Status::Closed | Status::Gone => None,
        }
    }

    /// Whether the claim ends in this stage: the board is its last.
    pub fn ends(self) -> bool {
        match self {
            Stage::Claimed | Stage::Joining => false,
            Stage::Merged | Stage::Closed | Stage::Escalated | Stage::Parked | Stage::Declared => {
                true
            }
        }
    }
}

/// What one status board shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board<'a> {
    /// The fleet instance the claim is, or was, held by.
    pub instance: &'a str,
    /// What the afkconvoy that writes the board was built from.
    pub version: &'a str,
    pub stage: Stage,
    /// The issue's PR, when it has one.
    pub pr: Option<u64>,
    /// The open issues a parked one waits on.
    pub blocked_by: &'a [u64],
    /// Whether a parked one waits on the next cut of its repo.
    pub cut: bool,
    /// The stage a claim's verdict left the issue at, when that stage is a
    /// human's: the issue waits there for them.
    pub waits_at: Option<&'a str>,
    /// What an escalated issue's board ends with: the lines that answer it
    /// (`waiting::Waits::answer_lines`).
    pub answer: Option<&'a str>,
    /// The worktree an escalated or parked claim left on its host, when it
    /// holds what the remote does not: the board says so.
    pub kept: Option<Kept>,
}

/// The steps of the board's task list, in order.
const STEPS: [&str; 4] = ["Claimed", "PR open", "Taking a turn", "Merged"];

/// How many of the steps are done in `stage`. An off-ramp keeps what was
/// reached: with a PR, as far as joining.
#[spec(fn(stage: Stage, has_pr: bool) -> usize{done: 1 <= done && done <= 4})]
fn steps_done(stage: Stage, has_pr: bool) -> usize {
    match stage {
        Stage::Claimed => 1,
        Stage::Joining => 3,
        Stage::Merged => 4,
        Stage::Closed | Stage::Escalated | Stage::Parked | Stage::Declared => {
            if has_pr {
                3
            } else {
                1
            }
        }
    }
}

/// One line of the task list. Only a step the claim reached is ticked.
#[spec(fn(at: usize, done: usize{done <= 4}, pr: Option<u64>) -> String requires at < 4)]
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
        Stage::Claimed | Stage::Joining | Stage::Merged => None,
        Stage::Closed => Some(
            "**Closed with nothing landed** — the issue needed no change, or somebody closed it."
                .to_string(),
        ),
        Stage::Escalated => {
            let escalated = format!(
                "**Escalated to a human** — the comment that says why is on this issue. Its PR \
                 and branch are kept.{}",
                kept_line(board)
            );
            Some(match board.answer {
                Some(answer) => format!("{escalated}\n\n{answer}"),
                None => escalated,
            })
        }
        Stage::Parked => {
            let mut on: Vec<String> =
                board.blocked_by.iter().map(|number| format!("#{number}")).collect();
            let blockers = on.len();
            if board.cut {
                on.push("the next cut of this repo".to_string());
            }
            let returns = match (blockers, board.cut) {
                (0, true) => "once a cut has published and installed a new version",
                (_, true) => {
                    "once they close and a cut has published and installed a new version"
                }
                _ => "when they close",
            };
            Some(format!(
                "**Parked** — waiting on {}. It returns to the frontier {returns}.{}",
                on.join(", "),
                kept_line(board)
            ))
        }
        Stage::Declared => Some(match board.waits_at {
            None => "**Its stage declared its verdict** — the stage record on this issue says \
                     which, and the `stage:` label where the issue is now."
                .to_string(),
            Some(stage) => format!(
                "**Its stage declared its verdict, and the issue waits for a human** — the \
                 stage `{stage}` that follows is a human's. No worker is started and no claim is \
                 held until they declare its verdict with `afkconvoy verdict`."
            ),
        }),
    }
}

/// The line that says a worktree is kept, with the space that leads it; empty
/// when none is.
fn kept_line(board: &Board) -> String {
    match &board.kept {
        Some(kept) => format!(" {}", kept.board_line()),
        None => String::new(),
    }
}

/// The text of a status board: its marker, who holds the claim, the task
/// list, and the off-ramp the claim took.
pub fn render(board: &Board) -> String {
    let who = if board.stage.ends() { "was worked" } else { "is being worked" };
    let mut lines = vec![
        record::marker(STATUS, &[]),
        format!(
            "**afkconvoy** — this issue {who} by fleet instance `{}`, afkconvoy `{}`.",
            board.instance, board.version
        ),
        String::new(),
    ];
    let done = steps_done(board.stage, board.pr.is_some());
    lines.push(step_line(0, done, board.pr));
    lines.push(step_line(1, done, board.pr));
    lines.push(step_line(2, done, board.pr));
    lines.push(step_line(3, done, board.pr));
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
        step_line(4, 4, pr)
    }

    /// More steps ticked than the list has.
    #[should_fail]
    fn more_steps_done_than_there_are(pr: Option<u64>) -> String {
        step_line(0, 5, pr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::left::KeptBecause;

    fn board(stage: Stage, pr: Option<u64>) -> Board<'static> {
        Board {
            instance: "alpha",
            version: "0.2.0+11.g9f58035",
            stage,
            pr,
            blocked_by: &[3, 4],
            cut: false,
            waits_at: None,
            answer: None,
            kept: None,
        }
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
            assert_eq!(text.lines().filter(|line| line.starts_with("- [")).count(), 4, "{text}");
            assert!(text.contains("fleet instance `alpha`, afkconvoy `0.2.0+11.g9f58035`."));
            assert!(!seen.contains(&text), "{stage:?} renders as another stage does");
            seen.push(text);
        }
        assert_eq!(seen.len(), 7);
    }

    #[test]
    fn every_status_a_tick_finds_a_claim_in_is_shown_or_settled() {
        use Status::*;
        let shown: Vec<Option<Stage>> =
            [NoPr, Landing, Landed, Closed].into_iter().map(Stage::held).collect();
        let (c, j) = (Stage::Claimed, Stage::Joining);
        assert_eq!(shown, [Some(c), Some(j), None, None]);
        for stage in Stage::ALL {
            assert_eq!(stage.ends(), ![c, j].contains(&stage), "{stage:?}");
        }
    }

    #[test]
    fn the_task_list_ticks_what_was_reached() {
        let claimed = render(&board(Stage::Claimed, None));
        assert_eq!(ticked(&claimed), 1);
        assert!(claimed.contains("- [x] Claimed\n- [ ] PR open\n- [ ] Taking a turn"));
        assert!(claimed.contains("is being worked by"));
        let joining = render(&board(Stage::Joining, Some(70)));
        assert!(joining.contains("- [x] PR open (#70)\n- [x] Taking a turn\n- [ ] Merged"));
        assert_eq!(ticked(&render(&board(Stage::Merged, Some(70)))), 4);
        let merged = render(&board(Stage::Merged, Some(70)));
        assert_eq!(ticked(&merged), 4);
        assert!(merged.contains("was worked by") && merged.ends_with("- [x] Merged"));
    }

    #[test]
    fn an_off_ramp_keeps_what_was_reached_and_says_how_it_ended() {
        let escalated = render(&board(Stage::Escalated, None));
        assert_eq!(ticked(&escalated), 1);
        assert!(escalated.contains("**Escalated to a human**"));
        assert_eq!(ticked(&render(&board(Stage::Escalated, Some(70)))), 3);
        // It ends with the lines that answer it, where it was told them.
        let answer = Some("**To answer it**:\n- rule on it — `afkconvoy rule --issue 7`");
        let answered = render(&Board { answer, ..board(Stage::Escalated, None) });
        assert!(answered.ends_with(
            "are kept.\n\n**To answer it**:\n- rule on it — `afkconvoy rule --issue 7`"
        ));
        let parked = render(&board(Stage::Parked, None));
        assert!(parked.ends_with(
            "**Parked** — waiting on #3, #4. It returns to the frontier when they close."
        ));
        // A worktree kept on its host is said, with what only it holds.
        let kept = Some(Kept { because: KeptBecause::Unpushed, unpushed: Some(2) });
        for stage in [Stage::Parked, Stage::Escalated] {
            let text = render(&Board { kept, ..board(stage, None) });
            assert!(
                text.contains(
                    "Its worktree is kept on the host that worked it: it holds 2 commits"
                ),
                "{text}"
            );
            assert!(!render(&board(stage, None)).contains("worktree"), "{stage:?}");
        }
        assert!(render(&board(Stage::Closed, None)).contains("**Closed with nothing landed**"));
        assert_eq!(ticked(&render(&board(Stage::Closed, Some(70)))), 3);
        let declared = render(&board(Stage::Declared, Some(70)));
        assert!(declared.contains("was worked by") && declared.contains("**Its stage declared"));
        assert_eq!(Stage::held(Status::AtStage), Some(Stage::Claimed));
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
