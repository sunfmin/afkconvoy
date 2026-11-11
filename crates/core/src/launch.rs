//! Checks an opaque worker launch command and the handshake of a new worker.
//! The host kit owns command selection; this pure module checks the shell's
//! resolution and warns when unattended flags are visibly absent.

use flux_rs::attrs::*;

use crate::harness::Harness;

/// The flags that let an agent work with nobody at its terminal. Only ever
/// looked for, to warn: a worker without one stops at its first permission
/// prompt, which the fleet cannot tell from a worker that finished.
const UNATTENDED: [&str; 8] = [
    "--dangerously-skip-permissions",
    "--dangerously-bypass-approvals-and-sandbox",
    "--yolo",
    "--yes-always",
    "--dangerously-allow-all",
    "--trust-all-tools",
    "--unrestricted",
    "--auto-approve",
];

/// The word whose resolving decides whether the command can run at all.
pub use afkconvoy_harness::first_word;

/// `fault`, of the worker launch command of `harness`, said with the harness
/// and what calls for it (`Team::harnesses`): the stage or the patrol a human
/// changes, or the command they give, to be rid of it.
pub fn of_harness(fault: &str, harness: Harness, callers: &[String]) -> String {
    format!(
        "{fault} — it is the worker launch command of the harness `{}`, called for by {}",
        harness.key(),
        callers.join(", ")
    )
}

/// A command a worker may be started with: its first word resolves in the
/// shell a worker gets. `checked` is the only way to one, so no worker is
/// started on a command nobody resolved.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(resolved: bool)]
#[invariant(resolved)]
pub struct LaunchCommand {
    command: String,
    #[field(bool[resolved])]
    // What the invariant is stated on: nothing reads it back.
    #[allow(dead_code)]
    resolved: bool,
    warning: Option<String>,
}

impl LaunchCommand {
    /// The command, exactly as it was given.
    pub fn as_str(&self) -> &str {
        &self.command
    }

    /// What the human should know about it, though it will be used.
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }
}

/// `command` as one a worker may be started with, given what the worker's
/// shell said its first word is (`resolution`: `type`'s answer, `None` when
/// it found nothing). An error names the word that resolves to nothing: used
/// anyway it starts no worker, and each claim would be escalated for it.
pub fn checked(command: &str, resolution: Option<&str>) -> Result<LaunchCommand, String> {
    let Some(word) = first_word(command) else {
        return Err("the worker launch command is empty".to_string());
    };
    let Some(resolution) = resolution.map(str::trim).filter(|text| !text.is_empty()) else {
        return Err(format!(
            "the worker launch command {command:?} cannot start a worker: {word:?} resolves to \
             nothing in a login shell"
        ));
    };
    let warning = match unattended(command, word, resolution) {
        Some(false) => Some(format!(
            "the worker launch command {command:?} shows no unattended flag ({resolution}): a \
             worker that stops at a permission prompt looks finished to the fleet"
        )),
        _ => None,
    };
    Ok(LaunchCommand { command: command.to_string(), resolved: true, warning })
}

/// Whether an unattended flag is visible: yes; plainly not — the resolution
/// shows the whole of what runs; or unknown (`None`) — a script's path, a
/// function, could carry one inside.
fn unattended(command: &str, word: &str, resolution: &str) -> Option<bool> {
    let mut visible = false;
    for flag in UNATTENDED {
        if command.contains(flag) || resolution.contains(flag) {
            visible = true;
        }
    }
    let whole = resolution.contains(" is an alias for ")
        || resolution.contains(" is aliased to ")
        || word == Harness::DEFAULT.program();
    if visible {
        Some(true)
    } else if whole {
        Some(false)
    } else {
        None
    }
}

/// What a wait on a freshly started worker came back with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waited {
    /// Ready for a prompt.
    Ready,
    /// Asking whether the folder it was started in is trusted: the launch's
    /// trust configuration did not take effect.
    AtTrustPrompt,
    NotReady,
}

