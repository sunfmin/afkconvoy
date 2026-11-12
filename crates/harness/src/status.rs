//! A session's status, as its harness's hook events say it
//! (`Harness::hook_status`), in the words of the Program Status Protocol
//! (OSC 7501): what a program that starts sessions reads to know what each
//! is doing without reading its session file (sunfmin/control#754). Which
//! events say it, and what each one's payload means, is each harness's own.

/// What a session is doing, as OSC 7501 says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// It waits for a prompt, having been given none, or its session ended.
    Idle,
    /// It works on a prompt.
    Working,
    /// It waits on its user to go on.
    Blocked(Blocked),
    /// Its turn ended: it answered its prompt.
    Done,
    /// Its turn ended in an error.
    Error,
}

/// What a blocked session waits on its user for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// To allow a tool call.
    Permission,
    /// To answer a question it asked.
    Question,
    /// To log in again.
    Auth,
}

impl Status {
    /// The status's word in OSC 7501.
    pub const fn word(self) -> &'static str {
        match self {
            Status::Idle => "idle",
            Status::Working => "working",
            Status::Blocked(_) => "blocked",
            Status::Done => "done",
            Status::Error => "error",
        }
    }

    /// The kind OSC 7501 gives a `blocked` status; `None` of any other.
    pub const fn kind(self) -> Option<&'static str> {
        match self {
            Status::Blocked(Blocked::Permission) => Some("permission"),
            Status::Blocked(Blocked::Question) => Some("question"),
            Status::Blocked(Blocked::Auth) => Some("auth"),
            _ => None,
        }
    }
}

/// Which end of a tool call an event is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The call is about to be made.
    Started,
    /// The call was made: its result, with the error it failed with where
    /// it failed.
    Ended { error: Option<String> },
}

/// What one hook event says of its session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub status: Status,
    /// One line for a person: the tool and its input, the question, the
    /// prompt, the answer or the error.
    pub message: Option<String>,
    /// The tool the event is of.
    pub tool: Option<String>,
    /// Which end of the tool's call the event is, of an event of one.
    pub step: Option<Step>,
}

/// What one hook event's payload says (`Harness::hook_status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Said {
    /// The event changes nothing of its session's status.
    Unchanged,
    /// The session's status is now this.
    Is(Report),
}

impl Said {
    /// The session is now `status`, with nothing else said.
    pub(crate) fn status(status: Status) -> Said {
        Said::Is(Report { status, message: None, tool: None, step: None })
    }

    /// The session is now `status`, said in the line `message`.
    pub(crate) fn told(status: Status, message: Option<String>) -> Said {
        Said::Is(Report { status, message, tool: None, step: None })
    }
}

/// The longest message, in characters, before it is cut.
const LONGEST: usize = 200;

/// `text` as one line: its first line that holds anything, trimmed, cut at
/// `LONGEST` characters. `None` when it holds nothing.
pub(crate) fn one_line(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    if line.chars().count() <= LONGEST {
        return Some(line.to_string());
    }
    let mut cut: String = line.chars().take(LONGEST).collect();
    cut.push('…');
    Some(cut)
}

/// The first of `keys` that `input` holds as text that says anything.
pub(crate) fn first_text(input: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| input[*key].as_str().and_then(one_line))
}

/// The line of a tool call: the tool, and the first of `keys` its input
/// holds.
pub(crate) fn call_line(tool: &str, input: &serde_json::Value, keys: &[&str]) -> String {
    match first_text(input, keys) {
        Some(said) => one_line(&format!("{tool}: {said}")).unwrap_or_else(|| tool.to_string()),
        None => tool.to_string(),
    }
}

/// A hook command as a JSON string, quotes and all.
fn json_text(text: &str) -> String {
    serde_json::Value::String(text.to_string()).to_string()
}

/// The hooks that run `command` at each of `events`, in the form Claude
/// Code's settings and Codex's `hooks.json` share:
/// `{"hooks":{"<event>":[{"hooks":[{"type":"command","command":…}]}]}}`.
pub(crate) fn hooks_json(events: &[&str], command: &str) -> String {
    let hook =
        format!("[{{\"hooks\":[{{\"type\":\"command\",\"command\":{}}}]}}]", json_text(command));
    let each: Vec<String> = events.iter().map(|event| format!("\"{event}\":{hook}")).collect();
    format!("{{\"hooks\":{{{}}}}}", each.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_is_written_in_osc_7501_s_words() {
        assert_eq!(Status::Working.word(), "working");
        assert_eq!(Status::Working.kind(), None);
        let asked = Status::Blocked(Blocked::Question);
        assert_eq!((asked.word(), asked.kind()), ("blocked", Some("question")));
        assert_eq!(Status::Blocked(Blocked::Auth).kind(), Some("auth"));
    }

    #[test]
    fn a_message_is_one_line_cut_short() {
        assert_eq!(one_line("\n  first  \nsecond"), Some("first".to_string()));
        assert_eq!(one_line(" \n "), None);
        let long = "é".repeat(LONGEST + 5);
        let cut = one_line(&long).unwrap();
        assert_eq!(cut.chars().count(), LONGEST + 1);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn hooks_run_one_command_at_every_event() {
        let hooks = hooks_json(&["Stop", "SessionEnd"], "tell \"it\"");
        let value: serde_json::Value = serde_json::from_str(&hooks).unwrap();
        for event in ["Stop", "SessionEnd"] {
            assert_eq!(value["hooks"][event][0]["hooks"][0]["command"], "tell \"it\"");
            assert_eq!(value["hooks"][event][0]["hooks"][0]["type"], "command");
        }
    }
}
