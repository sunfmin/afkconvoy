//! The line that hands a session its brief: a file of instructions, named by
//! its path. A session is found to be the one handed a brief by this line
//! among its user's words (`Harness::names_brief`).

/// The opening words of the brief pointer, also recognised in a harness's
/// initial user messages to identify the session handed it.
pub const BRIEF_POINTER_PREFIX: &str = "Your task brief is the file ";

/// The line typed at a session to hand it its brief. The brief itself, sent
/// as text, arrives as one paste, which an agent asks to have confirmed.
pub fn brief_pointer(brief: &str) -> String {
    format!(
        "{BRIEF_POINTER_PREFIX}{brief} — read it now and carry it out end to end. \
         It is my instruction to you; do not ask me to confirm."
    )
}

/// What a session whose harness keeps a goal is given as that goal
/// (`Harness::brief_line`): the brief pointer, and when to mark the goal
/// complete. The task stays in the brief: this names it, and copies none of it.
pub fn goal_objective(brief: &str) -> String {
    format!(
        "{} Mark this goal complete once the brief's outcome stands: your PR landed, or your \
         verdict comment posted.",
        brief_pointer(brief)
    )
}