/// A worker whose launch configuration did not answer its harness's trust
/// question. The original launch context is kept so the gap is reported once,
/// with everything needed to correct it, rather than retried as a dispatch.
#[derive(Debug)]
pub struct TrustGap {
    pub harness: Harness,
    pub command: String,
    pub worktree: String,
    pub terminal: String,
    pub seconds: u64,
    /// Raw Orca response and exit status, captured before terminal cleanup.
    pub startup_wait: String,
    /// Raw screen-read response, including an unavailable source or failure.
    pub startup_screen: String,
}

impl std::fmt::Display for TrustGap {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "{} worktree trust gap: Orca reported a workspace trust prompt within {}s; harness `{}`; worker launch command `{}`; worktree `{}` (terminal {})\n\nStartup wait (raw):\n{}\n\nStartup screen read (raw):\n{}",
            self.harness.name(),
            self.seconds,
            self.harness.key(),
            self.command,
            self.worktree,
            self.terminal,
            self.startup_wait,
            self.startup_screen,
        )
    }
}

impl std::error::Error for TrustGap {}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A worker started on a command whose first word nobody resolved: the
    /// typo that burns every claim it is tried on.
    #[should_fail]
    fn a_command_nobody_resolved(command: String, resolution: Option<&str>) -> LaunchCommand {
        LaunchCommand { command, resolved: resolution.is_some(), warning: None }
    }

    /// A command known to resolve to nothing, used anyway.
    #[should_fail]
    fn an_unresolved_command_used(command: String) -> LaunchCommand {
        LaunchCommand { command, resolved: false, warning: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: Harness = Harness::ClaudeCode;
    const STOCK: &str = CLAUDE.stock();

    #[test]
    fn a_fault_is_said_with_its_harness_and_what_calls_for_it() {
        let callers = ["the repo's default".to_string(), "the patrol `sweep`".to_string()];
        assert_eq!(
            of_harness("it resolves to nothing", CLAUDE, &callers),
            "it resolves to nothing — it is the worker launch command of the harness \
             `claude-code`, called for by the repo's default, the patrol `sweep`"
        );
    }

    const KIM: &str = "kim is an alias for ANTHROPIC_BASE_URL=https://kimi claude --yolo";

    #[test]
    fn a_command_that_resolves_is_kept_exactly() {
        let passed = "  kim  --model 'big one' ; true ";
        let command = checked(passed, Some(KIM)).unwrap();
        assert_eq!(command.as_str(), passed);
        assert_eq!(command.warning(), None);
        assert_eq!(first_word(passed), Some("kim"));
    }

    #[test]
    fn a_command_whose_first_word_resolves_to_nothing_is_refused() {
        for nothing in [None, Some(""), Some("  \n")] {
            let error = checked("ckim --yolo", nothing).unwrap_err();
            assert!(error.contains("\"ckim\" resolves to nothing"), "{error}");
        }
        assert!(checked("   ", Some("x")).unwrap_err().contains("empty"));
        assert!(checked("", None).unwrap_err().contains("empty"));
    }

    #[test]
    fn a_missing_unattended_flag_is_warned_about_only_when_it_is_plainly_missing() {
        let warned = |command: &str, resolution: &str| {
            checked(command, Some(resolution)).unwrap().warning().map(str::to_string)
        };
        // an alias shows all of what runs
        let plain = warned("kim", "kim is an alias for claude --model x").unwrap();
        assert!(plain.contains("no unattended flag") && plain.contains("\"kim\""), "{plain}");
        assert!(warned("kim", "kim is aliased to `claude'").is_some(), "bash's wording");
        // …and so does `claude` itself
        assert!(warned("claude", "claude is /usr/local/bin/claude").is_some());
        // a flag in the alias, or on the command, is a flag
        assert_eq!(warned("kim", KIM), None);
        assert_eq!(
            warned("kim --dangerously-skip-permissions", "kim is an alias for claude"),
            None
        );
        assert_eq!(warned(STOCK, "claude is /usr/local/bin/claude"), None);
        // a script could carry one inside: unknown is not a warning
        assert_eq!(warned("agent", "agent is /usr/local/bin/agent"), None);
        assert_eq!(warned("kim", "kim is a shell function from /home/u/.zshrc"), None);
    }
}
