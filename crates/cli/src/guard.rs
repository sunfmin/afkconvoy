//! `afkconvoy guard`: what the fleet has every worker of a bound repo ask
//! before each tool call (docs/adr/0012), with the call described on standard
//! input and the stage's facts in the environment. How the call is described
//! and how the answer is given are the harness's that asks
//! (`afkconvoy_core::harness`); whether the call is refused is the core's to
//! say, in one place for every harness (`afkconvoy_core::guard`).

use std::io::Read;
use std::process::ExitCode;

use afkconvoy_core::guard::{Limits, refused};
use afkconvoy_core::harness::Harness;

/// Reads one tool call on stdin and the stage's facts from the environment →
/// exit 0, saying nothing, when the worker may make the call; else the
/// reason on stderr and the exit code that refuses it. A call or a stage it
/// cannot read is refused: nothing is allowed unread.
pub fn guard() -> ExitCode {
    // Every worker's harness is the default: no stage names another.
    let harness = Harness::DEFAULT;
    let mut described = String::new();
    let decided = match std::io::stdin().read_to_string(&mut described) {
        Ok(_) => decide(harness, &described, |name| std::env::var(name).ok()),
        Err(error) => Err(format!("the tool call could not be read: {error}")),
    };
    match decided {
        Ok(None) => ExitCode::SUCCESS,
        Ok(Some(why)) | Err(why) => {
            eprintln!("afkconvoy guard: {why}");
            ExitCode::from(harness.refused())
        }
    }
}

/// Why the tool call `harness` `described` is refused under the facts `var`
/// answers for, when it is. An error is a call or a stage that cannot be read.
fn decide(
    harness: Harness,
    described: &str,
    var: impl Fn(&str) -> Option<String>,
) -> Result<Option<String>, String> {
    let unread = |error| format!("what it was handed describes no tool call: {error}");
    let mut described = serde_json::Deserializer::from_str(described);
    let call = harness.call(&mut described).map_err(unread)?;
    described.end().map_err(unread)?;
    Ok(refused(&call, &Limits::read(var)?))
}
