//! `afkconvoy guard`: the hook the fleet adds to every worker of a bound
//! repo (docs/adr/0012). Claude Code runs it before each tool call, with the
//! call described on standard input and the stage's facts in the
//! environment. Whether the call is refused is the core's to say
//! (`afkconvoy_core::guard`).

use std::io::Read;
use std::process::ExitCode;

use afkconvoy_core::guard::{Call, Limits, refused};

/// The exit code that tells Claude Code a hook refuses the call: the call is
/// not made, and what the hook wrote on stderr is shown to the worker.
const REFUSED: u8 = 2;

/// Reads one tool call on stdin and the stage's facts from the environment →
/// exit 0, saying nothing, when the worker may make the call; else the
/// reason on stderr and the exit code that refuses it. A call or a stage it
/// cannot read is refused: nothing is allowed unread.
pub fn guard() -> ExitCode {
    let mut described = String::new();
    let decided = match std::io::stdin().read_to_string(&mut described) {
        Ok(_) => decide(&described, |name| std::env::var(name).ok()),
        Err(error) => Err(format!("the tool call could not be read: {error}")),
    };
    match decided {
        Ok(None) => ExitCode::SUCCESS,
        Ok(Some(why)) | Err(why) => {
            eprintln!("afkconvoy guard: {why}");
            ExitCode::from(REFUSED)
        }
    }
}

/// Why the tool call `described` is refused under the facts `var` answers
/// for, when it is. An error is a call or a stage that cannot be read.
fn decide(described: &str, var: impl Fn(&str) -> Option<String>) -> Result<Option<String>, String> {
    let call: Call = serde_json::from_str(described)
        .map_err(|error| format!("what it was handed describes no tool call: {error}"))?;
    Ok(refused(&call, &Limits::read(var)?))
}
