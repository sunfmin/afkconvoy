//! `afkconvoy guard`: what the fleet has every worker of a bound repo ask
//! before each tool call (docs/adr/0012), with the call described on standard
//! input and the stage's facts in the environment — the harness that asks
//! among them. How the call is described and how the answer is given are
//! that harness's
//! (`afkconvoy_core::harness`); whether the call is refused is the core's to
//! say, in one place for every harness (`afkconvoy_core::guard`).

use std::io::Read;
use std::process::ExitCode;

use afkconvoy_core::guard::{Limits, refused};
use afkconvoy_core::harness::Harness;
use afkconvoy_core::role::HARNESS_ENV;

/// Reads one tool call on stdin and the stage's facts from the environment →
/// exit 0, saying nothing, when the worker may make the call; else the
/// reason on stderr and the exit code that refuses it. A call or a stage it
/// cannot read is refused: nothing is allowed unread.
pub fn guard() -> ExitCode {
    let harness = match asking(std::env::var(HARNESS_ENV).ok().as_deref()) {
        Ok(harness) => harness,
        Err(why) => {
            eprintln!("afkconvoy guard: {why}");
            return ExitCode::from(Harness::UNNAMED.refused());
        }
    };
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

/// The harness that asks, by what the worker's environment `names` for it: a
/// worker started before a start named one is a session of the one there was
/// then. An error is a name that is no harness: its call cannot be read.
fn asking(names: Option<&str>) -> Result<Harness, String> {
    match names {
        None => Ok(Harness::UNNAMED),
        Some(key) => Harness::read(key).ok_or_else(|| Harness::unknown(key)),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The facts of a stage that writes nothing, with `harness` asking.
    fn facts(name: &str) -> Option<String> {
        let value = match name {
            "AFKCONVOY_STAGE" => "look",
            "AFKCONVOY_WRITES" | "AFKCONVOY_CONSULT" => "false",
            "AFKCONVOY_BASE_BRANCH" => "main",
            "AFKCONVOY_ESCALATE_LABEL" => "ready-for-human",
            "AFKCONVOY_LOCK_LABEL" => "fleet-lock",
            _ => return None,
        };
        Some(value.to_string())
    }

    #[test]
    fn the_harness_that_asks_is_the_one_the_worker_s_environment_names() {
        assert_eq!(asking(None), Ok(Harness::ClaudeCode));
        assert_eq!(asking(Some("claude-code")), Ok(Harness::ClaudeCode));
        assert_eq!(asking(Some("pi")), Ok(Harness::Pi));
        assert!(asking(Some("codex")).unwrap_err().contains("`codex` is no harness"));
    }

    #[test]
    fn one_call_is_refused_for_the_same_reason_whichever_harness_describes_it() {
        let claude = r#"{"tool_name":"Bash","tool_input":{"command":"git commit -m x"}}"#;
        let pi = r#"{"toolName":"bash","input":{"command":"git commit -m x"}}"#;
        let why = decide(Harness::ClaudeCode, claude, facts).unwrap().expect("a refusal");
        assert!(why.contains("`writes: false`: `git commit` is refused"), "{why}");
        assert_eq!(decide(Harness::Pi, pi, facts), Ok(Some(why)));
        // A call described in another harness's words cannot be read.
        assert!(decide(Harness::Pi, claude, facts).unwrap_err().contains("describes no tool call"));
        assert!(decide(Harness::ClaudeCode, pi, facts).is_err());
        let allowed = r#"{"toolName":"bash","input":{"command":"ls"}}"#;
        assert_eq!(decide(Harness::Pi, allowed, facts), Ok(None));
    }
}
